import asyncio
import json

import httpx

from bipbridge import jobs, mail_scan
from bipbridge.config import AgentConfig, parse_config
from bipbridge.hermes import HermesClient


class FakeHermes:
    """A gateway with one conversation at a time: each poll of its counters adds `step` input tokens while a run goes
    on; a run ends after `rounds` polls of its status, or when stopped."""

    def __init__(self, rounds=3, step=40_000, output="Rapport : trois écoles comparées, page « Écoles »."):
        self.rounds, self.step, self.output = rounds, step, output
        self.requests = []
        self.runs = {}
        self.counters = {"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0}
        self.messages = []
        self.forget_runs = False

    def transport(self):
        return httpx.MockTransport(self.handle)

    def handle(self, request):
        body = json.loads(request.content) if request.content else None
        self.requests.append((request.method, request.url.path, body))
        path = request.url.path.removeprefix("/p/coach")
        if request.method == "POST" and path == "/api/sessions":
            return httpx.Response(200, json={"session": {"id": "s1", "title": body["title"]}})
        if request.method == "GET" and path == "/api/sessions/s1":
            if any(r["status"] == "running" for r in self.runs.values()):
                self.counters["input_tokens"] += self.step
                self.counters["output_tokens"] += self.step // 20
            return httpx.Response(200, json={"session": {"id": "s1", **self.counters}})
        if request.method == "GET" and path == "/api/sessions/s1/messages":
            return httpx.Response(200, json={"messages": self.messages})
        if request.method == "POST" and path == "/v1/runs":
            run_id = f"r{len(self.runs) + 1}"
            self.runs[run_id] = {"status": "running", "polls": 0, "input": body["input"]}
            return httpx.Response(202, json={"run_id": run_id, "status": "started"})
        if request.method == "POST" and path.endswith("/stop"):
            self.runs[path.split("/")[3]]["status"] = "cancelled"
            return httpx.Response(200, json={"status": "stopping"})
        if request.method == "GET" and path.startswith("/v1/runs/"):
            run = self.runs.get(path.split("/")[3])
            if run is None or self.forget_runs:
                return httpx.Response(404, json={"error": "not found"})
            run["polls"] += 1
            if run["status"] == "running" and run["polls"] >= self.rounds:
                run["status"] = "completed"
                return httpx.Response(200, json={"status": "completed", "output": self.output,
                                                 "usage": {"input_tokens": 100_000, "output_tokens": 5_000}})
            return httpx.Response(200, json={"status": run["status"]})
        return httpx.Response(404)


class FakePush:
    def __init__(self):
        self.told = []

    async def notify_job(self, agent, session_id, line, job_id, body):
        self.told.append((agent, session_id, line, body))


def setup(config_dict, tmp_path):
    config = parse_config(config_dict)
    agent = AgentConfig(name="coach", display_name="Coach", hermes_url="http://127.0.0.1:8642/p/coach", hermes_key="k")
    config.agents["coach"] = agent
    return config, agent


def run(scenario):
    asyncio.run(scenario())


def test_a_task_runs_in_its_own_conversation_and_ends_with_its_report(config_dict, tmp_path):
    config, agent = setup(config_dict, tmp_path)
    hermes, push = FakeHermes(), FakePush()

    async def scenario():
        async with httpx.AsyncClient(transport=hermes.transport()) as http:
            board = jobs.Jobs(config, HermesClient(http), push=push, poll=0.01)
            job = await board.create(agent, "Écoles du quartier", "Compare les trois écoles près de chez nous.",
                                     1_000_000, "research")
            assert job["state"] == "running" and job["session_id"] == "s1"
            await board.followers[job["id"]]
            done = board.view(job["id"])
            assert done["state"] == "done" and done["report"].startswith("Rapport")
            # The run's own usage at the end, not the counters' guess.
            assert done["tokens"] == {"input": 100_000, "output": 5_000, "cached": 0}
            posted = [b for m, p, b in hermes.requests if m == "POST"]
            assert posted[0]["title"] == "🧩 Écoles du quartier"
            assert posted[1]["session_id"] == "s1" and posted[1]["instructions"] == jobs.INSTRUCTIONS
            assert posted[1]["input"].startswith("Écoles du quartier\n\nCompare")
            assert push.told == [("coach", "s1", "Tâche de fond terminée : Écoles du quartier", "Tâche de fond terminée")]
            listed = board.list()
            assert listed[0]["id"] == job["id"] and "base" not in listed[0] and "run_id" not in listed[0]

    run(scenario)


def test_a_task_stops_at_its_budget_and_goes_on_with_a_bigger_one(config_dict, tmp_path):
    config, agent = setup(config_dict, tmp_path)
    hermes, push = FakeHermes(rounds=50, step=60_000), FakePush()

    async def scenario():
        async with httpx.AsyncClient(transport=hermes.transport()) as http:
            board = jobs.Jobs(config, HermesClient(http), push=push, poll=0.01)
            job = await board.create(agent, "Gros tri", "Range tout.", 100_000)
            await board.followers[job["id"]]
            paused = board.view(job["id"])
            assert paused["state"] == "paused" and paused["reason"] == "budget"
            assert ("POST", "/p/coach/v1/runs/r1/stop", None) in hermes.requests
            assert paused["tokens"]["input"] >= 100_000 * 20 // 21   # counted from the conversation's counters
            assert push.told[-1][3] == "Tâche de fond en pause"
            # Resuming without more budget is refused; with more, the same conversation goes on.
            try:
                await board.resume(job["id"])
                raise AssertionError("resumed without budget")
            except jobs.JobError:
                pass
            hermes.rounds = 2
            before = paused["tokens"]["input"]
            await board.resume(job["id"], max_tokens=5_000_000, message="Continue, et termine par ton rapport.")
            await board.followers[job["id"]]
            done = board.view(job["id"])
            assert done["state"] == "done" and done["runs"] == 2
            assert done["tokens"]["input"] == before + 100_000   # the second run's own usage added
            assert hermes.runs["r2"]["input"] == "Continue, et termine par ton rapport."
            assert sum(1 for m, p, b in hermes.requests if p.endswith("/api/sessions") and m == "POST") == 1

    run(scenario)


def test_the_person_pauses_resumes_and_forgets_a_task(config_dict, tmp_path):
    config, agent = setup(config_dict, tmp_path)
    hermes = FakeHermes(rounds=1000, step=10)

    async def scenario():
        async with httpx.AsyncClient(transport=hermes.transport()) as http:
            board = jobs.Jobs(config, HermesClient(http), poll=0.01)
            job = await board.create(agent, "Long", "Fais-le.", 1_000_000)
            try:
                await board.create(agent, "Autre", "Encore.", 1_000_000)
                raise AssertionError("two at once")
            except jobs.JobError as exc:
                assert "already" in str(exc)
            await asyncio.sleep(0.05)
            assert (await board.pause(job["id"]))["state"] == "stopping"
            await board.followers[job["id"]]
            assert board.view(job["id"])["state"] == "paused" and board.view(job["id"])["reason"] == "stopped"
            await board.resume(job["id"])
            assert hermes.runs["r2"]["input"] == jobs.CONTINUE
            await board.forget(job["id"])
            assert jobs.load(config, job["id"]) is None
            assert hermes.runs["r2"]["status"] == "cancelled"

    run(scenario)


def test_an_agent_proposes_a_task_the_person_starts(config_dict, tmp_path):
    config, agent = setup(config_dict, tmp_path)
    hermes, push = FakeHermes(rounds=1), FakePush()

    async def scenario():
        async with httpx.AsyncClient(transport=hermes.transport()) as http:
            board = jobs.Jobs(config, HermesClient(http), push=push, poll=0.01)
            result = await jobs.propose_tool(board, agent, {"title": "Dossier location", "prompt": "Prépare le dossier.",
                                                            "budget": "small"})
            assert not result["isError"] and "Tâches de fond" in result["content"][0]["text"]
            proposed = board.list()[0]
            assert proposed["state"] == "proposed" and proposed["proposed_by"] == "agent"
            assert proposed["max_tokens"] == jobs.BUDGETS["small"]
            assert not any(p.endswith("/v1/runs") for _, p, _ in hermes.requests)   # nothing spent before the go
            assert push.told[0][3] == "Tâche de fond proposée" and push.told[0][1] is None
            await board.start(proposed["id"], max_tokens=500_000)
            await board.followers[proposed["id"]]
            assert board.view(proposed["id"])["state"] == "done"

    run(scenario)


def test_after_a_restart_a_finished_run_is_read_from_its_conversation(config_dict, tmp_path):
    config, agent = setup(config_dict, tmp_path)
    hermes = FakeHermes(rounds=1000)

    async def scenario():
        async with httpx.AsyncClient(transport=hermes.transport()) as http:
            first = jobs.Jobs(config, HermesClient(http), poll=0.01)
            job = await first.create(agent, "Recherche", "Cherche.", 50_000_000)
            first.followers[job["id"]].cancel()   # the bridge stops
            hermes.forget_runs = True             # Hermes restarted meanwhile: the run is gone, its turns stay
            hermes.messages = [{"role": "user", "content": "Recherche\n\nCherche."},
                               {"role": "assistant", "content": "Voici le rapport."}]
            again = jobs.Jobs(config, HermesClient(http), poll=0.01)
            assert await again.resume_following() == 1
            await again.followers[job["id"]]
            assert again.view(job["id"])["state"] == "done" and again.view(job["id"])["report"] == "Voici le rapport."

    run(scenario)


def test_mail_scans_are_listed_and_controlled_with_the_tasks(config_dict, tmp_path):
    config, agent = setup(config_dict, tmp_path)
    mail_scan.save(config, {"agent": "coach", "account": "a1", "label": "Gmail", "state": "paused", "error": "budget",
                            "created": 1.0, "ranges": [], "tokens": {"input": 9000, "output": 900, "cached": 0},
                            "processed": 30, "pages": 0, "step": {"since": "2026-07-21", "until": "2026-10-21",
                                                                  "max_tokens": 10_000, "mails": 80, "processed": 30}})

    class Scans:
        def __init__(self):
            self.calls = []

        def running(self, name):
            return False

        def pause(self, config, name):
            self.calls.append(("pause", name))

        def resume(self, config, agent, max_tokens):
            self.calls.append(("resume", agent.name, max_tokens))

        def forget(self, config, name):
            self.calls.append(("forget", name))

    scans = Scans()
    board = jobs.Jobs(config, HermesClient(httpx.AsyncClient()), scans=scans)
    listed = board.list()
    assert listed == [{"id": "mail-scan:coach", "kind": "mail_scan", "agent": "coach", "title": "Gmail",
                       "state": "paused", "reason": "budget", "error": None, "created": 1.0, "finished": None,
                       "tokens": {"input": 9000, "output": 900, "cached": 0}, "max_tokens": 10_000,
                       "progress": {"done": 30, "total": 80}, "processed": 30, "pages": 0}]

    async def scenario():
        await board.resume("mail-scan:coach", 20_000)
        await board.forget("mail-scan:coach")

    run(scenario)
    assert scans.calls == [("resume", "coach", 20_000), ("forget", "coach")]


def test_the_routes_need_the_bridge_key_and_list_the_jobs(client):
    from tests.conftest import BRIDGE_KEY

    assert client.get("/v1/jobs").status_code == 401
    answer = client.get("/v1/jobs", headers={"Authorization": f"Bearer {BRIDGE_KEY}"})
    assert answer.status_code == 200 and answer.json() == {"jobs": [], "budgets": jobs.BUDGETS}
    missing = client.post("/v1/jobs/t000000000000/pause", headers={"Authorization": f"Bearer {BRIDGE_KEY}"})
    assert missing.status_code == 404


def test_a_running_task_says_what_it_does_now(config_dict, tmp_path):
    from bipbridge.hermes import SSEEvent

    config, agent = setup(config_dict, tmp_path)
    hermes = FakeHermes(rounds=1000, step=10)

    class Sub:
        backlog = [SSEEvent("tool.started", {"tool": "web_search", "preview": "mutuelle famille 4"}),
                   SSEEvent("message.delta", {"delta": "…"})]

    class Hub:
        def watch(self, agent, run_id, session_id=None, end_push=True):
            pass

        def subscription(self, agent, run_id):
            return Sub() if run_id == "r1" else None

    async def scenario():
        async with httpx.AsyncClient(transport=hermes.transport()) as http:
            board = jobs.Jobs(config, HermesClient(http), hub=Hub(), poll=0.01)
            job = await board.create(agent, "Mutuelles", "Compare.", 1_000_000)
            listed = board.list("coach")
            assert listed[0]["activity"] == {"tool": "web_search", "preview": "mutuelle famille 4"}
            assert board.list("other") == []
            await board.forget(job["id"])

    run(scenario)
