"""Pocket TTS's streaming steps rewritten for the ONNX export, with pocket_tts's own layers (same weights): every
streaming state is an explicit input and output, nothing is changed in place. Checked against pocket to 1.5e-6
(the spike, 2026-10-19). The Mimi decoder's convolutions of kernel 1 keep no state: they are left out of its inputs
(zero-sized tensors trouble the runtimes)."""
import math
import torch
import torch.nn.functional as F
from torch import nn
from pocket_tts.modules.conv import StreamingConv1d, StreamingConvTranspose1d
from pocket_tts.modules.seanet import SEANetResnetBlock


def rope(q, k, offset, max_period):
    # q, k: [B, T, H, D]; offset: scalar tensor (absolute position of the first step)
    B, T, H, D = q.shape
    ds = torch.arange(D // 2, dtype=torch.float32)
    freqs = torch.exp(ds * (-math.log(max_period) * 2 / D))
    if isinstance(offset, torch.Tensor):
        offset = offset.to(torch.float32)
    ts = (torch.arange(T, dtype=torch.float32) + offset).view(-1, 1, 1)
    q = q.reshape(B, T, H, D // 2, 2)
    k = k.reshape(B, T, H, D // 2, 2)
    qr, qi = q[..., 0], q[..., 1]
    kr, ki = k[..., 0], k[..., 1]
    rotr, roti = torch.cos(freqs * ts), torch.sin(freqs * ts)
    qo = torch.stack([qr * rotr - qi * roti, qr * roti + qi * rotr], dim=-1).reshape(B, T, H, D)
    ko = torch.stack([kr * rotr - ki * roti, kr * roti + ki * rotr], dim=-1).reshape(B, T, H, D)
    return qo, ko


def attn_layer(layer, x, past, offset, mask, max_period, keep=None):
    """One StreamingTransformerLayer step. past: [2, B, P, H, D]. Returns (x, present)."""
    sa = layer.self_attn
    B, T, C = x.shape
    H, D = sa.num_heads, sa.dim_per_head
    h = layer.norm1(x)
    q, k, v = torch.unbind(sa.in_proj(h).reshape(B, T, 3, H, D), dim=2)
    q, k = rope(q, k, offset, max_period)
    K = torch.cat([past[0], k], dim=1)
    V = torch.cat([past[1], v], dim=1)
    a = F.scaled_dot_product_attention(q.transpose(1, 2), K.transpose(1, 2), V.transpose(1, 2), mask)
    a = sa.out_proj(a.transpose(1, 2).reshape(B, T, H * D))
    x = x + layer.layer_scale_1(a)
    u = layer.linear2(F.gelu(layer.linear1(layer.norm2(x)), approximate="tanh"))
    x = x + layer.layer_scale_2(u)
    present = torch.stack([K, V])
    if keep is not None:
        present = present[:, :, -keep:]
    return x, present


class FlowLMStep(nn.Module):
    """x [1, T, 1024] (text embeddings, or the embedded previous latent), past_kv [6, 2, 1, P, 16, 64],
    noise [1, 32] (already scaled by sqrt(temp)) -> latent [1, 32] (fed back), eos_logit [1, 1],
    present_kv [6, 2, 1, P+T, 16, 64], frame [1, 1, 32] (the latent denormalized: what the audio decoder takes, so the
    decoder is the same for every language)."""

    def __init__(self, flow_lm):
        super().__init__()
        self.layers = flow_lm.transformer.layers
        self.max_period = flow_lm.transformer.rope.max_period
        self.out_norm = flow_lm.out_norm
        self.out_eos = flow_lm.out_eos
        self.flow_net = flow_lm.flow_net
        self.emb_std = flow_lm.emb_std
        self.emb_mean = flow_lm.emb_mean

    def forward(self, x, past_kv, noise):
        T = x.shape[1]
        P = past_kv.shape[3]
        offset = P  # int / SymInt in export
        pos_q = torch.arange(T) + P
        pos_k = torch.arange(P + T)
        mask = (pos_k[None, :] <= pos_q[:, None])[None, None]
        presents = []
        for i, layer in enumerate(self.layers):
            x, pres = attn_layer(layer, x, past_kv[i], offset, mask, self.max_period)
            presents.append(pres)
        h = self.out_norm(x)[:, -1]
        eos = self.out_eos(h)
        s = torch.zeros_like(noise[..., :1])
        t = torch.ones_like(noise[..., :1])
        latent = noise + self.flow_net(h, s, t, noise)
        frame = (latent * self.emb_std + self.emb_mean)[:, None, :]
        return latent, eos, torch.stack(presents), frame


def _conv_step(m: StreamingConv1d, x, prev):
    if prev.shape[-1] > 0:
        x = torch.cat([prev, x], dim=-1)
        return m.conv(x), x[..., -prev.shape[-1]:]
    return m.conv(x), prev


def _convtr_step(m: StreamingConvTranspose1d, x, partial):
    y = m.convtr(x)
    PT = partial.shape[-1]
    head = y[..., :PT] + partial
    y = torch.cat([head, y[..., PT:]], dim=-1)
    tail = y[..., -PT:]
    if m.convtr.bias is not None:
        tail = tail - m.convtr.bias[:, None]
    return y[..., :-PT], tail


class MimiDecodeStep(nn.Module):
    """One latent frame -> 1920 samples. Static shapes: the Mimi transformer keeps a fixed window of W=250 past
    keys (its attention context), padded at start and masked by absolute position.
    Inputs: frame [1, 1, 32] (FlowLM's `frame`, denormalized), pos [1] int64 (Mimi steps done = 16 x frames),
    tr_kv [2 layers, 2, 1, 250, 8, 64], conv states... Outputs: audio [1, 1, 1920], tr_kv', conv states'."""

    W = 250

    def __init__(self, tts):
        super().__init__()
        mimi = tts.mimi
        self.quant = mimi.quantizer.output_proj
        self.up = mimi.upsample.convtr
        self.tr = mimi.decoder_transformer.transformer
        self.context = self.tr.context
        self.max_period = self.tr.max_period
        self.dec = mimi.decoder
        # ordered streaming states of the SEANet decoder
        self.state_shapes = []
        for layer in self.dec.model:
            if isinstance(layer, StreamingConv1d):
                self.state_shapes.append(("conv", self._prev_shape(layer)))
            elif isinstance(layer, StreamingConvTranspose1d):
                self.state_shapes.append(("convtr", (1, layer.convtr.out_channels, layer._kernel_size - layer._stride)))
            elif isinstance(layer, SEANetResnetBlock):
                for sub in layer.block:
                    if isinstance(sub, StreamingConv1d):
                        self.state_shapes.append(("conv", self._prev_shape(sub)))
        # Only the states that hold something are inputs: a kernel-1 convolution has none.
        self.kept = [shape[-1] > 0 for _, shape in self.state_shapes]

    @staticmethod
    def _prev_shape(c):
        return (1, c.conv.in_channels, c._effective_kernel_size - c._stride)

    def init_states(self):
        up = torch.zeros(1, self.up.convtr.out_channels, self.up._kernel_size - self.up._stride)
        kv = torch.zeros(len(self.tr.layers), 2, 1, self.W, self.tr.layers[0].self_attn.num_heads,
                         self.tr.layers[0].self_attn.dim_per_head)
        return [up, kv] + [torch.zeros(s) for (_, s), keep in zip(self.state_shapes, self.kept) if keep]

    def forward(self, frame, pos, up_state, tr_kv, *conv_states):
        z = self.quant(frame.transpose(1, 2))                   # [1, 512, F]
        z, up_new = _convtr_step(self.up, z, up_state)          # [1, 512, 16F]
        x = z.transpose(1, 2)                                   # [1, 16F, 512]
        T = x.shape[1]
        W = self.W
        p = pos.reshape(())
        # key j of [past window (W) + new (T)] sits at absolute position p - W + j
        pos_k = torch.arange(W + T) + p - W
        pos_q = torch.arange(T) + p
        delta = pos_q[:, None] - pos_k[None, :]
        mask = ((delta >= 0) & (delta < self.context) & (pos_k[None, :] >= 0))[None, None]
        kvs = []
        for i, layer in enumerate(self.tr.layers):
            x, pres = attn_layer(layer, x, tr_kv[i], p, mask, self.max_period, keep=W)
            kvs.append(pres)
        z = x.transpose(1, 2)
        new_states = []
        it = iter(conv_states)
        kept = iter(self.kept)

        def state(channels_like: torch.Tensor):
            if next(kept):
                return next(it), True
            return channels_like[..., :0], False   # no history to keep

        for layer in self.dec.model:
            if isinstance(layer, StreamingConv1d):
                prev, keep = state(z)
                z, s = _conv_step(layer, z, prev)
                if keep:
                    new_states.append(s)
            elif isinstance(layer, StreamingConvTranspose1d):
                prev, keep = state(z)
                z, s = _convtr_step(layer, z, prev)
                if keep:
                    new_states.append(s)
            elif isinstance(layer, SEANetResnetBlock):
                v = z
                for sub in layer.block:
                    if isinstance(sub, StreamingConv1d):
                        prev, keep = state(v)
                        v, s = _conv_step(sub, v, prev)
                        if keep:
                            new_states.append(s)
                    else:
                        v = sub(v)
                z = z + v
            else:
                z = layer(z)
        return (z, up_new, torch.stack(kvs), *new_states)
