"""Speaking with a voice package the way the apps do (their engines follow this loop): ONNX Runtime and numpy only.

    voices/onnx/.venv/bin/python voices/onnx/reference.py build/voices french colibri "Bonjour !" out.wav

For each chunk of the text (textprep.py): the voice's cache, one step of the language model over the chunk's tokens
(its noise drawn and unused, as pocket does), then frame after frame: the previous latent (BOS first) through
`input_linear`, one model step (a new latent, the end-of-speech logit, the cache one longer, the frame for the
decoder), the frame through the audio decoder (1920 samples at 24 kHz, 80 ms). The end: the logit above the
threshold after the first frames, then `frames_after_eos` more. Each chunk starts again from the voice's cache and a
fresh decoder."""
from __future__ import annotations

import json
import sys
import wave
from pathlib import Path
from typing import Dict, Iterator, List, Optional

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import textprep  # noqa: E402

DTYPES = {"f16": "<f2", "f32": "<f4"}


def raw(path: Path, spec: Dict) -> np.ndarray:
    return np.fromfile(path, dtype=DTYPES[spec["dtype"]]).reshape(spec["shape"])


class Package:
    """A language's package and the shared decoder, as an app loads them."""

    def __init__(self, folder: Path, mimi_folder: Path, threads: int = 1):
        import onnxruntime as ort
        self.folder = Path(folder)
        self.manifest = json.loads((self.folder / "manifest.json").read_text(encoding="utf-8"))
        options = ort.SessionOptions()
        options.intra_op_num_threads = threads
        options.inter_op_num_threads = 1
        self.flow = ort.InferenceSession(str(self.folder / "flowlm.onnx"), options, providers=["CPUExecutionProvider"])
        self.mimi = ort.InferenceSession(str(Path(mimi_folder) / "mimi.onnx"), options,
                                         providers=["CPUExecutionProvider"])
        tensors = self.manifest["tensors"]
        self.table = raw(self.folder / "text_table.f16", tensors["text_table.f16"])
        self.input_linear = raw(self.folder / "input_linear.f32", tensors["input_linear.f32"])
        self.bos = raw(self.folder / "bos.f32", tensors["bos.f32"])
        self.tokenizer = textprep.Tokenizer.load(self.folder / "tokenizer.json")
        self.mimi_inputs = [i for i in self.mimi.get_inputs()]

    def voice(self, name: str) -> np.ndarray:
        spec = self.manifest["voices"][name]
        return raw(self.folder / spec["file"], spec).astype(np.float32)

    def _mimi_start(self) -> List[np.ndarray]:
        return [np.zeros(i.shape, dtype=np.float32) for i in self.mimi_inputs[2:]]

    def speak(self, text: str, voice: str, seed: Optional[int] = None) -> Iterator[np.ndarray]:
        """The audio, 1920 samples at a time."""
        gen = self.manifest["generation"]
        rng = np.random.default_rng(seed)
        scale = float(np.sqrt(gen["temperature"]))
        cache = self.voice(voice)
        names = [i.name for i in self.mimi_inputs]
        for chunk in textprep.chunks(self.tokenizer, text, self.manifest["language"]):
            noise = lambda: (rng.standard_normal((1, 32)) * scale).astype(np.float32)  # noqa: E731
            x = self.table[np.asarray(chunk.ids)].astype(np.float32)[None]
            _, _, past, _ = self.flow.run(None, {"x": x, "past_kv": cache, "noise": noise()})
            states = self._mimi_start()
            previous = self.bos.astype(np.float32)
            ended = None
            for step in range(chunk.max_gen_len):
                x = (self.input_linear @ previous).astype(np.float32)[None, None]
                latent, eos, past, frame = self.flow.run(None, {"x": x, "past_kv": past, "noise": noise()})
                if ended is None and step >= gen["min_frames_before_eos"] and float(eos.reshape(-1)[0]) > gen["eos_threshold"]:
                    ended = step
                if ended is not None and step >= ended + chunk.frames_after_eos:
                    break
                feeds = {names[0]: frame.astype(np.float32), names[1]: np.array([step * gen["mimi_steps_per_frame"]],
                                                                              dtype=np.int64)}
                feeds.update(zip(names[2:], states))
                out = self.mimi.run(None, feeds)
                states = out[1:]
                yield out[0].reshape(-1)
                previous = latent[0]


def save_wav(path: Path, audio: np.ndarray, rate: int = 24000) -> None:
    pcm = (np.clip(audio, -1, 1) * 32767).astype("<i2")
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(rate)
        out.writeframes(pcm.tobytes())


def find(out: Path, language: str) -> Path:
    index = json.loads((out / "index.json").read_text(encoding="utf-8"))
    return out / index["languages"][language]["folder"]


def main(argv: List[str]) -> int:
    out, language, voice, text, wav = Path(argv[0]), argv[1], argv[2], argv[3], Path(argv[4])
    index = json.loads((out / "index.json").read_text(encoding="utf-8"))
    package = Package(find(out, language), out / index["mimi"]["folder"])
    save_wav(wav, np.concatenate(list(package.speak(text, voice, seed=42))))
    print(wav)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
