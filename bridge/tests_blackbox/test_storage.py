"""The place's space watched (storage_watch.rs, Alex 2026-10-28): what takes it by agent and kind (`/v1/storage`), the
80 % / 95 % card once per threshold and its note for the agent's next turn, each choice applied by the bridge (keep the
last N, archive, send to the phone then delete, delete), the rules per agent and kind, a full place never silent (an
upload or a tool refused, a card), and the biggest agent's sandbox paused while the place is over its allowance."""
import json
import os
import time

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of

FIVE_GB = 5 * 2**30


def offer(bridge, used):
    """A place of the all-included offer whose last backup measured `used` bytes."""
    settings = bridge.home / ".config" / "bipagents" / "backup.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(json.dumps({"url": "https://g.test/v1/backup/", "token": "t", "password": "the-key"}))
    state = bridge.home / ".local" / "state" / "bipagents" / "backup.json"
    state.parent.mkdir(parents=True, exist_ok=True)
    state.write_text(json.dumps({"last_ok": 1790000000.0, "bytes": used}))


def podcasts(bridge, n, size=1000, agent="wellness"):
    folder = bridge.state_dir / "agent-audio" / agent
    folder.mkdir(parents=True, exist_ok=True)
    for i in range(n):
        path = folder / f"2026{i:04}-pod.m4a"
        path.write_bytes(b"\0" * size)
        os.utime(path, (1790000000 + i * 100, 1790000000 + i * 100))
    return folder


def space_cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-space-")]


def approve(bridge, card, choice):
    return bridge.post("/v1/approve", headers=AUTH, json={"agent": card["agent"], "run_id": card["run_id"],
                                                          "request_id": card["request_id"], "choice": choice})


def test_what_takes_the_space_by_agent_and_kind(bridge):
    podcasts(bridge, 3, 2000)
    body = bridge.post("/v1/storage/measure", headers=AUTH).json()
    assert body["allowance"] is None and body["percent"] is None   # a self-install: no limit
    wellness = body["by_agent"][0]
    assert wellness["agent"] == "wellness" and wellness["kinds"]["audio"] == 6000
    assert set(wellness["kinds"]) == {"audio", "pages", "received", "made", "space", "browser", "archives", "connectors"}
    assert body["rule_kinds"] == ["audio", "pages", "made", "archives"] and body["paused"] == []
    assert "more_space_available" not in body
    assert space_cards(bridge) == []
    assert bridge.get("/v1/storage").status_code == 401


def test_80_percent_one_card_one_note_and_keep_the_last(bridge):
    podcasts(bridge, 20)
    offer(bridge, int(FIVE_GB * 0.81))
    body = bridge.post("/v1/storage/measure", headers=AUTH).json()
    assert body["percent"] in (80, 81) and body["allowance"] == FIVE_GB
    cards = space_cards(bridge)
    assert len(cards) == 1
    card = cards[0]
    assert card["agent"] == "wellness" and card["choices"] == ["keep_last", "archive", "delete", "deny"]
    assert card["description"].startswith("Ton espace sert au travail de tes agents, pas à tout garder. Il est plein à 80 %")
    assert card["bip_space"]["level"] == 80 and card["bip_space"]["kind"] == "audio"
    assert card["bip_space"]["count"] == 20 and card["bip_space"]["keep_last"] == 10
    # measured again: no second card, no second note this month
    bridge.post("/v1/storage/measure", headers=AUTH)
    assert len(space_cards(bridge)) == 1
    note = bridge.post("/v1/live/recaps/take", headers=AUTH, json={"agent": "wellness", "session": "s1"}).json()["recap"]
    assert note.startswith("<!--bip:space\n") and "80 % full" in note and "podcasts" in note
    assert bridge.post("/v1/live/recaps/take", headers=AUTH, json={"agent": "wellness", "session": "s1"}).json()["recap"] is None
    done = approve(bridge, card, "keep_last:5")
    assert done.status_code == 200 and done.json() == {"ok": True, "choice": "keep_last", "removed": 15, "freed": 15000}
    assert len(list((bridge.state_dir / "agent-audio" / "wellness").iterdir())) == 5
    assert approve(bridge, card, "delete").status_code == 404
    assert space_cards(bridge) == []
    kept = bridge.get("/v1/storage", headers=AUTH).json()["kept_rules"]
    assert kept == {"wellness": {"audio": {"keep_last": 5}}}
    logged = bridge.get("/v1/approvals/answered", headers=AUTH, params={"agent": "wellness"}).json()["items"]
    assert logged[-1]["topic"] == "space" and logged[-1]["choice"] == "keep_last"
    told = bridge.post("/v1/live/recaps/take", headers=AUTH, json={"agent": "wellness", "session": "s1"}).json()["recap"]
    assert "keep your last 5" in told
    # 95 %: a card again (its own threshold)
    offer(bridge, int(FIVE_GB * 0.96))
    bridge.post("/v1/storage/measure", headers=AUTH)
    assert [c["bip_space"]["level"] for c in space_cards(bridge)] == [95]


def test_a_full_place_refuses_an_upload_and_the_person_gets_the_card(bridge):
    podcasts(bridge, 6, 2_000_000)
    offer(bridge, FIVE_GB)
    full = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("a.pdf", b"%PDF-1.4", "application/pdf")})
    assert full.status_code == 507
    cards = space_cards(bridge)
    assert len(cards) == 1 and cards[0]["bip_space"]["level"] == 100
    assert cards[0]["choices"] == ["keep_last", "archive", "send", "delete", "deny"]
    # archive: all but the newest N put aside (counted under « archives »)
    done = approve(bridge, cards[0], "archive").json()
    assert done["choice"] == "archive" and done["removed"] == 3
    body = bridge.post("/v1/storage/measure", headers=AUTH).json()
    assert body["by_agent"][0]["kinds"]["archives"] > 0
    assert len(list((bridge.state_dir / "agent-audio" / "wellness").iterdir())) == 3


def test_a_full_place_a_tool_says_so_plainly(bridge):
    give_token(bridge)
    offer(bridge, FIVE_GB)
    result = call(bridge, "create_page", {"html": "<p>x</p>", "title": "Plan"})
    assert result["isError"] is True and "no room left" in text_of(result)
    assert "Ton espace est plein" in text_of(result)
    assert len(space_cards(bridge)) == 1
    # the card is not made twice while it waits
    call(bridge, "create_page", {"html": "<p>x</p>", "title": "Plan"})
    assert len(space_cards(bridge)) == 1


def test_sent_to_the_phone_then_deleted_here(bridge):
    podcasts(bridge, 2, 2_000_000)
    offer(bridge, FIVE_GB)
    bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                files={"file": ("a.pdf", b"%PDF-1.4", "application/pdf")})
    card = space_cards(bridge)[0]
    assert card["bip_space"]["send_count"] == 2 and card["bip_space"]["send_bytes"] == 4_000_000
    done = approve(bridge, card, "send").json()
    assert done["sent"] == 2 and done["sent_bytes"] == 4_000_000
    sent = space_cards(bridge)
    assert len(sent) == 1 and sent[0]["bip_space"]["stage"] == "sent" and sent[0]["choices"] == ["delete", "deny"]
    assert sent[0]["description"].startswith("Je t'ai envoyé 2 fichiers")
    # the Boîte has the agent's message with the files, served to the app
    def boite():
        return bridge.get("/v1/outbox", headers=AUTH, params={"agent": "wellness"}).json()
    deadline = time.time() + 5
    while time.time() < deadline and "space-out" not in json.dumps(boite()):
        time.sleep(0.2)
    path = json.dumps(boite()).split("MEDIA:")[1].split("\\n")[0].split('"')[0]
    assert path.startswith("/bipagents/space-out/wellness/")
    assert bridge.get("/v1/media", params={"path": path}, headers=AUTH).status_code == 200
    done = approve(bridge, sent[0], "delete").json()
    assert done == {"ok": True, "choice": "delete", "removed": 2, "freed": 4_000_000}
    assert bridge.get("/v1/media", params={"path": path}, headers=AUTH).status_code == 404
    assert space_cards(bridge) == []


def test_delete_and_later(bridge):
    podcasts(bridge, 4)
    offer(bridge, int(FIVE_GB * 0.85))
    bridge.post("/v1/storage/measure", headers=AUTH)
    card = space_cards(bridge)[0]
    assert approve(bridge, card, "deny").json()["choice"] == "deny"
    assert len(list((bridge.state_dir / "agent-audio" / "wellness").iterdir())) == 4
    offer(bridge, FIVE_GB)
    bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                files={"file": ("a.pdf", b"%PDF-1.4", "application/pdf")})
    card = space_cards(bridge)[0]
    assert approve(bridge, card, "delete").json()["removed"] == 4
    assert list((bridge.state_dir / "agent-audio" / "wellness").iterdir()) == []


def test_rules_per_agent_and_kind(bridge):
    podcasts(bridge, 6)
    changed = bridge.patch("/v1/storage/rules", headers=AUTH, json={"agent": "wellness", "kind": "audio", "keep_last": 2})
    assert changed.status_code == 200 and changed.json()["kept_rules"] == {"wellness": {"audio": {"keep_last": 2}}}
    assert len(list((bridge.state_dir / "agent-audio" / "wellness").iterdir())) == 2
    assert changed.json()["by_agent"][0]["kinds"]["audio"] == 2000
    assert bridge.patch("/v1/storage/rules", headers=AUTH,
                        json={"agent": "wellness", "kind": "browser", "keep_last": 2}).status_code == 422
    assert bridge.patch("/v1/storage/rules", headers=AUTH,
                        json={"agent": "nobody", "kind": "audio", "keep_last": 2}).status_code == 404
    removed = bridge.patch("/v1/storage/rules", headers=AUTH, json={"agent": "wellness", "kind": "audio", "keep_last": None})
    assert removed.json()["kept_rules"] == {}
    # the agent's own tool: an estimate, then the person's choice kept
    give_token(bridge)
    estimate = text_of(call(bridge, "space_estimate", {"kind": "audio", "per_week": 7}))
    assert "Propose to keep the last" in estimate and "a month" in estimate
    kept = call(bridge, "space_estimate", {"kind": "audio", "keep_last": 1})
    assert kept["isError"] is False and "only your last 1" in text_of(kept)
    assert len(list((bridge.state_dir / "agent-audio" / "wellness").iterdir())) == 1


def test_over_its_allowance_the_biggest_agent_s_sandbox_pauses_then_resumes(bridge):
    podcasts(bridge, 3, 1000)
    bridge.commands.rule("podman", ["ps"], stdout="c0ffee\n")
    offer(bridge, FIVE_GB + 1)
    body = bridge.post("/v1/storage/measure", headers=AUTH).json()
    assert body["paused"] == ["wellness"]
    assert ["pause", "c0ffee"] in bridge.commands.of("podman")
    ps = [c for c in bridge.commands.of("podman") if c[0] == "ps"][0]
    assert "label=hermes-profile=wellness" in ps
    cards = space_cards(bridge)
    assert len(cards) == 1 and cards[0]["bip_space"]["paused"] is True
    assert "en pause tant que l'espace est plein" in cards[0]["description"]
    give_token(bridge)
    refused = call(bridge, "create_audio", {"text": "Bonjour"})
    assert refused["isError"] is True and "Ton espace est plein" in text_of(refused)
    # measured again while still over: not paused twice
    bridge.post("/v1/storage/measure", headers=AUTH)
    assert bridge.commands.of("podman").count(["pause", "c0ffee"]) == 1
    # back under: resumed, the agent told
    offer(bridge, FIVE_GB // 2)
    body = bridge.post("/v1/storage/measure", headers=AUTH).json()
    assert body["paused"] == []
    assert ["unpause", "c0ffee"] in bridge.commands.of("podman")
    note = bridge.post("/v1/live/recaps/take", headers=AUTH, json={"agent": "wellness", "session": "s"}).json()["recap"]
    assert "works again" in note
    log = bridge.log_path.read_text()
    assert "the agent's sandbox paused" in log and "the agent's sandbox resumed" in log
