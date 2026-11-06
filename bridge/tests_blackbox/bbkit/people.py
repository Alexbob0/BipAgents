"""The release journeys' people (docs/release-journeys.md, J32+): friends, relatives, other households.

- `Guichet`: Kerfy's guichet as both sides reach it for « Partager en lecture » (guichet-rs `read_shares.rs`): the
  owner's app makes a share with its account, the owner's bridge keeps it (writer token), a friend's phone signs in
  (accounts), looks at its link, redeems it with a key of its own and reads the sealed items. Only ciphertext here.
- `Friend`: a friend's phone, as both apps do it (ReadSharesCenter.swift, ViewerShares.kt): its account, its key for
  the share, the proof bound to the link's secret, the content key unwrapped and checked, the items opened.
- `project_share(journey, guichet)`: Alex's project agent « Pour les queens » on the journeys' bridge, a turn with a
  map in its thread, the whole thread shared at the guichet (as the owner's app does it), one link per friend.
- `households(bridges, fakes, …)`: two linked installs, Alex's and Marie's (one install is one person: a household),
  their agents answered by the journeys' scripted model.
"""
from __future__ import annotations

import base64
import hashlib
import hmac
import json
import secrets
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional
from urllib.parse import parse_qs

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from starlette.responses import JSONResponse

from .common import AUTH
from .fakes import Fake
from .servers import wait_until

PROJECT_TOKEN = "queens-token-0123456789abcdefghijklmn"


# -- the guichet ----------------------------------------------------------------------------------------------------

class Guichet(Fake):
    """The guichet's read-share and account routes (guichet-rs read_shares.rs, accounts.rs) for the journeys: every
    share, invitation, viewer and item kept in memory, the same answers' shapes. An account signs in with any e-mail
    code and is approved at once (the account's own journeys are not these)."""
    name = "guichet"

    def reset(self) -> None:
        super().reset()
        self.accounts: Dict[str, Dict[str, Any]] = {}     # token -> {account, device, email, key, name, platform}
        self.shares: Dict[str, Dict[str, Any]] = {}       # id -> {writer, account, epoch, ends, seq, items, viewers, invites}
        self.notified: List[bool] = []

    # helpers ----------------------------------------------------------------------------------------------------
    def sign_in(self, email: str, platform: str = "test", key: str = "") -> Dict[str, Any]:
        with self.lock:
            return self._sign_in(email, platform, key, "Téléphone")

    def _sign_in(self, email: str, platform: str, key: str, name: str) -> Dict[str, Any]:
        account = next((a["account"] for a in self.accounts.values() if a["email"] == email), len(self.accounts) + 1)
        token = "acct-" + secrets.token_urlsafe(24)
        device = f"d{len(self.accounts) + 1}"
        self.accounts[token] = {"account": account, "device": device, "email": email, "key": key, "name": name,
                                "platform": platform, "created": time.time()}
        return {"device": device, "token": token, "state": "approved", "created": True}

    def make_share(self, token: str, ends: float) -> Dict[str, Any]:
        """`POST /v1/rshares` as the owner's app makes one: `{share, writer, ends}`."""
        with self.lock:
            return self._make_share(self.accounts[token], ends)

    def _make_share(self, me: Dict[str, Any], ends: float) -> Dict[str, Any]:
        share = "rs" + secrets.token_hex(10)
        writer = "w-" + secrets.token_urlsafe(24)
        self.shares[share] = {"writer": writer, "account": me["account"], "epoch": 1, "ends": ends, "seq": 0,
                              "items": {}, "viewers": [], "invites": {}, "updated": None, "created": time.time()}
        return {"share": share, "writer": writer, "ends": ends}

    @staticmethod
    def _bearer(seen) -> str:
        return (seen.headers.get("authorization") or "").removeprefix("Bearer ").strip()

    # routes -----------------------------------------------------------------------------------------------------
    async def handle(self, seen):
        parts = seen.path.strip("/").split("/")
        try:
            body = json.loads(seen.body or b"{}")
        except ValueError:
            body = {}
        with self.lock:
            if parts[:2] == ["v1", "account"]:
                answer = self._account(seen, parts[2:], body)
            elif parts[:2] == ["v1", "rshares"]:
                answer = self._rshares(seen, parts[2:], body)
            else:
                answer = JSONResponse({"detail": "no route"}, status_code=404)
        # each call and its answer (shown under a failed test, and in serve_journeys_people.py's log)
        print(f"[guichet] {seen.method} {seen.path} -> {answer.status_code} {bytes(answer.body)[:160]!r}", flush=True)
        return answer

    def _account(self, seen, rest, body):
        if seen.method == "POST" and rest == ["email"]:
            return JSONResponse({"sent": True})
        if seen.method == "POST" and rest == ["signin"]:
            device = body.get("device") or {}
            return JSONResponse(self._sign_in(str(body.get("email") or "ami@example.org"), str(device.get("platform") or ""),
                                              str(device.get("key") or ""), str(device.get("name") or "Téléphone")))
        me = self.accounts.get(self._bearer(seen))
        if me is None:
            return JSONResponse({"detail": "unknown token"}, status_code=401)
        if seen.method == "GET" and rest == []:
            devices = [{"id": a["device"], "name": a["name"], "platform": a["platform"] or "ios", "state": "approved",
                        "created": a["created"], "seen": time.time(), "key": a["key"] or "", "me": a is me}
                       for a in self.accounts.values() if a["account"] == me["account"]]
            return JSONResponse({"state": "approved", "email": me["email"], "methods": ["email"],
                                 "recovery": {"code": True, "waiting_until": None, "mine": False},
                                 "devices": devices, "places": [], "transfers": 0, "totp": False})
        if seen.method == "GET" and rest == ["boxes"]:
            return JSONResponse({"boxes": []})
        if seen.method == "GET" and (rest[:1] == ["invitations"] or rest[:2] == ["family", "invitations"]):
            return JSONResponse({"invitations": []})
        if seen.method in ("PUT", "POST"):
            return JSONResponse({"ok": True})
        return JSONResponse({"detail": "no route"}, status_code=404)

    def _viewer(self, seen):
        return self.accounts.get(self._bearer(seen))

    def _invite(self, given: str):
        h = hashlib.sha256(given.encode()).hexdigest()
        for share_id, share in self.shares.items():
            if h in share["invites"]:
                return share_id, share, share["invites"][h]
        return None, None, None

    def _rshares(self, seen, rest, body):
        method = seen.method
        # the owner's app
        if method == "POST" and rest == []:
            me = self._viewer(seen)
            if me is None:
                return JSONResponse({"detail": "unknown token"}, status_code=401)
            return JSONResponse(self._make_share(me, float(body.get("ends") or time.time() + 86400)))
        # a friend's phone
        if rest in (["look"], ["redeem"], ["shared"]):
            me = self._viewer(seen)
            if me is None:
                return JSONResponse({"detail": "unknown token"}, status_code=401)
            if rest == ["shared"]:
                out = []
                for share_id, share in self.shares.items():
                    for v in share["viewers"]:
                        if v["device"] == me["device"] and share["ends"] > time.time():
                            keyed = v.get("wrapped") if v.get("wrapped_epoch") == share["epoch"] else None
                            out.append({"share": share_id, "viewer": v["viewer"], "from": v["sender"],
                                        "ends": share["ends"], "epoch": share["epoch"], "wrapped": keyed,
                                        "muted": v["muted"], "updated": share["updated"]})
                return JSONResponse({"shares": out})
            share_id, share, invite = self._invite(str(body.get("invite") or ""))
            if share is None:
                return JSONResponse({"detail": "no such invitation"}, status_code=404)
            if invite["expires"] < time.time():
                return JSONResponse({"detail": "expired"}, status_code=410)
            if invite["used"] is not None:
                return JSONResponse({"detail": "already used"}, status_code=409)
            if rest == ["look"]:
                return JSONResponse({"from": invite["from"], "ends": share["ends"], "expires": invite["expires"]})
            if share["account"] == me["account"]:
                return JSONResponse({"detail": "this is your own share"}, status_code=409)
            invite["used"] = time.time()
            viewer = str(sum(len(s["viewers"]) for s in self.shares.values()) + 1)
            share["viewers"].append({"viewer": viewer, "invite": invite["invite"], "name": invite["name"],
                                     "sender": invite["from"], "device": me["device"],
                                     "public_key": str(body.get("public_key") or ""), "proof": str(body.get("proof") or ""),
                                     "joined": time.time(), "muted": False})
            return JSONResponse({"share": share_id, "viewer": viewer, "from": invite["from"], "ends": share["ends"]})
        share_id = rest[0] if rest else ""
        share = self.shares.get(share_id)
        if share is None:
            return JSONResponse({"detail": "no such share"}, status_code=404)
        rest = rest[1:]
        token = self._bearer(seen)
        if token == share["writer"]:
            return self._writer(share_id, share, method, rest, body)
        me = self._viewer(seen)
        mine = [v for v in share["viewers"] if me is not None and v["device"] == me["device"]]
        if not mine:
            return JSONResponse({"detail": "unknown token"}, status_code=401 if me is None else 403)
        if method == "GET" and rest == ["items"]:
            after = int((parse_qs(seen.query).get("after") or ["0"])[0] or 0)
            items = sorted((i for i in share["items"].values() if i["seq"] > after), key=lambda i: i["seq"])
            return JSONResponse({"epoch": share["epoch"], "ends": share["ends"], "items": items, "more": False})
        if method == "POST" and rest == ["mute"]:
            mine[0]["muted"] = bool(body.get("muted"))
            return JSONResponse({"muted": mine[0]["muted"]})
        if method == "DELETE" and rest == ["me"]:
            share["viewers"] = [v for v in share["viewers"] if v is not mine[0]]
            return JSONResponse({"ok": True})
        if method == "POST" and rest == ["actions"]:
            return JSONResponse({"seq": 1}, status_code=201)
        return JSONResponse({"detail": "no route"}, status_code=404)

    def _writer(self, share_id, share, method, rest, body):
        if method == "GET" and rest == []:
            viewers = [{"viewer": v["viewer"], "invite": v["invite"], "name": v["name"], "public_key": v["public_key"],
                        "proof": v["proof"], "keyed": v.get("wrapped_epoch") == share["epoch"] and bool(v.get("wrapped")),
                        "joined": v["joined"], "muted": v["muted"]} for v in share["viewers"]]
            invites = [{"invite": i["invite"], "name": i["name"], "expires": i["expires"], "used": i["used"]}
                       for i in share["invites"].values()]
            return JSONResponse({"share": share_id, "epoch": share["epoch"], "ends": share["ends"], "bytes": 0,
                                 "viewers": viewers, "invites": invites})
        if method == "POST" and rest == ["invites"]:
            token = secrets.token_urlsafe(24)
            h = hashlib.sha256(token.encode()).hexdigest()
            share["invites"][h] = {"invite": h, "name": str(body.get("name") or ""), "from": str(body.get("from") or ""),
                                   "expires": time.time() + 7 * 86400, "used": None}
            return JSONResponse({"invite": token, "hash": h, "expires": share["invites"][h]["expires"]})
        if method == "DELETE" and rest[:1] == ["invites"] and len(rest) == 2:
            share["invites"].pop(rest[1], None)
            return JSONResponse({"ok": True})
        if method == "PUT" and rest == ["keys"]:
            if body.get("epoch") != share["epoch"]:
                return JSONResponse({"detail": "rekey"}, status_code=409)
            for k in body.get("keys") or []:
                for v in share["viewers"]:
                    if v["viewer"] == k.get("viewer"):
                        v["wrapped"], v["wrapped_epoch"] = k.get("wrapped"), share["epoch"]
            return JSONResponse({"ok": True})
        if method == "POST" and rest == ["items"]:
            if body.get("epoch") != share["epoch"]:
                return JSONResponse({"detail": "rekey"}, status_code=409)
            for item in body.get("items") or []:
                if not (isinstance(item.get("id"), str) and isinstance(item.get("kind"), str) and item.get("data")):
                    return JSONResponse({"detail": "items: {id, kind, data (base64)}"}, status_code=422)
                share["seq"] += 1
                share["items"][item["id"]] = {"seq": share["seq"], "id": item["id"], "kind": item["kind"],
                                              "data": item["data"], "at": time.time()}
            share["updated"] = time.time()
            self.notified.append(bool(body.get("notify")))
            return JSONResponse({"last": share["seq"]})
        if method == "DELETE" and rest[:1] == ["viewers"] and len(rest) == 2:
            share["viewers"] = [v for v in share["viewers"] if v["viewer"] != rest[1]]
            share["epoch"] += 1
            share["items"] = {}
            for v in share["viewers"]:
                v.pop("wrapped", None)
                v.pop("wrapped_epoch", None)
            return JSONResponse({"epoch": share["epoch"]})
        if method == "GET" and rest == ["actions"]:
            return JSONResponse({"actions": []})
        if method == "POST" and rest == ["actions", "taken"]:
            return JSONResponse({"taken": 0})
        if method == "POST" and rest == ["ends"]:
            share["ends"] = float(body.get("ends") or share["ends"])
            return JSONResponse({"ends": share["ends"]})
        if method == "DELETE" and rest == []:
            del self.shares[share_id]
            return JSONResponse({"ok": True})
        return JSONResponse({"detail": "no route"}, status_code=404)


# -- a friend's phone -----------------------------------------------------------------------------------------------

class Friend:
    """A friend's phone for one link (`https://bipagents.com/v#i=<invite>&k=<secret>`), as both apps do it: its own
    P-256 key for this share, the proof `HMAC(S, "bipagents-view-v1" ‖ invite ‖ key)`, the content key unwrapped from
    the box the owner's bridge sealed for that key (and checked against S), the items opened with it."""

    def __init__(self, guichet: Guichet, email: str = "lea@example.org"):
        import httpx
        self.guichet = guichet
        self.http = httpx.Client(base_url=guichet.url, timeout=20)
        self.token = self.http.post("/v1/account/signin", json={
            "method": "email", "email": email, "code": "123456",
            "device": {"name": "iPhone de Léa", "platform": "ios", "key": ""}}).json()["token"]
        self.key = ec.generate_private_key(ec.SECP256R1())
        self.public = self.key.public_key().public_bytes(serialization.Encoding.X962,
                                                         serialization.PublicFormat.UncompressedPoint)
        self.share: Optional[str] = None
        self.secret = b""
        self.invite = ""

    def _auth(self) -> Dict[str, str]:
        return {"Authorization": f"Bearer {self.token}"}

    def open_link(self, link: str) -> Dict[str, Any]:
        """The link opened (« Alex te montre un projet »): what the guichet says of it."""
        fragment = link.split("#", 1)[1]
        fields = dict(part.split("=", 1) for part in fragment.split("&"))
        self.invite = fields["i"]
        self.secret = base64.urlsafe_b64decode(fields["k"] + "=" * (-len(fields["k"]) % 4))
        looked = self.http.post("/v1/rshares/look", headers=self._auth(), json={"invite": self.invite})
        assert looked.status_code == 200, looked.text
        return looked.json()

    def see(self) -> str:
        """« Voir »: redeemed with this phone's key and the proof that binds it to the link."""
        proof = hmac.new(self.secret, b"bipagents-view-v1\x00" + self.invite.encode() + b"\x00" + self.public,
                         hashlib.sha256).hexdigest()
        done = self.http.post("/v1/rshares/redeem", headers=self._auth(), json={
            "invite": self.invite, "public_key": base64.b64encode(self.public).decode(), "proof": proof})
        assert done.status_code == 200, done.text
        self.share = done.json()["share"]
        return self.share

    def listed(self) -> Dict[str, Any]:
        found = self.http.get("/v1/rshares/shared", headers=self._auth()).json()["shares"]
        return next(s for s in found if s["share"] == self.share)

    def content_key(self, timeout: float = 60) -> bytes:
        """Waits for the owner's bridge to key this phone (its sync: every 20 s, every 2 s while the owner watches),
        then opens the box: the key, its share and epoch, its tag made with S (the guichet cannot slip its own in)."""
        assert wait_until(lambda: self.listed().get("wrapped"), timeout=timeout), \
            f"the owner's bridge never keyed the friend's phone ({timeout} s): {self.listed()}"
        listed = self.listed()
        raw = base64.b64decode(listed["wrapped"])
        ephemeral, nonce, sealed = raw[:65], raw[65:77], raw[77:]
        shared = self.key.exchange(ec.ECDH(), ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), ephemeral))
        box = HKDF(algorithm=hashes.SHA256(), length=32, salt=ephemeral + self.public,
                   info=b"bipagents-box-v1").derive(shared)
        inner = json.loads(AESGCM(box).decrypt(nonce, sealed, None))
        assert inner["s"] == self.share and inner["e"] == listed["epoch"], inner
        k = base64.b64decode(inner["k"])
        msg = (b"bipagents-view-key-v1\x00" + self.share.encode() + b"\x00" + str(listed["epoch"]).encode()
               + b"\x00" + k)
        assert hmac.compare_digest(inner["t"], hmac.new(self.secret, msg, hashlib.sha256).hexdigest()), \
            "the content key's tag does not match the link's secret"
        return k

    def items(self, key: bytes) -> List[Dict[str, Any]]:
        """Every item of the share, as listed by the guichet (`seq`, `id`, `kind`), opened: the plaintext under
        `opened` (None when it does not open with the key)."""
        got = self.http.get(f"/v1/rshares/{self.share}/items", headers=self._auth(), params={"after": 0})
        assert got.status_code == 200, got.text
        out = []
        for item in got.json()["items"]:
            raw = base64.b64decode(item["data"])
            try:
                opened = json.loads(AESGCM(key).decrypt(raw[:12], raw[12:], None))
            except Exception:  # noqa: BLE001 - InvalidTag: shown as such by the contract check
                opened = None
            out.append({**item, "opened": opened})
        return out


# -- Alex's project, shared -----------------------------------------------------------------------------------------

@dataclass
class ProjectShare:
    agent: str           # the project agent's id
    thread: str          # its thread
    share: str           # the share's id (at the guichet and in the bridge)
    owner_token: str     # Alex's account at the guichet
    words: List[str] = field(default_factory=list)   # what was said in it, in order

    def link(self, bridge, name: str = "Léa") -> str:
        made = bridge.post(f"/v1/read-shares/{self.share}/invites", headers=AUTH, json={"name": name})
        assert made.status_code == 200, made.text
        return made.json()["link"]


def mcp_as(bridge, token: str, path: str, name: str, arguments: Dict[str, Any], timeout: float = 120) -> str:
    """One of the bridge's tools called as an agent's Hermes does (its own token)."""
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
    resp = bridge.post(path, json=body, headers={"Authorization": f"Bearer {token}"}, timeout=timeout)
    assert resp.status_code == 200, resp.text
    result = resp.json()["result"]
    text = "\n".join(p.get("text", "") for p in result.get("content", []))
    if result.get("isError"):
        raise RuntimeError(text)
    return text


def agent_tokens(journey) -> Dict[str, str]:
    """The tokens of the agents the journeys' model calls the bridge as (its `calls` look a run's agent up here)."""
    found = getattr(journey.model, "agent_tokens", None)
    if found is None:
        found = {}
        journey.model.agent_tokens = found
        original = journey.model.calls.get("map")

        def map_for(run_id: str) -> str:   # the map made by the agent whose turn it is (its own pages)
            run = journey.model.runs.get(run_id)
            token = found.get(run.agent) if run else None
            if token is None and original is not None:
                return original(run_id)
            return mcp_as(journey.bridge, token, "/v1/agent-tools/mcp", "map_image",
                          {"markers": [{"place": "Sagrada Família"}, {"place": "Park Güell"}],
                           "area": "Barcelona", "title": "Week-end à Barcelone"})
        journey.model.calls["map"] = map_for
    return found


def give_agent_token(bridge, agent: str, token: str) -> None:
    from .mcp import give_token
    give_token(bridge, agent, token)


def project_share(journey, guichet: Guichet, name: str = "Pour les queens") -> ProjectShare:
    """Alex's project agent on the journeys' bridge (`POST /v1/project-agents`, as the app makes one), a turn in its
    thread that answers with a map (`#carte`: the bridge's `map_image`, a map page), its whole thread shared at the
    guichet with Alex's account (the owner's app's two steps: `POST /v1/rshares`, then `POST /v1/read-shares`)."""
    bridge = journey.bridge
    made = bridge.post("/v1/project-agents", headers=AUTH,
                       json={"name": name, "ends": time.time() + 10 * 86400, "lang": "fr", "category": "creative"})
    assert made.status_code == 201, made.text
    agent = made.json()["agent"]
    tokens = agent_tokens(journey)
    give_agent_token(bridge, agent, PROJECT_TOKEN)
    tokens[agent] = PROJECT_TOKEN
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": agent})
    assert main.status_code == 200, main.text
    thread = main.json()["id"]
    words = "Tu peux nous préparer un week-end à Barcelone ? #carte"
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": agent, "client_id": str(uuid.uuid4()).upper(), "text": words})
    assert sent.status_code == 200, sent.text

    def page():
        return bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH,
                          params={"agent": agent, "tz": "Europe/Paris"}).json()
    assert wait_until(lambda: any("Voici la carte" in (i.get("text") or "") for i in page()["items"])
                      and page()["pending"].get("run") is None, timeout=60), page()
    owner = guichet.sign_in("alex@example.org", platform="ios")["token"]
    at_guichet = guichet.make_share(owner, time.time() + 10 * 86400)
    shared = bridge.post("/v1/read-shares", headers=AUTH, json={
        "agent": agent, "thread": True, "title": name, "from": "Alex", "share": at_guichet["share"],
        "writer": at_guichet["writer"], "guichet": guichet.url, "ends": at_guichet["ends"]})
    assert shared.status_code == 200, shared.text
    return ProjectShare(agent, thread, at_guichet["share"], owner, [words])


def share_view(bridge, agent: str) -> Dict[str, Any]:
    """The owner's view of the project's share (`GET /v1/read-shares?agent=`), as the app reads it for « Visible par
    N personnes »."""
    shares = bridge.get("/v1/read-shares", headers=AUTH, params={"agent": agent}).json()["shares"]
    assert len(shares) == 1, shares
    return shares[0]


# -- two households -------------------------------------------------------------------------------------------------

@dataclass
class Households:
    alex: Any
    marie: Any
    model: Any
    tokens: Dict[tuple, str] = field(default_factory=dict)   # (bridge name, agent) -> its tool token


def households(bridges, fakes, files: Path, alex_agents=("vie", "cuisine"), marie_agents=("coach",),
               word_gap: float = 0.03) -> Households:
    """Alex's install and Marie's (one install is one person: agent_shares.rs), linked by Alex's invitation (Réglages ›
    Famille: « Inviter un proche », then Marie's « Scanner son invitation »), their agents' turns answered by the
    journeys' scripted model (one fake Hermes for both: the runs say whose agent they are)."""
    from test_households_and_peers import household
    from .model import ScriptedModel

    alex = household(bridges, fakes, "alex", list(alex_agents))
    marie = household(bridges, fakes, "marie", list(marie_agents))
    assert alex.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert marie.put("/v1/family/name", headers=AUTH, json={"name": "Marie"}).status_code == 200
    invitation = alex.post("/v1/family/invite", headers=AUTH).json()
    joined = marie.post("/v1/family/join", headers=AUTH,
                        json={k: invitation[k] for k in ("url", "code", "fingerprint") if invitation.get(k)})
    assert joined.status_code == 200, joined.text
    model = ScriptedModel(fakes.hermes, files, word_gap=word_gap).install()
    return Households(alex, marie, model)


def household_token(homes: Households, bridge, agent: str) -> str:
    """The agent's own token to its bridge's tools (agent-tools.json, as the bridge gives one)."""
    key = (bridge.name, agent)
    if key not in homes.tokens:
        token = f"{agent}-{bridge.name}-token-" + hashlib.sha256(f"{bridge.name}/{agent}".encode()).hexdigest()[:24]
        give_agent_token(bridge, agent, token)
        homes.tokens[key] = token
    return homes.tokens[key]


def thread_page(bridge, thread: str, agent: str) -> Dict[str, Any]:
    resp = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": agent, "tz": "Europe/Paris"})
    assert resp.status_code == 200, resp.text
    return resp.json()


def send(bridge, thread: str, agent: str, text: str, client_id: Optional[str] = None):
    return bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": agent, "client_id": client_id or str(uuid.uuid4()).upper(), "text": text})


def reply_in(bridge, thread: str, agent: str, words: str, timeout: float = 60) -> Dict[str, Any]:
    def found():
        page = thread_page(bridge, thread, agent)
        return page if (any(words in (i.get("text") or "") for i in page["items"] if i.get("role") == "agent")
                        and page["pending"].get("run") is None) else None
    assert wait_until(found, timeout=timeout), thread_page(bridge, thread, agent)
    return found()

