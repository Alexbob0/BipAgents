import array
import asyncio
import struct

import httpx
import pytest

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
    texts = [f"Phrase numéro {i}." for i in range(8)]
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
