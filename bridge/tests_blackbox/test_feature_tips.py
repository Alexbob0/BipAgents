"""Features discovered over the first weeks (feature_tips.rs, Alex 2026-10-28): a moment noticed in the person's message
(`/v1/live/recaps/take` with `text` and `kinds`) proposes the fitting feature once, as a hidden note for the agent and a
`bip-tip-` card « Essayer / Plus tard / Pas pour moi »; one a day, never at night, never in a routine's run, « Pas pour
moi » never again; the page `/v1/tips` with each feature's state, the costumes, the counts; a switch to turn it off."""
import json

from bbkit.common import AUTH


def tips_file(bridge):
    return bridge.state_dir / "tips.json"


def tweak(bridge, **fields):
    """The bridge's tips state changed on disk (the clock can't be: quiet hours, the day of the last proposal)."""
    path = tips_file(bridge)
    state = json.loads(path.read_text()) if path.exists() else {"enabled": True}
    state.update(fields)
    path.write_text(json.dumps(state))


def daytime(bridge):
    """Never quiet, no Friday podcast: the triggers alone."""
    tweak(bridge, quiet=[0, 0], podcast_day=9)


def say(bridge, text, kinds=None, session="s1"):
    body = {"agent": "wellness", "session": session, "text": text}
    if kinds:
        body["kinds"] = kinds
    response = bridge.post("/v1/live/recaps/take", headers=AUTH, json=body)
    assert response.status_code == 200, response.text
    return response.json()["recap"]


def tip_cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-tip-")]


def approve(bridge, card, choice):
    return bridge.post("/v1/approve", headers=AUTH, json={"agent": card["agent"], "run_id": card["run_id"],
                                                          "request_id": card["request_id"], "choice": choice})


def feature_of(view, id):
    return next(f for f in view["features"] if f["id"] == id)


def test_a_trip_proposes_the_project_agent_with_a_card(bridge):
    daytime(bridge)
    note = say(bridge, "On prépare un voyage à Lisbonne avec des amis")
    assert note.startswith("<!--bip:tip\n") and "project agent" in note and note.endswith("-->")
    cards = tip_cards(bridge)
    assert len(cards) == 1
    card = cards[0]
    assert card["choices"] == ["try", "later", "never"] and card["agent"] == "wellness"
    assert card["bip_tip"]["feature"] == "project" and card["bip_tip"]["action"] == {"kind": "message"}
    assert card["description"].startswith("Un agent pour un projet")
    assert "Lisbonne" not in tips_file(bridge).read_text(), "no content kept"
    done = approve(bridge, card, "try")
    assert done.status_code == 200
    assert done.json() == {"ok": True, "choice": "try", "feature": "project", "action": {"kind": "message"},
                           "costume": "beret"}
    assert approve(bridge, card, "try").status_code == 404
    logged = bridge.get("/v1/approvals/answered", headers=AUTH, params={"agent": "wellness"}).json()["items"]
    assert logged[-1]["topic"] == "tip" and logged[-1]["choice"] == "try"
    view = bridge.get("/v1/tips", headers=AUTH).json()
    assert feature_of(view, "project")["state"] == "tried"
    assert "beret" in view["costumes"]["unlocked"] and "chef" in view["costumes"]["locked"]
    assert view["stats"]["project"] == {"proposed": 1, "accepted": 1, "later": 0, "declined": 0, "tried": 1,
                                        "adopted": 0}
    # the agent is told on its next turn
    assert "Essayer" in say(bridge, "merci")
    assert bridge.get("/v1/tips").status_code == 401


def test_one_a_day_and_later(bridge):
    daytime(bridge)
    assert "the vault" in say(bridge, "j'ai encore oublié mon mot de passe")
    card = tip_cards(bridge)[0]
    assert say(bridge, "voici ma facture", kinds=["pdf"]) is None   # noticed, waits: a card is there
    assert approve(bridge, card, "later").json() == {"ok": True, "choice": "later", "feature": "vault"}
    assert say(bridge, "merci") is None, "one a day"
    tweak(bridge, last_day="2000-01-01")   # the next day
    assert "Archives" in say(bridge, "salut")
    assert [c["bip_tip"]["feature"] for c in tip_cards(bridge)] == ["archives"]
    view = bridge.get("/v1/tips", headers=AUTH).json()
    assert feature_of(view, "vault")["later_until"] is not None and feature_of(view, "vault")["state"] == "proposed"


def test_quiet_hours_routines_and_busy_conversations_wait(bridge):
    tweak(bridge, quiet=[0, 24], podcast_day=9)   # always night
    assert say(bridge, "rappelle-moi d'appeler le garage") is None
    assert tip_cards(bridge) == []
    daytime(bridge)
    assert say(bridge, "rappelle-moi d'appeler le garage", session="cron_daily_20261028_080000") is None
    assert tip_cards(bridge) == []
    note = say(bridge, "ok")   # the moment noticed at night comes now
    assert "routine" in note


def test_never_again(bridge):
    daytime(bridge)
    say(bridge, "mon mot de passe Netflix")
    card = tip_cards(bridge)[0]
    assert approve(bridge, card, "never").json()["choice"] == "never"
    assert "never propose it again" in say(bridge, "ok")
    for _ in range(3):
        tweak(bridge, last_day="2000-01-01", calendar_done=[1, 2, 4, 7, 10, 14, 21, 28])
        assert say(bridge, "et mon mot de passe ?") is None
    view = bridge.get("/v1/tips", headers=AUTH).json()
    assert feature_of(view, "vault")["declined"] is True and view["stats"]["vault"]["declined"] == 1


def test_adoption_the_page_and_the_switch(bridge):
    daytime(bridge)
    assert bridge.post("/v1/tips/used", headers=AUTH, json={"feature": "carnet"}).json() == {"ok": True,
                                                                                            "costume": "apprendre"}
    assert feature_of(bridge.get("/v1/tips", headers=AUTH).json(), "carnet")["state"] == "tried"
    state = json.loads(tips_file(bridge).read_text())
    state["features"]["carnet"]["use_days"] = ["2026-01-01", "2026-01-02"]
    tips_file(bridge).write_text(json.dumps(state))
    bridge.post("/v1/tips/used", headers=AUTH, json={"feature": "carnet"})
    view = bridge.get("/v1/tips", headers=AUTH).json()
    assert feature_of(view, "carnet")["state"] == "adopted" and view["stats"]["carnet"]["adopted"] == 1
    assert bridge.post("/v1/tips/used", headers=AUTH, json={"feature": "nope"}).status_code == 422
    tried = bridge.post("/v1/tips/live/try", headers=AUTH).json()
    assert tried["action"] == {"kind": "open", "target": "live"} and tried["costume"] == "musique"
    assert bridge.post("/v1/tips/nope/try", headers=AUTH).status_code == 404
    # off: nothing noticed, nothing proposed, every costume open
    off = bridge.put("/v1/tips/settings", headers=AUTH, json={"enabled": False}).json()
    assert off["enabled"] is False and off["costumes"]["locked"] == []
    assert say(bridge, "mon mot de passe") is None and tip_cards(bridge) == []
    assert bridge.put("/v1/tips/settings", headers=AUTH, json={"enabled": True}).json()["enabled"] is True


def test_an_older_app_without_text(bridge):
    daytime(bridge)
    response = bridge.post("/v1/live/recaps/take", headers=AUTH, json={"agent": "wellness", "session": "s1"})
    assert response.status_code == 200 and response.json()["recap"] is None   # day 0: nothing from the calendar
    view = bridge.get("/v1/tips", headers=AUTH).json()
    assert view["day"] == 0 and view["enabled"] is True and len(view["features"]) == 12
