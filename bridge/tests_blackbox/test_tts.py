"""The TTS engines behind /v1/tts/*: Kyutai's queue and cache, Pocket alone, and the person's own voice server (Drama 3
on audio.cpp streaming 44.1 kHz, Kokoro's whole files, a busy GPU handing over to the agent's Bip) — tests/test_tts.py,
through the bridge's routes."""
import array
import concurrent.futures
import struct

from starlette.responses import Response

from bbkit.common import AUTH, make_wav, pcm_for
from bbkit.servers import free_port


def stream(bridge, **body):
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json=body)
    assert resp.status_code == 200, resp.text
    return resp.content


def sentence(bridge, **body):
    return bridge.post("/v1/tts/sentence", headers=AUTH, json=body)


def test_pcm16_strips_wav_header_and_sends_kyutai_params(bridge, fakes):
    resp = sentence(bridge, text="Bonjour **toi** [lien](http://x.y).", voice="4193", format="pcm16")
    assert fakes.kyutai.inputs == [{"model": "kyutai-tts", "input": "Bonjour toi lien.", "voice": "4193",
                                    "response_format": "wav"}]
    assert resp.content == pcm_for("Bonjour toi lien.")
    assert (resp.headers["x-sample-rate"], resp.headers["x-channels"]) == ("24000", "1")
    assert resp.headers["content-type"] == "application/octet-stream"


def test_one_kyutai_call_at_a_time(bridge, fakes):
    fakes.kyutai.delay = 0.05
    texts = [f"Phrase numéro {word}." for word in ("un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit")]
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        results = list(pool.map(lambda t: sentence(bridge, text=t).content, texts))
    assert sorted(b["input"] for b in fakes.kyutai.inputs) == sorted(texts)
    assert fakes.kyutai.max_active == 1
    assert results == [pcm_for(t) for t in texts]


def test_lru_cache_hits_and_evicts(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["limits"].update(tts_cache_entries=2)).start()
    first = sentence(bridge, text="Salut.")
    again = sentence(bridge, text="Salut.")
    assert first.headers["x-cache"] == "miss" and again.headers["x-cache"] == "hit" and len(fakes.kyutai.inputs) == 1
    sentence(bridge, text="Salut.", voice="4193")   # other voice = other key
    sentence(bridge, text="Autre.")                 # evicts ("5476", "Salut.")
    assert sentence(bridge, text="Salut.").headers["x-cache"] == "miss"
    assert len(fakes.kyutai.inputs) == 4
    other_format = sentence(bridge, text="Autre.", format="wav")
    assert other_format.headers["x-cache"] == "miss"   # the format is part of the key


def test_errors(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["limits"].update(tts_max_chars=20)).start()
    empty = sentence(bridge, text="```\ncode\n```")
    assert empty.status_code == 400
    long = sentence(bridge, text="x " * 50)
    assert long.status_code == 413
    fakes.kyutai.status = 500
    upstream = sentence(bridge, text="Ok.")
    assert upstream.status_code == 502 and upstream.json() == {"detail": "kyutai returned HTTP 500"}
    down = bridges.new("down", change=lambda c: c["kyutai"].update(url=f"http://127.0.0.1:{free_port()}")).start()
    unreachable = sentence(down, text="Ok.")
    assert unreachable.status_code == 502 and unreachable.json() == {"detail": "kyutai unreachable"}
    assert down.get("/health").json()["kyutai"] is False


def test_wav_float32_and_extensible_are_converted(bridge, fakes):
    floats = array.array("f", [0.0, 0.5, -1.0, 2.0])
    fakes.kyutai.override = lambda seen: Response(make_wav(floats.tobytes(), bits=32, tag=3), media_type="audio/wav") \
        if seen.path == "/v1/audio/speech" else None
    resp = sentence(bridge, text="Flottant.")
    assert array.array("h", resp.content).tolist() == [0, 16383, -32767, 32767]
    raw = struct.pack("<hh", 1, -2)
    fakes.kyutai.override = lambda seen: Response(make_wav(raw), media_type="audio/wav") \
        if seen.path == "/v1/audio/speech" else None
    assert sentence(bridge, text="Entier.").content == raw


def test_pocket_only_routes_everything_to_pocket_and_never_to_kyutai(bridges, fakes):
    def change(c):
        c["kyutai"] = {"enabled": False}
        c["pocket"] = {"url": fakes.pocket.url, "default_voice": "pocket:loutre"}
        c["agents"]["Wellness"].pop("voice")
    bridge = bridges.new(change=change).start()
    sentence(bridge, text="Bonjour.", voice="5476")  # a Kyutai voice: the default Bip speaks instead
    assert fakes.pocket.inputs[-1]["voice"] == "loutre" and fakes.kyutai.inputs == []
    assert bridge.get("/v1/agents", headers=AUTH).json()["agents"][0]["voice"] == "pocket:loutre"
    fakes.pocket.status = 503
    assert sentence(bridge, text="Encore.", voice="pocket:loutre").status_code >= 500
    assert fakes.kyutai.inputs == []
    health = bridge.get("/health").json()
    assert health["kyutai"] is None and health["pocket"] is False
    out = bridge.cli("check")
    assert "  kyutai      off" in out.stdout.splitlines() and "  voice       pocket:loutre (default)" in out.stdout


def test_no_tts_engine_is_a_config_error(bridges):
    bridge = bridges.new(change=lambda c: (c.update(kyutai={"url": ""}), c.pop("pocket")))
    bridge.write_config()
    out = bridge.cli("check")
    assert out.returncode == 2 and out.stderr.strip() == "config error: no TTS engine: set [pocket] url, or [kyutai] url"
    bridge.start(wait=False)
    assert bridge.process.wait(timeout=30) == 2


def test_the_persons_own_respellings_win(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(pronunciations={"fr": {"Garmin": "Garmine", "Oura": "Ou-ra"}})).start()
    sentence(bridge, text="Ta montre garmin et ta bague Oura.", voice="pocket:galet")
    assert fakes.pocket.inputs[-1]["input"] == "Ta montre Garmine et ta bague Ou-ra."
    sentence(bridge, text="Your Garmin.", voice="pocket:en/galet")
    assert fakes.pocket.inputs[-1]["input"] == "Your Garmin."   # French respellings stay French


# -- the person's voice server -----------------------------------------------------------------------------------------

def drama(bridges, fakes, **server):
    table = {"url": fakes.voices.url, "model": "drama-3", "stream_rate": 44100, **server}
    return bridges.new(change=lambda c: c.update(voice_server=table)).start()


def test_drama_streams_its_44k_audio_resampled_to_24k(bridges, fakes):
    bridge = drama(bridges, fakes)
    out = stream(bridge, text="Bonjour, il est sept heures cinquante.", voice="drama:fr-mascotte-s17")
    assert fakes.voices.inputs == [{"model": "drama-3", "input": "Bonjour, il est sept heures cinquante.",
                                    "voice": "fr-mascotte-s17", "response_format": "pcm", "stream_format": "audio"}]
    assert abs(len(out) // 2 - 24000) <= 50          # one second at 24 kHz
    again = stream(bridge, text="Bonjour, il est sept heures cinquante.", voice="drama:fr-mascotte-s17")
    assert again == out and len(fakes.voices.inputs) == 1           # a complete take is cached
    health = bridge.get("/health").json()
    assert health["voice_server"] is True and health["drama"] is True


def test_an_earlier_installs_drama_section_is_a_voice_server(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(drama={"url": fakes.voices.url})).start()
    stream(bridge, text="Bonjour.", voice="drama:fr-mascotte-s17")
    assert fakes.voices.inputs[0]["model"] == "drama-3" and fakes.voices.inputs[0]["response_format"] == "pcm"
    assert bridge.get("/health").json()["drama"] is True


def test_drama_down_speaks_with_a_pocket_bip_of_the_same_language(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(voice_server={"url": f"http://127.0.0.1:{free_port()}",
                                                                 "model": "drama-3", "stream_rate": 44100})).start()
    out = stream(bridge, text="Bonjour.", voice="drama:fr-mascotte-s17")
    assert fakes.pocket.inputs[-1]["voice"] == "colibri" and out
    audio = sentence(bridge, text="Hello there.", voice="drama:en-naturelle-s47")
    assert fakes.pocket.inputs[-1]["voice"] == "en/galet" and audio.content
    health = bridge.get("/health").json()
    assert health["voice_server"] is False and health["drama"] is False


def test_a_drama_take_reading_on_and_on_is_cut(bridges, fakes):
    bridge = drama(bridges, fakes)
    fakes.voices.mode = "endless"
    out = stream(bridge, text="Oui.", voice="drama:fr-naturelle-s15")
    assert len(out) / 2 / 24000 < 6


def test_drama_whole_files_and_formats_it_cannot_make(bridges, fakes):
    bridge = drama(bridges, fakes)
    fakes.voices.mode = "wav44k"
    audio = sentence(bridge, text="Bonjour à toi.", voice="drama:fr-naturelle-s21")
    assert audio.headers["x-sample-rate"] == "24000" and abs(len(audio.content) // 2 - 12000) <= 50
    assert fakes.voices.inputs[-1]["response_format"] == "wav" and "stream_format" not in fakes.voices.inputs[-1]
    wav = sentence(bridge, text="Bonjour à toi.", voice="drama:fr-naturelle-s21", format="wav")
    assert wav.content[:4] == b"RIFF" and struct.unpack("<I", wav.content[24:28])[0] == 24000
    mp3 = bridge.post("/v1/tts/message", headers=AUTH, json={"text": "Bonjour à toi.", "voice": "drama:fr-naturelle-s21"})
    assert fakes.pocket.inputs[-1]["voice"] == "galet" and mp3.content.startswith(b"ID3pocket")   # a Bip reads it


def test_drama_voices_without_drama_go_to_pocket(bridge, fakes):
    sentence(bridge, text="Salut.", voice="drama:fr-mascotte-s28")
    assert fakes.pocket.inputs[-1]["voice"] == "colibri"


def test_a_server_that_does_not_stream_gives_whole_wav_files_in_its_language(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(voice_server={"url": fakes.voices.url, "model": "kokoro"})).start()
    fakes.voices.mode = "wav24k"
    out = stream(bridge, text="Hello, it is 7:50.", voice="server:en/af_bella")
    seen = fakes.voices.inputs[0]
    assert seen["model"] == "kokoro" and seen["voice"] == "af_bella" and seen["response_format"] == "wav"
    assert "seven" in seen["input"]                   # English numbers: the voice's language
    assert abs(len(out) // 2 - 12000) <= 50
    assert bridge.get("/health").json()["drama"] is None


def test_mp3_from_a_server_that_makes_it(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(voice_server={"url": fakes.voices.url})).start()
    fakes.voices.mode = "mp3"
    audio = bridge.post("/v1/tts/message", headers=AUTH, json={"text": "Bonjour.", "voice": "server:fr/v1"})
    assert audio.content == b"ID3server" and audio.headers["content-type"] == "audio/mpeg"
    assert fakes.voices.inputs[-1]["model"] == "tts-1" and fakes.voices.inputs[-1]["response_format"] == "mp3"


def test_a_voice_server_too_slow_for_a_live_sentence_hands_over_to_the_agents_bip(bridges, fakes):
    def change(c):
        c["voice_server"] = {"url": fakes.voices.url, "first_audio_seconds": 0.2}
        c["agents"]["Wellness"]["voice"] = "pocket:mousse"
    bridge = bridges.new(change=change).start()
    fakes.voices.mode = "busy"                       # a GPU busy with something else
    out = stream(bridge, text="Bonjour.", voice="server:fr/v1", agent="wellness")
    assert out and fakes.pocket.inputs[-1]["voice"] == "mousse"
    # For a while, the next sentences go to the Bip at once: one voice through the Live.
    audio = sentence(bridge, text="Ça va ?", voice="server:fr/v1", agent="wellness")
    assert audio.content and len(fakes.voices.inputs) == 1 and fakes.pocket.inputs[-1]["voice"] == "mousse"


def test_queued_live_sentences_do_not_count_the_queue_against_the_server(bridges, fakes):
    def change(c):
        c["voice_server"] = {"url": fakes.voices.url, "first_audio_seconds": 0.25}
        c["agents"]["Wellness"]["voice"] = "pocket:mousse"
    bridge = bridges.new(change=change).start()
    fakes.voices.mode = "steady"                     # each sentence well within its deadline once sent
    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        results = list(pool.map(lambda n: sentence(bridge, text=f"Phrase {n}.", voice="server:fr/v1", agent="wellness"),
                                range(4)))
    assert all(r.status_code == 200 for r in results)
    assert fakes.pocket.inputs == [] and len(fakes.voices.inputs) == 4
