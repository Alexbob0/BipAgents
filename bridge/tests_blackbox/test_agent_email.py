"""An agent's own Kerfy Studio address (agent_email.rs), asked of a fake guichet with the place's token. The guichet
gives an address it already made a new password only with its current one (audit 2026-10-27, 17): told « exists »,
the bridge asks again with the password it holds instead of failing."""
import json

from starlette.responses import JSONResponse

from bbkit.common import AUTH
from bbkit.fakes import Fake
from conftest import managed_install, profile


class FakeMailGuichet(Fake):
    name = "guichet-mail"

    def reset(self) -> None:
        super().reset()
        self.passwords = {}   # agent -> its address's current password
        self.made = 0

    async def handle(self, seen):
        if seen.headers.get("authorization") != "Bearer place-token":
            return JSONResponse({"detail": "unknown token"}, status_code=401)
        parts = seen.path.strip("/").split("/")   # v1 mailboxes <agent>
        if parts[:2] != ["v1", "mailboxes"] or len(parts) != 3:
            return JSONResponse({"detail": "no route"}, status_code=404)
        agent = parts[2]
        if seen.method == "DELETE":
            return JSONResponse({"ok": True}) if self.passwords.pop(agent, None) else JSONResponse(
                {"detail": "no address"}, status_code=404)
        body = json.loads(seen.body or b"{}")
        with self.lock:
            if agent in self.passwords and body.get("current") != self.passwords[agent]:
                return JSONResponse({"detail": "this address exists: its current password is needed (else delete it "
                                               "and make a new one)"}, status_code=403)
            self.made += 1
            self.passwords[agent] = f"pw-{self.made}"
        return JSONResponse({
            "address": f"{agent}.k7m2@a.bipagents.test", "password": self.passwords[agent],
            "imap": {"host": "mail.bipagents.test", "port": 993, "security": "tls"},
            "smtp": {"host": "mail.bipagents.test", "port": 465, "security": "tls"},
            "authserv_id": "mail.bipagents.test", "retention_days": 90})


def place_bridge(bridges, fakes, guichet):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(home, "coach")
    settings = bridge.home / ".config" / "bipagents" / "backup.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(json.dumps({"url": guichet.url + "/v1/backup", "token": "place-token"}))
    return bridge.start(), folder


def puts(guichet):
    return [json.loads(r.body or b"{}") for r in guichet.seen("/v1/mailboxes/coach", "PUT")]


def test_an_existing_address_is_asked_again_with_its_current_password(bridges, fakes):
    guichet = FakeMailGuichet().start()
    try:
        bridge, folder = place_bridge(bridges, fakes, guichet)
        on = bridge.post("/v1/agents/coach/email", headers=AUTH, json={"kind": "bipagents"})
        assert on.status_code == 201, on.text
        assert "EMAIL_PASSWORD=pw-1" in (folder / ".env").read_text()
        assert [("current" in b) for b in puts(guichet)] == [False]
        # this place lost the agent's record (its profile wasn't written, say): the address is still the guichet's
        (bridge.data / "agent-email" / "coach.json").unlink()
        again = bridge.post("/v1/agents/coach/email", headers=AUTH, json={"kind": "bipagents"})
        assert again.status_code == 201, again.text
        sent = puts(guichet)
        assert len(sent) == 3 and "current" not in sent[1] and sent[2]["current"] == "pw-1"
        assert "EMAIL_PASSWORD=pw-2" in (folder / ".env").read_text()
        assert bridge.logged("agent e-mail address asked again with its current password")
        assert "pw-1" not in bridge.log_path.read_text()
        # turned off: deleted at the guichet, nothing of its password kept here
        assert bridge.delete("/v1/agents/coach/email", headers=AUTH).status_code == 200
        assert not (bridge.data / "agent-email" / "given" / "coach.json").exists()
    finally:
        guichet.stop()


def test_an_existing_address_whose_password_is_lost_is_refused_plainly(bridges, fakes):
    guichet = FakeMailGuichet().start()
    try:
        bridge, folder = place_bridge(bridges, fakes, guichet)
        guichet.passwords["coach"] = "made-elsewhere"
        refused = bridge.post("/v1/agents/coach/email", headers=AUTH, json={"kind": "bipagents"})
        assert refused.status_code == 409 and "no longer has its password" in refused.json()["detail"]
        assert [("current" in b) for b in puts(guichet)] == [False]   # nothing to prove it with: asked once
        assert not (bridge.data / "agent-email" / "coach.json").exists()
        assert "EMAIL_PASSWORD" not in ((folder / ".env").read_text() if (folder / ".env").exists() else "")
    finally:
        guichet.stop()
