"""The Bips' voices for the phones, from Pocket TTS 4.0.1 (Kyutai, CC-BY-4.0: converted to ONNX, the language model to
8 bits): one package per language and the audio decoder they share. Run with the export's venv (requirements.txt):

    voices/onnx/.venv/bin/python voices/onnx/build.py [--languages french,english] [--out build/voices]

    out/index.json                          every package: folder, bytes, sha256 of its manifest (the apps' list)
    out/mimi-<id>/mimi.onnx                 the audio decoder: a frame in, 1920 samples at 24 kHz out (fp32, static)
    out/<language>-<id>/
        flowlm.onnx, flowlm.onnx.data       the language model, one step (int8 per channel, its weights apart)
        text_table.f16                      the tokens' embeddings [4001, 1024], float16 little-endian
        input_linear.f32                    the previous latent into the model [1024, 32], float32
        bos.f32                             the first frame's latent [32]
        tokenizer.json                      the language's tokenizer (textprep.py)
        voices/<bip>.kv.f16                 each Bip: the model's cache after its voice clip [6, 2, 1, T, 16, 64]
        manifest.json                       the files (bytes, sha256), tensors, voices, text rules, generation, credit

The model's graph takes x [1, T, 1024], past_kv [6, 2, 1, P, 16, 64] and noise [1, 32] (normal, sd √temperature) and
gives latent [1, 32], eos_logit [1, 1], present_kv [6, 2, 1, P+T, 16, 64] and frame [1, 1, 32] (the latent
denormalized, which the decoder takes: so the decoder is the same for every language). reference.py speaks with a
package as the apps do; each build ends with a sample per language in out/samples/ to listen to.

The manifest's `engine` (« pocket-tts 4.0.1 ») is what the apps send the bridge (`engine` in /v1/tts/prepare): below 4
it respells brands and English words for the old voices, from 4 on the text goes as written. `package` counts the
packages' versions (1, without the key: Pocket 3.3.0; 2: Pocket 4.0.1, with `fade_in_samples`: Pocket 4 fades each
chunk's first 5 ms in, a fresh decoder's first samples click)."""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
import tempfile
import time
from pathlib import Path
from typing import Dict, List, Tuple

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))

import textprep  # noqa: E402

LANGUAGES = ("french", "english", "german", "spanish")
PACKAGE = 2           # 1: Pocket 3.3.0 (2026-10-20); 2: Pocket 4.0.1 (2026-10-25)
THREADS = 2            # a busy Mac: torch and the checks on two threads
FADE_IN_SAMPLES = 120  # pocket 4's linspace(0, 1, sample_rate // 200) on each chunk's first samples
BIPS = ("colibri", "galet", "lumen", "mousse", "ours")
CREDIT = ("Pocket TTS by Kyutai (https://github.com/kyutai-labs/pocket-tts), weights CC-BY-4.0 "
          "(https://creativecommons.org/licenses/by/4.0/): converted to ONNX, the language model quantized to 8 bits. "
          "The Bips' voices: synthetic, made with Qwen3-TTS (Apache-2.0), then cloned with Pocket TTS "
          "(kyutai/pocket-tts, CC-BY-4.0).")
SAMPLES = {
    "french": "Salut ! J'ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet après-midi.",
    "english": "Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon.",
    "german": "Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen Spaziergang.",
    "spanish": "¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde.",
}


def engine() -> str:
    from importlib.metadata import version
    return f"pocket-tts {version('pocket-tts')}"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def digest_of(state: Dict) -> str:
    digest = hashlib.sha256()
    for name in sorted(state):
        digest.update(name.encode())
        digest.update(state[name].detach().float().contiguous().numpy().tobytes())
    return digest.hexdigest()


def write_raw(path: Path, array, dtype: str) -> Dict:
    import numpy as np
    data = np.ascontiguousarray(array, dtype=np.dtype(dtype).newbyteorder("<"))
    path.parent.mkdir(parents=True, exist_ok=True)
    data.tofile(path)
    return {"dtype": {"<f2": "f16", "<f4": "f32"}[np.dtype(dtype).newbyteorder("<").str], "shape": list(data.shape)}


def export_flow(model, path: Path) -> None:
    import torch
    from torch.export import Dim
    from wrappers import FlowLMStep
    step = FlowLMStep(model.flow_lm).eval()
    x, past, noise = torch.randn(1, 5, 1024), torch.randn(6, 2, 1, 71, 16, 64), torch.randn(1, 32)
    torch.onnx.export(step, (x, past, noise), str(path), dynamo=True,
                      input_names=["x", "past_kv", "noise"],
                      output_names=["latent", "eos_logit", "present_kv", "frame"],
                      dynamic_shapes={"x": {1: Dim("T", min=1, max=512)}, "past_kv": {3: Dim("P", min=1, max=4096)},
                                      "noise": None},
                      external_data=False, optimize=True)


def quantize(source: Path, target: Path, scratch: Path) -> None:
    """int8 per channel on the matrix products (the spike's choice: 3 % mean error, inaudible), weights in a file of
    their own next to the graph (read in place by the runtime)."""
    import onnx
    from onnxruntime.quantization import QuantType, quantize_dynamic
    model = onnx.load(str(source))
    del model.graph.value_info[:]   # the exporter's value_info trips the quantizer's shape inference
    plain = scratch / "flow-novi.onnx"
    onnx.save(model, str(plain))
    quantized = scratch / "flow-int8.onnx"
    quantize_dynamic(str(plain), str(quantized), weight_type=QuantType.QInt8, per_channel=True,
                     op_types_to_quantize=["MatMul", "Gemm"], extra_options={"MatMulConstBOnly": True})
    onnx.save_model(onnx.load(str(quantized)), str(target), save_as_external_data=True, all_tensors_to_one_file=True,
                    location=target.name + ".data", size_threshold=1024)


def export_mimi(model, path: Path) -> List[str]:
    import torch
    from wrappers import MimiDecodeStep
    step = MimiDecodeStep(model).eval()
    states = step.init_states()
    names = ["up_state", "tr_kv"] + [f"conv_{i}" for i in range(len(states) - 2)]
    torch.onnx.export(step, (torch.randn(1, 1, 32), torch.tensor([0]), *states), str(path), dynamo=True,
                      input_names=["frame", "pos"] + names, output_names=["audio"] + [n + "_out" for n in names],
                      external_data=False, optimize=True)
    return [{"name": n, "shape": list(s.shape)} for n, s in zip(names, states)]


def check_flow(model, onnx_path: Path, tolerance: float) -> float:
    """The graph against pocket's own layers on a real step (the voice's cache, a sentence): the largest relative
    error of the latent."""
    import numpy as np
    import onnxruntime as ort
    import torch
    from wrappers import FlowLMStep
    step = FlowLMStep(model.flow_lm).eval()
    state = model.get_state_for_audio_prompt(str(REPO / "voices" / model_language(model) / "colibri.safetensors"))
    past = torch.stack([state[f"transformer.layers.{i}.self_attn"]["cache"] for i in range(6)])
    ids = model.flow_lm.conditioner.tokenizer.encode("Bonjour, ceci est un essai.")
    x = model.flow_lm.conditioner.embed(torch.tensor([ids]))
    noise = torch.randn(1, 32) * 0.3 ** 0.5
    with torch.no_grad():
        expected = step(x, past, noise)[0].numpy()
    options = ort.SessionOptions()
    options.intra_op_num_threads = THREADS
    session = ort.InferenceSession(str(onnx_path), options, providers=["CPUExecutionProvider"])
    got = session.run(None, {"x": x.detach().numpy(), "past_kv": past.numpy(), "noise": noise.numpy()})[0]
    error = float(np.abs(got - expected).max() / (np.abs(expected).max() + 1e-9))
    if error > tolerance:
        raise SystemExit(f"{onnx_path.name}: relative error {error:.4f} over {tolerance}")
    return error


def model_language(model) -> str:
    return model._bipagents_language


def kept_decoder(out: Path, weights: str):
    """A decoder package already in `out` with these same weights (made by an earlier build): kept as it is, so the
    phones and servers that have it don't fetch it again (Pocket 4.0.1's decoder is 3.3.0's)."""
    for manifest in sorted(out.glob("mimi-*/manifest.json")):
        try:
            data = json.loads(manifest.read_text(encoding="utf-8"))
        except ValueError:
            continue
        files = data.get("files", {})
        if data.get("weights_sha256") == weights and all((manifest.parent / f).is_file() for f in files):
            return manifest.parent
    return None


def build_language(language: str, out: Path, scratch: Path, mimi: Dict) -> Dict:
    import torch
    from pocket_tts import TTSModel
    started = time.time()
    model = TTSModel.load_model(language=language)
    model._bipagents_language = language
    config = model.config
    assert not model.pad_with_spaces_for_short_inputs and model.model_recommended_frames_after_eos is None
    assert (model.replace_characters or {}) == textprep.LANGUAGES[language]["replace"]
    assert model.remove_semicolons == textprep.LANGUAGES[language]["semicolons"]
    assert model.flow_lm.input_linear.bias is None
    folder = scratch / language
    folder.mkdir()
    # The audio decoder: exported once, shared when its weights are the same (they are for these four).
    decoder = {k: v for k, v in model.mimi.state_dict().items()
               if k.startswith(("quantizer.output_proj", "upsample", "decoder_transformer", "decoder."))}
    weights = digest_of(decoder)
    if not mimi and kept_decoder(out, weights):
        mimi.update({"weights": weights, "kept": kept_decoder(out, weights)})
    elif not mimi:
        mimi_dir = scratch / "mimi"
        mimi_dir.mkdir()
        names = export_mimi(model, mimi_dir / "mimi.onnx")
        mimi.update({"weights": weights, "dir": mimi_dir, "states": names})
    elif mimi["weights"] != weights:
        raise SystemExit(f"{language}: its audio decoder differs from the others' (to export apart)")
    # The language model: fp32, checked against pocket, then 8 bits, checked again.
    plain = scratch / f"{language}-flow-f32.onnx"
    export_flow(model, plain)
    error_f32 = check_flow(model, plain, 1e-3)
    quantize(plain, folder / "flowlm.onnx", scratch)
    error_int8 = check_flow(model, folder / "flowlm.onnx", 0.35)
    plain.unlink()
    fl = model.flow_lm
    tensors = {
        "text_table.f16": write_raw(folder / "text_table.f16", fl.conditioner.embed.weight.detach().numpy(), "<f2"),
        "input_linear.f32": write_raw(folder / "input_linear.f32", fl.input_linear.weight.detach().numpy(), "<f4"),
        "bos.f32": write_raw(folder / "bos.f32", fl.bos_emb.detach().numpy(), "<f4"),
    }
    shutil.copyfile(HERE / "tokenizers" / f"{language}.json", folder / "tokenizer.json")
    voices = {}
    for bip in BIPS:
        state = model.get_state_for_audio_prompt(str(REPO / "voices" / language / f"{bip}.safetensors"))
        layers = [state[f"transformer.layers.{i}.self_attn"] for i in range(len(fl.transformer.layers))]
        cache = torch.stack([layer["cache"] for layer in layers])
        frames = int(cache.shape[3])
        assert all(int(layer["offset"].reshape(-1)[0]) == frames and int(layer["pad"].reshape(-1)[0]) == 0
                   for layer in layers), f"{language}/{bip}: a cache with padding"
        spec = write_raw(folder / "voices" / f"{bip}.kv.f16", cache.numpy(), "<f2")
        voices[bip] = {"file": f"voices/{bip}.kv.f16", "frames": frames, **spec}
    files = {}
    for path in sorted(p for p in folder.rglob("*") if p.is_file()):
        files[path.relative_to(folder).as_posix()] = {"bytes": path.stat().st_size, "sha256": sha256(path)}
    manifest = {
        "format": 1,
        "package": PACKAGE,
        "language": language,
        "engine": engine(),
        "weights": config.weights_path_without_voice_cloning,
        "credit": CREDIT,
        "files": files,
        "tensors": tensors,
        "voices": voices,
        "graph": {"inputs": ["x", "past_kv", "noise"], "outputs": ["latent", "eos_logit", "present_kv", "frame"],
                  "layers": len(fl.transformer.layers), "heads": 16, "head_dim": 64, "latent": 32, "model": 1024},
        "text": {"replace": textprep.LANGUAGES[language]["replace"],
                 "semicolons": textprep.LANGUAGES[language]["semicolons"], "max_tokens": textprep.MAX_TOKENS},
        "generation": {"temperature": float(model.temp), "eos_threshold": float(model.eos_threshold),
                       "min_frames_before_eos": int(model._MIN_FRAMES_BEFORE_EOS),
                       "tokens_per_second": textprep.TOKENS_PER_SECOND, "padding_seconds": textprep.PADDING_SECONDS,
                       "frame_rate": float(config.mimi.frame_rate), "sample_rate": int(config.mimi.sample_rate),
                       "samples_per_frame": 1920, "mimi_steps_per_frame": 16,
                       "fade_in_samples": FADE_IN_SAMPLES},
        "check": {"relative_error_f32": round(error_f32, 6), "relative_error_int8": round(error_int8, 4)},
    }
    (folder / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{language}: built in {time.time() - started:.0f} s, error f32 {error_f32:.2e}, int8 {error_int8:.3f}, "
          f"{sum(f['bytes'] for f in files.values()) / 1e6:.1f} MB", flush=True)
    return manifest


def package_id(folder: Path) -> str:
    return sha256(folder / "manifest.json")[:12]


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--languages", default=",".join(LANGUAGES))
    parser.add_argument("--out", default=str(REPO / "build" / "voices"))
    parser.add_argument("--threads", type=int, default=THREADS, help="CPU threads for PyTorch and the checks")
    args = parser.parse_args(argv)
    import torch
    torch.set_num_threads(args.threads)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    index: Dict = {"format": 1, "package": PACKAGE, "engine": engine(), "languages": {}}
    mimi: Dict = {}
    with tempfile.TemporaryDirectory(dir=out) as tmp:
        scratch = Path(tmp)
        for language in args.languages.split(","):
            build_language(language, out, scratch, mimi)
            pid = package_id(scratch / language)
            target = out / f"{language}-{pid}"
            if target.exists():
                shutil.rmtree(target)
            shutil.move(str(scratch / language), target)
            index["languages"][language] = {"folder": target.name, "manifest_sha256": sha256(target / "manifest.json"),
                                            "bytes": sum(p.stat().st_size for p in target.rglob("*") if p.is_file())}
        if mimi.get("kept"):
            target = mimi["kept"]
            print(f"decoder: {target.name} kept (the same weights)", flush=True)
        else:
            target = save_decoder(mimi, out)
        index["mimi"] = {"folder": target.name, "manifest_sha256": sha256(target / "manifest.json"),
                         "bytes": sum(p.stat().st_size for p in target.rglob("*") if p.is_file())}
    (out / "index.json").write_text(json.dumps(index, indent=1) + "\n", encoding="utf-8")
    samples(out, index)
    return 0


def save_decoder(mimi: Dict, out: Path) -> Path:
    """The decoder exported by this build, its manifest written, in `out`."""
    mimi_dir = mimi["dir"]
    names = [state["name"] for state in mimi["states"]]
    manifest = {"format": 1, "engine": engine(), "credit": CREDIT, "weights_sha256": mimi["weights"],
                "inputs": ["frame", "pos"] + names, "outputs": ["audio"] + [n + "_out" for n in names],
                "states": mimi["states"],   # each state's name and shape (float32 zeros at the start of a chunk)
                "files": {"mimi.onnx": {"bytes": (mimi_dir / "mimi.onnx").stat().st_size,
                                        "sha256": sha256(mimi_dir / "mimi.onnx")}}}
    (mimi_dir / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n", encoding="utf-8")
    target = out / f"mimi-{package_id(mimi_dir)}"
    if target.exists():
        shutil.rmtree(target)
    shutil.move(str(mimi_dir), target)
    return target


def samples(out: Path, index: Dict) -> None:
    # A sample per language with the first Bip, spoken as the apps will: to listen to before shipping.
    from reference import Package, save_wav
    import numpy as np
    (out / "samples").mkdir(exist_ok=True)
    for language in index["languages"]:
        package = Package(out / index["languages"][language]["folder"], out / index["mimi"]["folder"])
        started = time.time()
        audio = np.concatenate(list(package.speak(SAMPLES[language], "colibri", seed=42)))
        spent = time.time() - started
        save_wav(out / "samples" / f"{language}-colibri.wav", audio)
        print(f"{language}: sample {len(audio) / 24000:.1f} s of audio in {spent:.1f} s "
              f"({len(audio) / 24000 / spent:.1f}x real time, one thread)", flush=True)


if __name__ == "__main__":
    sys.exit(main())
