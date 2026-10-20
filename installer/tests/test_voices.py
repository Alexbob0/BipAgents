"""The light voices (voices.py): the phones' packages downloaded and checked, or nothing when they can't be had."""
import hashlib
import json

import httpx

from bipinstall import hermes, voices


def packages_server(tamper=False):
    """index.json, a French package and the decoder, as voices/onnx/build.py publishes them."""
    files = {"mimi-1/mimi.onnx": b"decoder", "french-1/flowlm.onnx": b"model", "french-1/voices/colibri.kv.f16": b"voice"}

    def manifest(folder):
        return {"files": {path.split("/", 1)[1]: {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
                          for path, data in files.items() if path.startswith(folder + "/")}}

    index = {"languages": {"french": {"folder": "french-1", "bytes": 10}, "english": {"folder": "english-1", "bytes": 10}},
             "mimi": {"folder": "mimi-1", "bytes": 7}}

    def handle(request):
        path = request.url.path.removeprefix("/v1/")
        if path == "index.json":
            return httpx.Response(200, json=index)
        folder, _, name = path.partition("/")
        if name == "manifest.json":
            return httpx.Response(200, json=manifest(folder))
        data = files.get(path)
        if data is None:
            return httpx.Response(404)
        return httpx.Response(200, content=b"tampered" if tamper and name == "flowlm.onnx" else data)

    return httpx.MockTransport(handle)


def test_the_packages_are_downloaded_checked_and_listed(tmp_path):
    said = []
    target = voices.install(["fr", "xx"], tmp_path, hermes.Runner(dry=False), said.append,
                            base="https://voices.test/v1/", transport=packages_server())
    assert target == tmp_path / ".local/share/bipagents/voices"
    assert (target / "french-1/voices/colibri.kv.f16").read_bytes() == b"voice"
    assert (target / "mimi-1/manifest.json").is_file()
    index = json.loads((target / "index.json").read_text())
    assert list(index["languages"]) == ["french"] and index["mimi"]["folder"] == "mimi-1"   # only what was asked
    assert not said


def test_a_bad_file_or_no_server_falls_back_to_pockets_models(tmp_path):
    said = []
    assert voices.install(["fr"], tmp_path, hermes.Runner(dry=False), said.append, base="https://voices.test/v1/",
                          transport=packages_server(tamper=True)) is None
    assert "Pocket's own models instead" in said[-1]
    assert not (tmp_path / ".local/share/bipagents/voices/french-1").exists()   # a checksum failed: not kept
    down = httpx.MockTransport(lambda request: httpx.Response(503))
    assert voices.install(["fr"], tmp_path, hermes.Runner(dry=False), said.append, base="https://voices.test/v1/",
                          transport=down) is None


def test_an_update_keeps_the_light_voices_light(monkeypatch, tmp_path):
    from bipinstall import upgrade
    monkeypatch.setenv("HOME", str(tmp_path))
    units = tmp_path / ".config/systemd/user"
    units.mkdir(parents=True)
    unit = units / "bipagents-pocket.service"
    unit.write_text("ExecStart=/x/python /x/pocket/pocket_server.py --voices /x/voices --port 8098\n")
    assert not upgrade.pocket_light()
    unit.write_text("ExecStart=/x/python /x/pocket/pocket_server.py --onnx /x/voices --keep none --port 8098\n")
    assert upgrade.pocket_light()
