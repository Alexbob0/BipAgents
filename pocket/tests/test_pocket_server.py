"""The HTTP contract the bridge relies on, with a fake Pocket model (no torch, no download)."""
import io
import sys
import wave
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import pocket_server  # noqa: E402
from pocket_server import Engine, create_app  # noqa: E402


class FakeModel:
    sample_rate = 24000

    def __init__(self, code):
        self.code = code
        self.loaded = []

    def get_state_for_audio_prompt(self, path):
        self.loaded.append(Path(path).name)
        return f"{self.code}:{Path(path).stem}"


class FakeEngine(Engine):
    def stream(self, voice, text, stop, pace="read"):
        model, state = self.resolve(voice)
        for word in text.split():
            if stop.is_set():
                break
            yield f"[{state}:{word}]".encode()


@pytest.fixture
def setup(tmp_path):
    for folder, names in {"french": ["loutre", "ours"], "english": ["loutre"]}.items():
        (tmp_path / folder).mkdir()
        for name in names:
            (tmp_path / folder / f"{name}.safetensors").write_bytes(b"x")
    engine = FakeEngine(tmp_path, models={"fr": FakeModel("fr"), "en": FakeModel("en")})
    return engine, TestClient(create_app(engine))


def test_health_lists_languages_and_voices(setup):
    _, client = setup
    assert client.get("/health").json() == {"status": "ok", "languages": ["fr", "en"],
                                           "voices": {"fr": ["loutre", "ours"], "en": ["loutre"]}}


def test_voice_names_pick_the_language(setup):
    engine, client = setup
    assert client.post("/v1/audio/stream", json={"input": "Salut toi", "voice": "loutre"}).content == b"[fr:loutre:Salut][fr:loutre:toi]"
    assert client.post("/v1/audio/stream", json={"input": "Hi", "voice": "en/loutre"}).content == b"[en:loutre:Hi]"
    engine.resolve("loutre")
    assert engine.models["fr"].loaded == ["loutre.safetensors"]  # voice states are loaded once


@pytest.mark.parametrize("voice", ["en/ours", "es/loutre", "../secrets", "fr/LOUTRE", "pirate"])
def test_unknown_voices_are_404(setup, voice):
    _, client = setup
    assert client.post("/v1/audio/stream", json={"input": "Hi", "voice": voice}).status_code == 404
    assert client.post("/v1/audio/speech", json={"input": "Hi", "voice": voice}).status_code == 404


def test_speech_formats(setup, monkeypatch):
    _, client = setup
    resp = client.post("/v1/audio/speech", json={"input": "Bonjour", "voice": "ours", "response_format": "wav"})
    assert resp.status_code == 200 and resp.headers["content-type"] == "audio/wav"
    with wave.open(io.BytesIO(resp.content)) as wav:
        assert (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) == (1, 2, 24000)
        assert wav.readframes(100) == b"[fr:ours:Bonjour]"
    monkeypatch.setattr(pocket_server, "mp3_bytes", lambda pcm, rate: b"ID3" + pcm)
    mp3 = client.post("/v1/audio/speech", json={"input": "Bonjour", "voice": "ours", "response_format": "mp3"})
    assert mp3.headers["content-type"] == "audio/mpeg" and mp3.content.startswith(b"ID3")
    assert client.post("/v1/audio/speech", json={"input": "Bonjour", "voice": "ours", "response_format": "opus"}).status_code == 415
    assert client.post("/v1/audio/speech", json={"input": "  ", "voice": "ours"}).status_code == 400



def test_phrases_split_sentences_and_lines_and_join_short_ones():
    from pocket_server import phrases
    text = "Bonne question ! D'après ce que j'ai trouvé, c'est simple.\nLe train de neuf heures, le plus reposant.\nOui. D'accord, je le note pour demain."
    assert phrases(text) == [
        ("Bonne question ! D'après ce que j'ai trouvé, c'est simple.", "para"),  # 2 words: joined to the next
        ("Le train de neuf heures, le plus reposant.", "para"),
        ("Oui. D'accord, je le note pour demain.", "para"),
    ]
    assert phrases("Tu veux que je réserve le premier train ? Je peux aussi regarder les retours…") == [
        ("Tu veux que je réserve le premier train ?", "?"), ("Je peux aussi regarder les retours…", "para")]
    assert phrases("Prix : 3.5 euros pour le billet aller") == [("Prix : 3.5 euros pour le billet aller", "para")]


def test_shape_trims_silence_and_levels_each_sentence():
    np = pytest.importorskip("numpy")
    from pocket_server import shape
    rate = 1000  # 10-sample frames keep the arithmetic readable
    silence, voice = np.zeros(300, dtype=np.float32), np.full(400, 0.3, dtype=np.float32)
    chunks = [silence[:150], silence[150:], voice[:200], voice[200:], silence]
    out = np.concatenate(list(shape(chunks, rate, lead_pad=0.03, tail_pad=0.08, fade=0.02)))
    assert len(out) == 30 + 400 + 80  # margins before and after the voice, the rest of the silence dropped
    assert np.isclose(out[100], 0.3 * 0.7)  # loud sentence: gain floored at 0.7
    assert out[-1] == 0 and out[-25] == 0  # fade-out over silence: no click
    quiet = np.concatenate(list(shape([np.full(400, 0.02, dtype=np.float32)], rate)))
    assert np.isclose(quiet.max(), 0.02 * 1.6)  # quiet sentence: raised, but capped at 1.6


def test_shape_keeps_a_soft_ending():
    np = pytest.importorskip("numpy")
    from pocket_server import shape
    rate = 1000
    loud, soft = np.full(300, 0.1, dtype=np.float32), np.full(100, 0.008, dtype=np.float32)  # a fading last syllable
    out = np.concatenate(list(shape([loud, soft, np.zeros(300, dtype=np.float32)], rate, tail_pad=0.08)))
    assert len(out) >= 300 + 100  # the soft end (8 % of the sentence level) is kept, not cut


def test_phrased_puts_pauses_between_sentences_only():
    np = pytest.importorskip("numpy")
    from pocket_server import phrased
    rate = 1000
    generate = lambda sentence: [np.full(300, 0.075, dtype=np.float32)]
    audio = list(phrased("Une première phrase assez longue. Une deuxième phrase assez longue ?\nFin du paragraphe et du texte.",
                         generate, rate, pace="read", jitter=0))
    pauses = [len(a) for a in audio if len(a) and not a.any()]
    assert pauses == [320, 620, 320]  # after the ".", after the line, then the last sentence's own "."
    live = [len(a) for a in phrased("Une première phrase assez longue. Une deuxième phrase assez longue.", generate, rate,
                                    pace="live", jitter=0) if len(a) and not a.any()]
    assert live == [220, 220]


def test_trailing_off_sentences_keep_more_tail():
    np = pytest.importorskip("numpy")
    from pocket_server import phrased, trails_off
    assert trails_off("Tu as l'air en forme aujourd'hui…") and trails_off("Bon... on verra.") is False
    rate = 1000
    generate = lambda sentence: [np.full(300, 0.075, dtype=np.float32), np.zeros(400, dtype=np.float32)]
    plain = [a for a in phrased("Une phrase assez longue pour elle seule.", generate, rate, jitter=0)]
    trailing = [a for a in phrased("Une phrase assez longue pour elle seule…", generate, rate, jitter=0)]
    assert len(trailing[-2]) - len(plain[-2]) == 120  # 200 ms kept after the voice instead of 80
