"""The TTS engines behind /v1/tts/*: Pocket's queue and cache, and the person's own voice server (Drama 3
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


def test_pcm16_strips_wav_header_and_sends_pocket_params(bridge, fakes):
    resp = sentence(bridge, text="Bonjour **toi** [lien](http://x.y).", voice="pocket:colibri", format="pcm16")
    assert fakes.pocket.inputs == [{"model": "pocket-tts", "input": "Bonjour toi lien.", "voice": "colibri",
                                    "response_format": "wav"}]
    assert resp.content == pcm_for("Bonjour toi lien.")
    assert (resp.headers["x-sample-rate"], resp.headers["x-channels"]) == ("24000", "1")
    assert resp.headers["content-type"] == "application/octet-stream"


def test_one_pocket_call_at_a_time(bridge, fakes):
    fakes.pocket.delay = 0.05
    texts = [f"Phrase numéro {word}." for word in ("un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit")]
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        results = list(pool.map(lambda t: sentence(bridge, text=t).content, texts))
    assert sorted(b["input"] for b in fakes.pocket.inputs) == sorted(texts)
    assert fakes.pocket.max_active == 1
    assert results == [pcm_for(t) for t in texts]


def test_lru_cache_hits_and_evicts(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["limits"].update(tts_cache_entries=2)).start()
    first = sentence(bridge, text="Salut.")
    again = sentence(bridge, text="Salut.")
    assert first.headers["x-cache"] == "miss" and again.headers["x-cache"] == "hit" and len(fakes.pocket.inputs) == 1
    sentence(bridge, text="Salut.", voice="pocket:galet")   # other voice = other key
    sentence(bridge, text="Autre.")                         # evicts ("pocket:ours", "Salut.")
    assert sentence(bridge, text="Salut.").headers["x-cache"] == "miss"
    assert len(fakes.pocket.inputs) == 4
    other_format = sentence(bridge, text="Autre.", format="wav")
    assert other_format.headers["x-cache"] == "miss"   # the format is part of the key


def test_errors(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["limits"].update(tts_max_chars=20)).start()
    empty = sentence(bridge, text="---\n**")   # nothing to say (a code block is named since spoken.rs)
    assert empty.status_code == 400
    long = sentence(bridge, text="x " * 50)
    assert long.status_code == 413
    fakes.pocket.status = 500
    upstream = sentence(bridge, text="Ok.")
    assert upstream.status_code == 502 and upstream.json() == {"detail": "pocket returned HTTP 500"}
    down = bridges.new("down", change=lambda c: c["pocket"].update(url=f"http://127.0.0.1:{free_port()}")).start()
    unreachable = sentence(down, text="Ok.")
    assert unreachable.status_code == 502 and unreachable.json() == {"detail": "pocket unreachable"}
    assert down.get("/health").json()["pocket"] is False


def test_wav_float32_and_extensible_are_converted(bridge, fakes):
    floats = array.array("f", [0.0, 0.5, -1.0, 2.0])
    fakes.pocket.override = lambda seen: Response(make_wav(floats.tobytes(), bits=32, tag=3), media_type="audio/wav") \
        if seen.path == "/v1/audio/speech" else None
    resp = sentence(bridge, text="Flottant.")
    assert array.array("h", resp.content).tolist() == [0, 16383, -32767, 32767]
    raw = struct.pack("<hh", 1, -2)
    fakes.pocket.override = lambda seen: Response(make_wav(raw), media_type="audio/wav") \
        if seen.path == "/v1/audio/speech" else None
    assert sentence(bridge, text="Entier.").content == raw


def test_an_old_kyutai_voice_speaks_with_the_default_bip(bridges, fakes):
    """Kyutai 1.6B is gone (a Kyutai server is a voice server like any other): an old `[kyutai]` section is ignored,
    an old Kyutai voice speaks with the default Bip."""
    def change(c):
        c["kyutai"] = {"url": "http://127.0.0.1:8097", "default_voice": "5476"}
        c["pocket"] = {"url": fakes.pocket.url, "default_voice": "pocket:loutre"}
        c["agents"]["Wellness"].pop("voice")
    bridge = bridges.new(change=change).start()
    sentence(bridge, text="Bonjour.", voice="5476")
    assert fakes.pocket.inputs[-1]["voice"] == "loutre"
    # an agent nobody picked a voice for gets its category's Bip in the language it writes in (place-locale, 2026-10-30),
    # not the install's default Bip
    assert bridge.get("/v1/agents", headers=AUTH).json()["agents"][0]["voice"] == "pocket:en/ours"
    fakes.pocket.status = 503
    assert sentence(bridge, text="Encore.", voice="pocket:loutre").status_code >= 500
    health = bridge.get("/health").json()
    assert "kyutai" not in health and health["pocket"] is False
    assert "  voice       pocket:loutre (default)" in bridge.cli("check").stdout


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
    assert health["voice_server"] is True and "drama" not in health


def test_drama_down_speaks_with_a_pocket_bip_of_the_same_language(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(voice_server={"url": f"http://127.0.0.1:{free_port()}",
                                                                 "model": "drama-3", "stream_rate": 44100})).start()
    out = stream(bridge, text="Bonjour.", voice="drama:fr-mascotte-s17")
    assert fakes.pocket.inputs[-1]["voice"] == "ours" and out      # the mascot's stand-in is Ours (2026-10-30)
    audio = sentence(bridge, text="Hello there.", voice="drama:en-naturelle-s47")
    assert fakes.pocket.inputs[-1]["voice"] == "en/galet" and audio.content
    health = bridge.get("/health").json()
    assert health["voice_server"] is False


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
    assert fakes.pocket.inputs[-1]["voice"] == "galet" and mp3.content.startswith(b"ID3")   # a Bip reads it


def test_drama_voices_without_drama_go_to_pocket(bridge, fakes):
    sentence(bridge, text="Salut.", voice="drama:fr-mascotte-s28")
    assert fakes.pocket.inputs[-1]["voice"] == "ours"              # the mascot's stand-in is Ours (2026-10-30)


def test_a_server_that_does_not_stream_gives_whole_wav_files_in_its_language(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(voice_server={"url": fakes.voices.url, "model": "kokoro"})).start()
    fakes.voices.mode = "wav24k"
    out = stream(bridge, text="Hello, it is 7:50.", voice="server:en/af_bella")
    seen = fakes.voices.inputs[0]
    assert seen["model"] == "kokoro" and seen["voice"] == "af_bella" and seen["response_format"] == "wav"
    assert "seven" in seen["input"]                   # English numbers: the voice's language
    assert abs(len(out) // 2 - 12000) <= 50


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
