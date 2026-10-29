"""« Partager en lecture » (read_shares.rs): the owner's bridge keeps a conversation's encrypted copy at the guichet.
A fake guichet stands in for Kerfy's; the test plays the viewer's phone with its own P-256 key and opens what the
bridge sealed with the apps' formats (internal/read-only-shares.md): only the invited key gets the content key, the
items carry the person's messages and the agent's replies only, and a removed viewer means a new key."""
import base64
import hashlib
import hmac
import json
import threading
import time

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from starlette.responses import JSONResponse

from bbkit.common import AUTH
from bbkit.fakes import Fake


class FakeGuichet(Fake):
    name = "guichet"

    def reset(self) -> None:
        super().reset()
        self.epoch, self.ends = 1, time.time() + 30 * 86400
        self.viewers, self.invites, self.items, self.notified = [], {}, {}, []
        self.actions, self.seq = [], 0   # friends' taps waiting for the bridge (friends_cards.rs)
        self.gone = False

    async def handle(self, seen):
        parts = seen.path.strip("/").split("/")          # v1 rshares <id> [rest…]
        if seen.headers.get("authorization") != "Bearer writer-token-0123456789":
            return JSONResponse({"detail": "unknown token"}, status_code=401)
        if self.gone:
            return JSONResponse({"detail": "no such share"}, status_code=404)
        body = json.loads(seen.body or b"{}")
        rest = parts[3:]
        with self.lock:
            if seen.method == "GET" and not rest:
                return JSONResponse({"share": parts[2], "epoch": self.epoch, "ends": self.ends, "bytes": 0,
                                     "viewers": [dict(v, keyed=v.get("wrapped_epoch") == self.epoch) for v in self.viewers],
                                     "invites": list(self.invites.values())})
            if seen.method == "POST" and rest == ["invites"]:
                invite = f"invite{len(self.invites):02d}abcdefghijklmn"
                h = hashlib.sha256(invite.encode()).hexdigest()
                self.invites[h] = {"invite": h, "name": body["name"], "expires": time.time() + 7 * 86400, "used": None,
                                   "from": body["from"]}
                return JSONResponse({"invite": invite, "hash": h, "expires": self.invites[h]["expires"]})
            if seen.method == "PUT" and rest == ["keys"]:
                if body["epoch"] != self.epoch:
                    return JSONResponse({"detail": "rekey"}, status_code=409)
                for k in body["keys"]:
                    for v in self.viewers:
                        if v["viewer"] == k["viewer"]:
                            v["wrapped"], v["wrapped_epoch"] = k["wrapped"], self.epoch
                return JSONResponse({"ok": True})
            if seen.method == "POST" and rest == ["items"]:
                if body["epoch"] != self.epoch:
                    return JSONResponse({"detail": "rekey"}, status_code=409)
                for item in body["items"]:
                    self.items[item["id"]] = item
                self.notified.append(body.get("notify"))
                return JSONResponse({"last": len(self.items)})
            if seen.method == "DELETE" and rest[:1] == ["viewers"]:
                self.viewers = [v for v in self.viewers if v["viewer"] != rest[1]]
                self.epoch += 1
                self.items, self.actions = {}, []
                for v in self.viewers:
                    v.pop("wrapped", None)
                    v.pop("wrapped_epoch", None)
                return JSONResponse({"epoch": self.epoch})
            if seen.method == "GET" and rest == ["actions"]:
                return JSONResponse({"actions": list(self.actions)})
            if seen.method == "POST" and rest == ["actions", "taken"]:
                before = len(self.actions)
                self.actions = [a for a in self.actions if a["seq"] > body["through"]]
                return JSONResponse({"taken": before - len(self.actions)})
            if seen.method == "POST" and rest == ["ends"]:
                self.ends = body["ends"]
                return JSONResponse({"ends": self.ends})
            if seen.method == "DELETE" and not rest:
                self.gone = True
                return JSONResponse({"ok": True})
        return JSONResponse({"detail": "no route"}, status_code=404)

    def act(self, viewer: str, data: str, tag: str) -> None:
        """A viewer's device posted a sealed tap (the guichet's `POST /v1/rshares/{id}/actions`)."""
        with self.lock:
            self.seq += 1
            self.actions.append({"seq": self.seq, "viewer": viewer, "data": data, "tag": tag, "at": time.time()})

    def redeem(self, invite: str, public_key: bytes, proof: str, viewer: str) -> None:
        with self.lock:
            h = hashlib.sha256(invite.encode()).hexdigest()
            self.invites[h]["used"] = time.time()
            self.viewers.append({"viewer": viewer, "invite": h, "name": self.invites[h]["name"],
                                 "public_key": base64.b64encode(public_key).decode(), "proof": proof,
                                 "joined": time.time(), "muted": False})


class Phone:
    """A viewer's device: its key for this share, the link's secret."""

    def __init__(self, link: str):
        fragment = link.split("#", 1)[1]
        fields = dict(part.split("=", 1) for part in fragment.split("&"))
        self.invite = fields["i"]
        self.secret = base64.urlsafe_b64decode(fields["k"] + "=" * (-len(fields["k"]) % 4))
        self.key = ec.generate_private_key(ec.SECP256R1())
        self.public = self.key.public_key().public_bytes(serialization.Encoding.X962,
                                                         serialization.PublicFormat.UncompressedPoint)

    def proof(self) -> str:
        msg = b"bipagents-view-v1\x00" + self.invite.encode() + b"\x00" + self.public
        return hmac.new(self.secret, msg, hashlib.sha256).hexdigest()

    def unwrap(self, wrapped: str, share: str, epoch: int) -> bytes:
        raw = base64.b64decode(wrapped)
        ephemeral, nonce, sealed = raw[:65], raw[65:77], raw[77:]
        shared = self.key.exchange(ec.ECDH(), ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), ephemeral))
        box = HKDF(algorithm=hashes.SHA256(), length=32, salt=ephemeral + self.public, info=b"bipagents-box-v1").derive(shared)
        inner = json.loads(AESGCM(box).decrypt(nonce, sealed, None))
        assert inner["s"] == share and inner["e"] == epoch
        k = base64.b64decode(inner["k"])
        msg = b"bipagents-view-key-v1\x00" + share.encode() + b"\x00" + str(epoch).encode() + b"\x00" + k
        assert hmac.compare_digest(inner["t"], hmac.new(self.secret, msg, hashlib.sha256).hexdigest())
        return k


def open_item(key: bytes, data: str) -> dict:
    raw = base64.b64decode(data)
    return json.loads(AESGCM(key).decrypt(raw[:12], raw[12:], None))


def test_a_conversation_is_kept_sealed_for_the_invited_phone_only(bridge, fakes):
    guichet = FakeGuichet().start()
    try:
        fakes.hermes.add_session("sess-lisbonne", [
            {"id": 1, "role": "user", "content": "On organise Lisbonne ?"},
            {"id": 2, "role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]},
            {"id": 3, "role": "tool", "content": "mail de la banque : solde 1 234 €"},
            {"id": 4, "role": "assistant", "content": "Voici un plan pour trois jours.\nMEDIA:/somewhere/else.pdf"},
        ], title="Voyage à Lisbonne")
        made = bridge.post("/v1/read-shares", headers=AUTH, json={
            "agent": "wellness", "session": "sess-lisbonne", "title": "Voyage à Lisbonne", "from": "Alex",
            "share": "share0123456789abcdef", "writer": "writer-token-0123456789", "guichet": guichet.url,
            "ends": time.time() + 30 * 86400})
        assert made.status_code == 200, made.text
        view = made.json()
        assert view["id"] == "share0123456789abcdef" and view["count"] == 0
        assert "shown read-only" in view["instructions"]
        assert guichet.notified == [False]                  # the first upload tells nobody
        assert set(guichet.items) == {"meta", "m-1", "m-4"}  # no tool call, no tool result

        link = bridge.post("/v1/read-shares/share0123456789abcdef/invites", headers=AUTH, json={"name": "Léa"}).json()
        assert link["link"].startswith("https://bipagents.com/v#i=") and "&k=" in link["link"]
        lea = Phone(link["link"])
        guichet.redeem(lea.invite, lea.public, lea.proof(), "1")
        # a key slipped in by someone without the link's secret: never keyed
        mallory = ec.generate_private_key(ec.SECP256R1()).public_key().public_bytes(
            serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
        other = bridge.post("/v1/read-shares/share0123456789abcdef/invites", headers=AUTH, json={"name": "Sam"}).json()
        guichet.redeem(Phone(other["link"]).invite, mallory, "00" * 32, "2")

        synced = bridge.post("/v1/read-shares/share0123456789abcdef/sync", headers=AUTH)
        assert synced.status_code == 200, synced.text
        states = {v["name"]: v["state"] for v in synced.json()["viewers"]}
        assert states == {"Léa": "ok", "Sam": "refused"}
        viewers = {v["viewer"]: v for v in guichet.viewers}
        assert "wrapped" not in viewers["2"]
        key = lea.unwrap(viewers["1"]["wrapped"], "share0123456789abcdef", 1)
        items = {i: open_item(key, guichet.items[i]["data"]) for i in guichet.items}
        assert all(item["s"] == "share0123456789abcdef" and item["id"] == i for i, item in items.items())
        assert items["meta"]["title"] == "Voyage à Lisbonne" and items["meta"]["from"] == "Alex"
        assert items["m-1"]["role"] == "user" and items["m-1"]["text"] == "On organise Lisbonne ?"
        assert items["m-4"]["text"] == "Voici un plan pour trois jours."
        assert "solde" not in json.dumps(items)

        # a new reply: sent, the viewers told
        fakes.hermes.messages[("", "sess-lisbonne")].append({"id": 5, "role": "assistant", "content": "Et un musée ?"})
        assert bridge.post("/v1/read-shares/share0123456789abcdef/sync", headers=AUTH).status_code == 200
        assert open_item(key, guichet.items["m-5"]["data"])["text"] == "Et un musée ?"
        assert guichet.notified[-1] is True

        # a photo the person sent in it: sealed too, named by its message
        photo = b"\x89PNG\r\n\x1a\n" + b"vue" * 100
        fakes.hermes.messages[("", "sess-lisbonne")].append({"id": 6, "role": "user", "content": [
            {"type": "text", "text": "La vue de l'hôtel"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(photo).decode()}}]})
        assert bridge.post("/v1/read-shares/share0123456789abcdef/sync", headers=AUTH).status_code == 200
        said = open_item(key, guichet.items["m-6"]["data"])
        assert said["text"] == "La vue de l'hôtel" and len(said["media"]) == 1
        shown = open_item(key, guichet.items[said["media"][0]]["data"])
        assert shown["kind"] == "photo" and shown["mime"] == "image/png" and base64.b64decode(shown["data"]) == photo

        # Léa removed: a new key, everything sent again; her old key opens nothing new
        removed = bridge.delete("/v1/read-shares/share0123456789abcdef/viewers/1", headers=AUTH)
        assert removed.status_code == 200, removed.text
        assert guichet.epoch == 2 and set(guichet.items) == {"meta", "m-1", "m-4", "m-5", "m-6", said["media"][0]}
        try:
            open_item(key, guichet.items["m-5"]["data"])
            raise AssertionError("the old key opened a new item")
        except Exception as exc:  # noqa: BLE001 - InvalidTag
            assert type(exc).__name__ == "InvalidTag"

        # the share listed for the conversation; stopped: gone here and at the guichet
        listed = bridge.get("/v1/read-shares?agent=wellness&session=sess-lisbonne", headers=AUTH).json()["shares"]
        assert [s["id"] for s in listed] == ["share0123456789abcdef"]
        assert bridge.delete("/v1/read-shares/share0123456789abcdef", headers=AUTH).json() == {"ok": True}
        assert guichet.gone
        assert bridge.get("/v1/read-shares?agent=wellness&session=sess-lisbonne", headers=AUTH).json() == {"shares": []}
        assert bridge.post("/v1/read-shares/share0123456789abcdef/sync", headers=AUTH).status_code == 404
        assert bridge.get("/v1/read-shares?agent=wellness&session=sess-lisbonne").status_code == 401
    finally:
        guichet.stop()


def test_a_share_needs_an_https_guichet_and_a_known_agent(bridge):
    body = {"agent": "wellness", "session": "s1", "title": "T", "from": "Alex", "share": "share0123456789abcdef",
            "writer": "w", "guichet": "http://evil.test", "ends": time.time() + 86400}
    assert bridge.post("/v1/read-shares", headers=AUTH, json=body).status_code == 422
    assert bridge.post("/v1/read-shares", headers=AUTH, json=dict(body, agent="ghost")).status_code == 404
    assert bridge.post("/v1/read-shares", headers=AUTH, json=dict(body, ends=1)).status_code == 422


def test_the_agent_s_private_tools_close_while_it_answers_in_a_shared_conversation(bridge, fakes):
    from bbkit.common import sse
    from bbkit.mcp import call, give_token, text_of
    from bbkit.servers import wait_until

    guichet = FakeGuichet().start()
    try:
        fakes.hermes.add_session("sess-fete", [{"id": 1, "role": "user", "content": "La fête de samedi"}], title="Fête")
        made = bridge.post("/v1/read-shares", headers=AUTH, json={
            "agent": "wellness", "session": "sess-fete", "title": "Fête", "from": "Alex",
            "share": "share0123456789abcdef", "writer": "writer-token-0123456789", "guichet": guichet.url,
            "ends": time.time() + 30 * 86400})
        assert made.status_code == 200, made.text
        give_token(bridge)
        assert "shown to people" not in json.dumps(call(bridge, "mail_search", {"query": "x"}))
        gate = threading.Event()
        fakes.hermes.runs["run_s"] = [sse("tool.started", run_id="run_s", tool="mail_search"), gate,
                                      sse("run.completed", run_id="run_s", output="Samedi !")]
        watched = bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_s",
                                                               "session_id": "sess-fete"})
        assert watched.status_code == 200, watched.text
        closed = call(bridge, "mail_search", {"query": "banque"})
        assert closed["isError"] is True and "shown to people your person invited" in text_of(closed)
        assert "shown to people" not in json.dumps(call(bridge, "weather", {"place": "Lisbonne"}))
        gate.set()
        assert wait_until(lambda: "shown to people" not in json.dumps(call(bridge, "mail_search", {"query": "x"})),
                          timeout=15)
    finally:
        guichet.stop()
