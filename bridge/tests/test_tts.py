import array
import asyncio
import json
import struct

import httpx
import pytest

from bipbridge.config import VoiceServerConfig
from bipbridge.tts import BACKGROUND, INTERACTIVE, FifoScheduler, TtsError, TtsService, wav_to_pcm16
from conftest import KYUTAI, FakeBackend, make_wav, pcm_for


def service(backend: FakeBackend, **kw) -> TtsService:
    client = httpx.AsyncClient(transport=backend.transport())
    return TtsService(client, KYUTAI, "5476", **kw)


async def test_pcm16_strips_wav_header_and_sends_kyutai_params():
    backend = FakeBackend()
    tts = service(backend)
    audio = await tts.synthesize("Bonjour **toi** [lien](http://x.y).", "4193", "pcm16")
    assert backend.kyutai_inputs == [{"model": "kyutai-tts", "input": "Bonjour toi lien.", "voice": "4193",
                                      "response_format": "wav"}]
    assert audio.data == pcm_for("Bonjour toi lien.")
    assert (audio.sample_rate, audio.channels, audio.content_type) == (24000, 1, "application/octet-stream")


async def test_fifo_order_and_no_parallel_calls():
    backend = FakeBackend()
    backend.kyutai_delay = 0.02
    tts = service(backend)
    texts = [f"Phrase numéro {word}." for word in ("un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit")]
    tasks = []
    for text in texts:
        tasks.append(asyncio.create_task(tts.synthesize(text, None, "pcm16")))
        await asyncio.sleep(0)  # arrival order = creation order
    results = await asyncio.gather(*tasks)
    assert [b["input"] for b in backend.kyutai_inputs] == texts
    assert backend.kyutai_max_active == 1
    assert [r.data for r in results] == [pcm_for(t) for t in texts]


async def test_interactive_overtakes_waiting_background_but_not_running():
    backend = FakeBackend()
    backend.kyutai_delay = 0.03
    tts = service(backend)
    first = asyncio.create_task(tts.synthesize("Arrière plan un.", None, "mp3", BACKGROUND))
    await asyncio.sleep(0.005)
    second = asyncio.create_task(tts.synthesize("Arrière plan deux.", None, "mp3", BACKGROUND))
    await asyncio.sleep(0)
    third = asyncio.create_task(tts.synthesize("Voix directe.", None, "pcm16", INTERACTIVE))
    await asyncio.gather(first, second, third)
    assert [b["input"] for b in backend.kyutai_inputs] == ["Arrière plan un.", "Voix directe.", "Arrière plan deux."]


async def test_cancelled_waiter_leaves_queue():
    backend = FakeBackend()
    backend.kyutai_delay = 0.03
    tts = service(backend)
    a = asyncio.create_task(tts.synthesize("Un.", None, "pcm16"))
    await asyncio.sleep(0.005)
    b = asyncio.create_task(tts.synthesize("Deux.", None, "pcm16"))
    c = asyncio.create_task(tts.synthesize("Trois.", None, "pcm16"))
    await asyncio.sleep(0)
    b.cancel()
    await asyncio.gather(a, c)
    with pytest.raises(asyncio.CancelledError):
        await b
    assert [x["input"] for x in backend.kyutai_inputs] == ["Un.", "Trois."]
    assert tts.scheduler.depth == 0


async def test_lru_cache_hits_and_evicts():
    backend = FakeBackend()
    tts = service(backend, cache_entries=2)
    first = await tts.synthesize("Salut.", None, "pcm16")
    again = await tts.synthesize("Salut.", None, "pcm16")
    assert not first.cached and again.cached and len(backend.kyutai_inputs) == 1
    await tts.synthesize("Salut.", "4193", "pcm16")  # other voice = other key
    await tts.synthesize("Autre.", None, "pcm16")    # evicts ("5476", "Salut.")
    await tts.synthesize("Salut.", None, "pcm16")
    assert len(backend.kyutai_inputs) == 4


async def test_errors():
    backend = FakeBackend()
    tts = service(backend, max_chars=20)
    with pytest.raises(TtsError) as empty:
        await tts.synthesize("```\ncode\n```", None, "pcm16")
    assert empty.value.status == 400
    with pytest.raises(TtsError) as long:
        await tts.synthesize("x " * 50, None, "pcm16")
    assert long.value.status == 413
    backend.kyutai_status = 500
    with pytest.raises(TtsError) as upstream:
        await tts.synthesize("Ok.", None, "pcm16")
    assert upstream.value.status == 502


def test_wav_float32_and_extensible_are_converted():
    floats = array.array("f", [0.0, 0.5, -1.0, 2.0])
    pcm, rate, channels = wav_to_pcm16(make_wav(floats.tobytes(), bits=32, tag=3))
    assert array.array("h", pcm).tolist() == [0, 16383, -32767, 32767]
    assert (rate, channels) == (24000, 1)
    raw = struct.pack("<hh", 1, -2)
    assert wav_to_pcm16(make_wav(raw))[0] == raw


async def test_scheduler_reentrancy_after_release():
    sched = FifoScheduler()
    async with sched.slot():
        assert sched.depth == 1
    async with sched.slot(BACKGROUND):
        pass
    assert sched.depth == 0


async def test_pocket_only_routes_everything_to_pocket_and_never_to_kyutai():
    from conftest import POCKET
    backend = FakeBackend()
    tts = TtsService(httpx.AsyncClient(transport=backend.transport()), None, "pocket:loutre", pocket_url=POCKET)
    await tts.synthesize("Bonjour.", "5476", "pcm16")  # a Kyutai voice: the default Bip speaks instead
    assert backend.pocket_inputs[-1]["voice"] == "loutre" and backend.kyutai_inputs == []
    backend.pocket_status = 503
    with pytest.raises(TtsError):
        await tts.synthesize("Encore.", "pocket:loutre", "pcm16")
    assert backend.kyutai_inputs == []
    assert await tts.reachable() is None


# -- The person's voice server (here Drama 3 on audio.cpp, as an earlier install set it up) ------------------------

DRAMA = "http://drama.test"


def drama_service(backend: FakeBackend, drama_handler, **kw) -> TtsService:
    async def handle(request: httpx.Request) -> httpx.Response:
        if request.url.host == "drama.test":
            return await drama_handler(request)
        return await backend.handle(request)
    from conftest import POCKET
    client = httpx.AsyncClient(transport=httpx.MockTransport(handle))
    server = kw.pop("server", None) or VoiceServerConfig(url=DRAMA, model="drama-3", stream_rate=44100)
    return TtsService(client, KYUTAI, "5476", pocket_url=POCKET, voice_server=server, **kw)


def tone(seconds: float, rate: int = 44100) -> bytes:
    import math
    n = int(seconds * rate)
    return array.array("h", (int(8000 * math.sin(2 * math.pi * 440 * i / rate)) for i in range(n))).tobytes()


async def test_drama_streams_its_44k_audio_resampled_to_24k():
    seen = []

    async def drama(request: httpx.Request) -> httpx.Response:
        seen.append(json.loads(request.content))
        audio = tone(1.0)

        async def chunks():
            for i in range(0, len(audio), 4411):   # odd sizes: chunks need not be sample-aligned
                yield audio[i:i + 4411]

        return httpx.Response(200, content=chunks())

    tts = drama_service(FakeBackend(), drama)
    out = b"".join([c async for c in tts.stream("Bonjour, il est sept heures cinquante.", "drama:fr-mascotte-s17")])
    assert seen == [{"model": "drama-3", "input": "Bonjour, il est sept heures cinquante.", "voice": "fr-mascotte-s17",
                     "response_format": "pcm", "stream_format": "audio"}]
    assert abs(len(out) // 2 - 24000) <= 50          # one second at 24 kHz
    again = b"".join([c async for c in tts.stream("Bonjour, il est sept heures cinquante.", "drama:fr-mascotte-s17")])
    assert again == out and len(seen) == 1           # a complete take is cached


async def test_drama_down_speaks_with_a_pocket_bip_of_the_same_language():
    async def drama(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    backend = FakeBackend()
    tts = drama_service(backend, drama)
    out = b"".join([c async for c in tts.stream("Bonjour.", "drama:fr-mascotte-s17")])
    assert backend.pocket_inputs[-1]["voice"] == "colibri" and out
    audio = await tts.synthesize("Hello there.", "drama:en-naturelle-s47", "pcm16")
    assert backend.pocket_inputs[-1]["voice"] == "en/galet" and audio.data


async def test_a_drama_take_reading_on_and_on_is_cut():
    async def drama(request: httpx.Request) -> httpx.Response:
        async def chunks():
            for _ in range(40):                      # 40 s of audio for a 4-character text
                yield tone(1.0)
        return httpx.Response(200, content=chunks())

    tts = drama_service(FakeBackend(), drama)
    out = b"".join([c async for c in tts.stream("Oui.", "drama:fr-naturelle-s15")])
    assert len(out) / 2 / 24000 < 6


async def test_drama_whole_files_and_formats_it_cannot_make():
    async def drama(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        if body["response_format"] == "mp3":
            return httpx.Response(400)                # audio.cpp makes no mp3
        assert body["response_format"] == "wav" and "stream_format" not in body
        pcm = tone(0.5)
        header = (b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt " +
                  struct.pack("<IHHIIHH", 16, 1, 1, 44100, 88200, 2, 16) + b"data" + struct.pack("<I", len(pcm)))
        return httpx.Response(200, content=header + pcm)

    backend = FakeBackend()
    tts = drama_service(backend, drama)
    audio = await tts.synthesize("Bonjour à toi.", "drama:fr-naturelle-s21", "pcm16")
    assert audio.sample_rate == 24000 and abs(len(audio.data) // 2 - 12000) <= 50
    wav = await tts.synthesize("Bonjour à toi.", "drama:fr-naturelle-s21", "wav")
    assert wav.data[:4] == b"RIFF" and wav_to_pcm16(wav.data)[1] == 24000
    mp3 = await tts.synthesize("Bonjour à toi.", "drama:fr-naturelle-s21", "mp3")   # outbox audio: a Bip reads it
    assert backend.pocket_inputs[-1]["voice"] == "galet" and mp3.data.startswith(b"ID3pocket")


async def test_drama_voices_without_drama_go_to_pocket():
    backend = FakeBackend()
    from conftest import POCKET
    tts = TtsService(httpx.AsyncClient(transport=backend.transport()), KYUTAI, "5476", pocket_url=POCKET)
    await tts.synthesize("Salut.", "drama:fr-mascotte-s28", "pcm16")
    assert backend.pocket_inputs[-1]["voice"] == "colibri"


def wav(pcm: bytes, rate: int, channels: int = 1) -> bytes:
    return (b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt " +
            struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * channels * 2, channels * 2, 16) + b"data" +
            struct.pack("<I", len(pcm)) + pcm)


async def test_a_server_that_does_not_stream_gives_whole_wav_files_in_its_language():
    seen = []

    async def kokoro(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        seen.append(body)
        return httpx.Response(200, content=wav(tone(0.5, 24000), 24000))

    tts = drama_service(FakeBackend(), kokoro, server=VoiceServerConfig(url=DRAMA, model="kokoro"))
    out = b"".join([c async for c in tts.stream("Hello, it is 7:50.", "server:en/af_bella")])
    assert seen[0]["model"] == "kokoro" and seen[0]["voice"] == "af_bella" and seen[0]["response_format"] == "wav"
    assert "seven" in seen[0]["input"]                # English numbers: the voice's language
    assert abs(len(out) // 2 - 12000) <= 50


async def test_mp3_from_a_server_that_makes_it():
    async def server(request: httpx.Request) -> httpx.Response:
        assert json.loads(request.content)["response_format"] == "mp3"
        return httpx.Response(200, content=b"ID3server")

    tts = drama_service(FakeBackend(), server, server=VoiceServerConfig(url=DRAMA))
    audio = await tts.synthesize("Bonjour.", "server:fr/v1", "mp3")
    assert audio.data == b"ID3server" and audio.format == "mp3"


async def test_the_server_voices_in_its_order_from_whichever_route_it_has():
    async def audiocpp(request: httpx.Request) -> httpx.Response:
        if request.url.path == "/v1/audio/voices":
            return httpx.Response(200, json={"voices": ["fr-naturelle-s15", "fr-mascotte-s17"]})
        return httpx.Response(404)

    tts = drama_service(FakeBackend(), audiocpp)
    assert await tts.server_voices() == ["fr-naturelle-s15", "fr-mascotte-s17"]

    async def kyutai(request: httpx.Request) -> httpx.Response:
        if request.url.path == "/v1/voices":
            return httpx.Response(200, json={"default": "5476", "aliases": {"5476": "a.wav", "posee": "b.wav"}})
        return httpx.Response(404)

    tts = drama_service(FakeBackend(), kyutai, server=VoiceServerConfig(url=DRAMA))
    assert await tts.server_voices() == ["5476", "posee"]

    async def bare(request: httpx.Request) -> httpx.Response:
        return httpx.Response(404)

    tts = drama_service(FakeBackend(), bare, server=VoiceServerConfig(url=DRAMA, voices=["alba", "marius"]))
    assert await tts.server_voices() == ["alba", "marius"]


async def test_the_persons_own_respellings_win():
    backend = FakeBackend()
    from conftest import POCKET
    tts = TtsService(httpx.AsyncClient(transport=backend.transport()), KYUTAI, "5476", pocket_url=POCKET,
                     words={"fr": {"Garmin": "Garmine", "Oura": "Ou-ra"}})
    await tts.synthesize("Ta montre garmin et ta bague Oura.", "pocket:galet", "pcm16")
    assert backend.pocket_inputs[-1]["input"] == "Ta montre Garmine et ta bague Ou-ra."
    await tts.synthesize("Your Garmin.", "pocket:en/galet", "pcm16")
    assert backend.pocket_inputs[-1]["input"] == "Your Garmin."   # French respellings stay French
