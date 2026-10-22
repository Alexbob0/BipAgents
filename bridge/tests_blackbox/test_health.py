"""« Santé »: the person's health days sent by the phone (`/v1/health-data…`), each agent's categories
(`PATCH /v1/agents/{id}/settings` with `health` and `health_categories`), and the `health` tool over MCP serving each
agent only its own. Rust only: skipped when `/health` does not list the feature (the Python bridge)."""
import datetime
import json

import pytest

from bbkit.common import AUTH
from bbkit.mcp import call as mcp_call, give_token, rpc, text_of
from conftest import managed_install, profile, wait_registered

COACH = "coach-token-" + "h" * 30
VIE = "vie-token-" + "v" * 32


def two_agents(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach", "vie"))
    for name in ("coach", "vie"):
        (profile(home, name) / "SOUL.md").write_text(f"Tu es {name.title()}.\n")
    bridge.start()
    if "health_categories" not in bridge.get("/health").json().get("features", []):
        pytest.skip("this bridge has no health categories")
    wait_registered(bridge, ["coach", "vie"])
    return bridge


def settings(bridge, agent, **body):
    answer = bridge.patch(f"/v1/agents/{agent}/settings", headers=AUTH, json=body)
    assert answer.status_code == 200, answer.text
    return answer.json()["health"]


def tokens(bridge):
    """The agents' tokens given again: each change of their tools writes their entry anew, with a new token."""
    give_token(bridge, "coach", COACH)
    give_token(bridge, "vie", VIE)


def call(bridge, name, arguments, token):
    tokens(bridge)
    return mcp_call(bridge, name, arguments, token=token)


def tool(bridge, token):
    tokens(bridge)
    tools = rpc(bridge, "tools/list", token=token).json()["result"]["tools"]
    return next((t for t in tools if t["name"] == "health"), None)


def test_each_agent_reads_only_the_categories_it_was_given(bridges, fakes):
    bridge = two_agents(bridges, fakes)
    assert settings(bridge, "coach", health=True) == {"on": True, "categories": ["activity", "sleep", "heart"]}
    # « Tout partager (sauf le plus sensible) » for one, the sensitive ones alone for the other.
    everything = ["activity", "sleep", "heart", "body", "food", "wellbeing"]
    assert settings(bridge, "coach", health_categories=everything) == {"on": True, "categories": everything}
    assert settings(bridge, "vie", health=True, health_categories=["cycle", "glucose"]) == {
        "on": True, "categories": ["glucose", "cycle"]}
    odd = bridge.patch("/v1/agents/vie/settings", headers=AUTH, json={"health_categories": ["ecg"]})
    assert odd.status_code == 422
    status = bridge.get("/v1/health-data", headers=AUTH).json()
    assert status["categories"] == {"coach": everything, "vie": ["glucose", "cycle"]}
    assert status["shared"] == everything + ["glucose", "cycle"]
    assert bridge.get("/v1/agents/vie/settings", headers=AUTH).json()["health"]["categories"] == ["glucose", "cycle"]

    described = tool(bridge, VIE)
    assert described["inputSchema"]["properties"]["categories"]["items"]["enum"] == ["glucose", "cycle"]
    assert "blood glucose" in described["description"] and "steps" not in described["description"]

    today = datetime.date.today().isoformat()
    day = {"date": today, "steps": 9000, "distance_km": 6.2, "flights": 11, "stand_hours": 9,
           "sleep": {"asleep_min": 420, "deep_min": 65, "rem_min": 100, "core_min": 230, "awake_min": 25},
           "resp_rate": 14.2, "wrist_temp_c": 35.4, "resting_hr": 56, "hrv_ms": 48, "hr_avg": 71, "hr_min": 49,
           "hr_max": 151, "spo2_pct": 97.5, "vo2max": 43.8, "weight_kg": 70.4, "body_fat_pct": 18.1, "bmi": 22.3,
           "bp_sys": 118, "bp_dia": 77, "kcal_eaten": 2150, "water_l": 1.9, "caffeine_mg": 180, "mindful_min": 12,
           "mood_valence": 0.35, "mood_logs": 2, "glucose_mmol": 5.7, "glucose_min_mmol": 4.6,
           "glucose_max_mmol": 7.8, "menstrual_flow": "medium", "cycle_start": True}
    sent = bridge.put("/v1/health-data/days", headers=AUTH, json={"days": [day]})
    assert sent.status_code == 200 and sent.json() == {"stored": 1, "days": 1}

    coach = text_of(call(bridge, "health", {"detail": "days", "days": 1}, token=COACH))
    for figure in ("9000 steps", "6.2 km walked or run", "11 flights", "9 stand hours", "[deep 1 h 05, REM 1 h 40",
                   "14.2 breaths/min", "HR 71 bpm (49–151)", "blood oxygen 97.5 %", "VO2 max 43.8", "70.4 kg",
                   "body fat 18.1 %", "BMI 22.3", "blood pressure 118/77", "2150 kcal eaten", "1.9 L water",
                   "180 mg caffeine", "12 min mindfulness", "mood +0.35"):
        assert figure in coach, f"{figure} not in {coach}"
    assert "glucose" not in coach and "flow" not in coach
    vie = text_of(call(bridge, "health", {}, token=VIE))
    assert "Blood glucose: 5.7 mmol/L on average (lowest 4.6, highest 7.8)" in vie
    assert f"Cycle: period started on {today}; flow on 1 days (medium ×1)" in vie
    assert "Steps" not in vie and "Sleep" not in vie and "Weight" not in vie
    refused = call(bridge, "health", {"categories": ["sleep"]}, token=VIE)
    assert refused["isError"] and "some of glucose, cycle" in text_of(refused)

    # The cycle taken back: no agent reads it any more, so the place no longer keeps it.
    assert settings(bridge, "vie", health_categories=["glucose"])["categories"] == ["glucose"]
    kept = json.loads((bridge.state_dir / "health" / "days" / f"{today}.json").read_text())
    assert "menstrual_flow" not in kept and kept["glucose_mmol"] == 5.7 and kept["steps"] == 9000
    assert "cycle" not in bridge.get("/v1/health-data", headers=AUTH).json()["shared"]
    assert "medium" not in text_of(call(bridge, "health", {"categories": ["glucose"]}, token=VIE))

    # Its switch off: its categories go with it, and setting them is refused.
    assert settings(bridge, "vie", health=False) == {"on": False, "categories": []}
    assert bridge.patch("/v1/agents/vie/settings", headers=AUTH, json={"health_categories": ["sleep"]}).status_code == 400
    assert tool(bridge, VIE) is None
    assert call(bridge, "health", {}, token=VIE)["isError"]


def test_the_new_figures_are_checked_not_refused(bridges, fakes):
    bridge = two_agents(bridges, fakes)
    settings(bridge, "coach", health=True, health_categories=["heart", "cycle"])
    today = datetime.date.today().isoformat()
    odd = {"date": today, "spo2_pct": 140, "hr_avg": 70, "menstrual_flow": "torrential", "steps": 5000}
    assert bridge.put("/v1/health-data/days", headers=AUTH, json={"days": [odd]}).json()["stored"] == 1
    text = text_of(call(bridge, "health", {"detail": "days"}, token=COACH))
    assert f"{today}: HR 70 bpm" in text and "oxygen" not in text and "flow" not in text and "steps" not in text
