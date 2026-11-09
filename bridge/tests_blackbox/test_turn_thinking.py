"""Automatic thinking (turn_thinking.rs, Alex 9/11): an agent whose level the person never chose thinks only on the
turns that need it (a calculation, a plan, a comparison), answers at once on the others; the phone's own decision wins
over the rule; a thinking turn that ends without an answer makes the next turns answer without thinking; a level the
person chose stays every turn's. The run's `model_options` is what Hermes reads (`_request_reasoning_config`)."""
import yaml

from bbkit.common import AUTH, read_sse, sse
from bbkit.fakes import CLOSE
from conftest import managed_install, profile, wait_registered

THINK = {"reasoning": {"enabled": True, "effort": "medium"}}
OFF = {"reasoning": {"enabled": False}}
GLM_FLOOR = {"reasoning": {"enabled": True, "effort": "low"}}
SPARK = "model:\n  provider: custom\n  default: GLM-5.3-Flash-EXL3\n  base_url: http://gpu:8888/v1\nagent:\n  max_turns: 60\n"
EUROUTER = "model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\n"
NO_REPLY = "⚠️ No reply: dgx-spark didn't produce a reply this time, even after retries."


def start(bridges, fakes, config):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=(), public_hub_url="https://mini.ts.net:8642")
    bridge.config["agents"]["coach"] = {"hermes_url": f"{fakes.hermes.url}/p/coach", "hermes_key": "k" * 32}
    folder = profile(home, "coach")
    (folder / "config.yaml").write_text(config)
    bridge.start()
    wait_registered(bridge, ["coach"])
    return bridge, folder


def turn(bridge, fakes, n, text, output="D'accord.", **extra):
    """Message `n` (its own new conversation): its run's `model_options`, and the run's end as the apps read it."""
    run_id = f"r{len(fakes.hermes.started) + 1}"
    fakes.hermes.runs[run_id] = [sse("run.started", run_id=run_id), sse("run.completed", run_id=run_id, output=output),
                                 CLOSE]
    body = {"agent": "coach", "client_id": f"C1D00000-0000-4000-8000-00000000000{n}", "text": text, **extra}
    sent = bridge.post("/v1/threads/new/messages", headers=AUTH, json=body)
    assert sent.status_code == 200, sent.text
    assert sent.json()["run_id"] == run_id
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=coach", headers=AUTH, timeout=30) as resp:
        ends = [data for name, data in read_sse(resp) if name == "run.completed"]
    assert ends, run_id
    return fakes.hermes.started[run_id]["model_options"], ends[0]["output"]


def options(bridge, fakes, n, text, **extra):
    return turn(bridge, fakes, n, text, **extra)[0]


def test_calculations_think_and_plain_chat_answers_at_once(bridges, fakes):
    bridge, folder = start(bridges, fakes, SPARK)
    split = "Alex a payé 240 €, John 90 €, Fabien rien. On partage à trois : qui doit combien à qui ?"
    assert options(bridge, fakes, 1, split) == THINK
    assert options(bridge, fakes, 2, "Salut ! Ça va ?") == OFF
    assert options(bridge, fakes, 3, "Fais-moi un plan en 3 étapes pour le week-end") == THINK
    # the phone's decision wins over the rule (an on-phone decider)
    assert options(bridge, fakes, 4, "Salut", thinking=True) == THINK
    assert options(bridge, fakes, 5, "Combien font 12 x 7 ?", thinking=False) == OFF
    # a level the person chose: every turn's, as before (Hermes reads it from the profile)
    assert bridge.put("/v1/agents/coach/thinking", headers=AUTH, json={"level": "high"}).status_code == 200
    assert yaml.safe_load((folder / "config.yaml").read_text())["agent"]["reasoning_effort"] == "high"
    assert options(bridge, fakes, 6, split) is None
    assert options(bridge, fakes, 7, "Salut") is None


def test_glm_through_eurouter_keeps_its_floor_on_every_turn(bridges, fakes):
    # « none » makes GLM answer nothing there; « medium » only made it slower (bench 9/11)
    bridge, _ = start(bridges, fakes, EUROUTER)
    assert options(bridge, fakes, 1, "Le resto fait 87,50 € avec 15 % de pourboire, divisé par 3 ?") == GLM_FLOOR
    assert options(bridge, fakes, 2, "Merci beaucoup") == GLM_FLOOR


def test_a_thinking_turn_without_an_answer_stops_thinking_for_the_next_turns(bridges, fakes):
    bridge, _ = start(bridges, fakes, SPARK)
    sent, said = turn(bridge, fakes, 1, "137/3 au centime ?", output=NO_REPLY)
    assert sent == THINK
    assert said.strip() and not said.startswith("⚠️")   # never an empty turn: a plain line instead of Hermes' own
    # the next calculations answer without thinking (Hermes' own ladder had already asked again without it)
    assert options(bridge, fakes, 2, "Et 245/7 ?") == OFF
    assert options(bridge, fakes, 3, "Et 12 x 9 ?", thinking=True) == OFF
    assert options(bridge, fakes, 4, "Merci") == OFF
