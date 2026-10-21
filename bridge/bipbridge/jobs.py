"""Background jobs (« Tâches de fond », decided 2026-10-21): long work an agent does apart from the person's
conversations, with its cost shown before (a budget the person chooses) and after (the tokens it took), pause, resume
and cancel, and a notification when it ends. Two kinds:

    mail_scan   a mailbox read through by steps (mail_scan.py): listed and paused here, run there
    task        a long task in the agent's own Hermes, with all its tools, in a conversation of its own (« 🧩 <title> »):
                the bridge starts the run, reads the conversation's token counters every few seconds and stops the run
                once the budget is reached (paused: the person may give more and resume) or after three hours; the
                agent ends with a report, which the notification opens

An agent may propose a task (MCP ``propose_task`` in agent_tools.py, for work too long for a reply): it waits, budget and all, until the
person starts it in the app. An agent never starts paid work by itself.

    GET    /v1/jobs                        every job of every agent: proposals first, then the newest
    POST   /v1/agents/{id}/jobs            {title, prompt, max_tokens, template?}   a task started now
    POST   /v1/jobs/{id}/start             {max_tokens?}       a proposed task started
    POST   /v1/jobs/{id}/pause
    POST   /v1/jobs/{id}/resume            {max_tokens?, message?}   a bigger budget, the words that resume it
    DELETE /v1/jobs/{id}                   stopped if it runs, then forgotten (its conversation and pages stay)

Mail scans keep their own routes for their steps (/v1/agents/{id}/mail-scan…); their job id here is
``mail-scan:<agent>``.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import secrets
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

import httpx
from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from . import mail_scan
from .config import AgentConfig, Config
from .cronwatch import final_reply
from .hermes import HermesClient, HermesHTTPError
from .logs import fields

log = logging.getLogger("bipbridge.jobs")

POLL_SECONDS = 10.0
MAX_STEP_SECONDS = 3 * 3600       # a run going on longer is paused: the person resumes it if it was worth it
RUNNING_MAX = 3                   # tasks running at once on this install (one per agent)
PROPOSALS_MAX = 5                 # an agent's proposals waiting for the person
STOP_GRACE = 120.0                # seconds a stop may take before the task is counted as paused anyway
KEPT_PER_AGENT = 40               # finished tasks kept in the list, per agent
REPORT_CHARS = 4000
BUDGETS = {"small": 300_000, "medium": 1_000_000, "large": 3_000_000}   # an agent's proposal: tokens
TERMINAL = {"completed", "failed", "cancelled", "canceled", "interrupted"}
TITLE_MARK = "🧩 "
INSTRUCTIONS = """This is a background task the person gave you from the app (« Tâches de fond »): they are not following
this conversation. Work through it on your own, step by step, with your tools. Don't ask questions unless you truly
cannot go on: choose sensible defaults and say which in your report. Keep what you make where the person finds it: a
page (create_page) for a table, a comparison or a dashboard, a file in your workspace for a longer document. Your work
has a budget; if you are told to continue, pick up where you stopped instead of starting again. End with a short
report in the person's language: what you did, what you found or made and where it is, what is left or uncertain."""
CONTINUE = "Continue the task where you stopped (don't start again), then end with your report."
DONE_BODY = "Tâche de fond terminée"
PAUSED_BODY = "Tâche de fond en pause"
PROPOSED_BODY = "Tâche de fond proposée"


class JobError(Exception):
    """Safe to show."""


def _folder(config: Config) -> Path:
    return Path(config.outbox.db_path).parent / "jobs"


def _path(config: Config, job_id: str) -> Path:
    if not re.fullmatch(r"t[0-9a-f]{12}", job_id):
        raise JobError("unknown job")
    return _folder(config) / f"{job_id}.json"


def load(config: Config, job_id: str) -> Optional[Dict[str, Any]]:
    try:
        return json.loads(_path(config, job_id).read_text(encoding="utf-8"))
    except (OSError, ValueError, JobError):
        return None


def save(config: Config, job: Dict[str, Any]) -> None:
    path = _path(config, job["id"])
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(job, fh, ensure_ascii=False)
    os.replace(temporary, path)


def tasks(config: Config) -> List[Dict[str, Any]]:
    out = []
    for path in sorted(_folder(config).glob("t*.json")):
        try:
            out.append(json.loads(path.read_text(encoding="utf-8")))
        except (OSError, ValueError):
            continue
    return out


def counters(session: Dict[str, Any]) -> Dict[str, int]:
    """The conversation's token counters (Hermes keeps them per session, updated after each model call)."""
    def number(key: str) -> int:
        value = session.get(key)
        return int(value) if isinstance(value, (int, float)) and not isinstance(value, bool) else 0
    return {"input": number("input_tokens"), "output": number("output_tokens"), "cached": number("cache_read_tokens")}


def spent(job: Dict[str, Any]) -> int:
    return job["tokens"]["input"] + job["tokens"]["output"]


def scan_view(job: Dict[str, Any], running: bool) -> Dict[str, Any]:
    """A mailbox scan (mail_scan.py) in the common shape."""
    state = job.get("state", "none")
    if state in ("counting", "running") and not running:
        state = "paused"
    step = job.get("step") or {}
    last = (job.get("ranges") or [{}])[-1]
    return {"id": f"mail-scan:{job['agent']}", "kind": "mail_scan", "agent": job["agent"],
            "title": job.get("label") or "", "state": "running" if state == "counting" else state,
            "reason": "budget" if job.get("error") == "budget" else None,
            "error": job.get("error") if state == "failed" else None,
            "created": job.get("created"), "finished": last.get("at") if state in ("done", "ready") else None,
            "tokens": job.get("tokens") or {"input": 0, "output": 0, "cached": 0},
            "max_tokens": step.get("max_tokens"),
            "progress": {"done": step.get("processed", 0), "total": step.get("mails", 0)} if step else None,
            "processed": job.get("processed", 0), "pages": job.get("pages", 0)}


def task_view(job: Dict[str, Any]) -> Dict[str, Any]:
    keep = ("id", "kind", "agent", "title", "prompt", "template", "state", "reason", "error", "created", "started",
            "finished", "tokens", "max_tokens", "session_id", "report", "proposed_by", "runs")
    return {key: job.get(key) for key in keep}


class Jobs:
    """The tasks running in this bridge, each followed by one coroutine."""

    def __init__(self, config: Config, hermes: HermesClient, hub: Any = None, push: Any = None,
                 scans: Any = None, poll: float = POLL_SECONDS, max_seconds: float = MAX_STEP_SECONDS):
        self.config = config
        self.hermes = hermes
        self.hub = hub
        self.push = push
        self.scans = scans
        self.poll = poll
        self.max_seconds = max_seconds
        self.followers: Dict[str, asyncio.Task] = {}

    # -- the list ------------------------------------------------------------------------------------------------------

    def list(self) -> List[Dict[str, Any]]:
        out = [task_view(job) for job in tasks(self.config) if self.config.agent(job["agent"]) is not None]
        for name in self.config.agents:
            scan = mail_scan.load(self.config, name)
            if scan is not None:
                running = self.scans is not None and self.scans.running(name)
                out.append(scan_view(scan, running))
        out.sort(key=lambda j: (j["state"] != "proposed", j["state"] not in ("running", "stopping"),
                                -(j.get("started") or j.get("created") or 0)))
        return out

    def view(self, job_id: str) -> Dict[str, Any]:
        job = load(self.config, job_id)
        if job is None:
            raise JobError("unknown job")
        return task_view(job)

    def _agent(self, name: str) -> AgentConfig:
        agent = self.config.agent(name)
        if agent is None:
            raise JobError("this agent isn't on this install any more")
        return agent

    # -- a new task ----------------------------------------------------------------------------------------------------

    def new(self, agent: AgentConfig, title: str, prompt: str, max_tokens: int, template: Optional[str] = None,
            proposed_by: str = "person") -> Dict[str, Any]:
        title, prompt = " ".join(title.split())[:80], prompt.strip()
        if not title or not prompt:
            raise JobError("a task needs a title and what to do")
        job = {"id": "t" + secrets.token_hex(6), "kind": "task", "agent": agent.name, "title": title,
               "prompt": prompt, "template": template, "state": "proposed", "reason": None, "error": None,
               "created": time.time(), "started": None, "finished": None, "max_tokens": max_tokens,
               "tokens": {"input": 0, "output": 0, "cached": 0}, "session_id": None, "run_id": None,
               "report": None, "proposed_by": proposed_by, "runs": 0}
        save(self.config, job)
        self._trim(agent.name)
        return job

    async def create(self, agent: AgentConfig, title: str, prompt: str, max_tokens: int,
                     template: Optional[str] = None) -> Dict[str, Any]:
        self._may_run(agent.name)
        job = self.new(agent, title, prompt, max_tokens, template)
        try:
            await self._launch(job, f"{job['title']}\n\n{job['prompt']}")
        except JobError:
            _path(self.config, job["id"]).unlink(missing_ok=True)
            raise
        return task_view(job)

    async def propose(self, agent: AgentConfig, title: str, prompt: str, budget: str) -> Dict[str, Any]:
        """An agent's proposal: kept until the person starts it (or forgets it) in the app."""
        if sum(1 for j in tasks(self.config) if j["agent"] == agent.name and j["state"] == "proposed") >= PROPOSALS_MAX:
            raise JobError("enough of your proposals are waiting for the person already")
        job = self.new(agent, title, prompt, BUDGETS.get(budget, BUDGETS["medium"]), proposed_by="agent")
        if self.push is not None:
            try:
                await self.push.notify_job(agent.name, None, f"{PROPOSED_BODY} : {job['title']}", job["id"],
                                           PROPOSED_BODY)
            except Exception:   # noqa: BLE001 - the proposal stays in the list
                log.warning("job proposal not told", extra=fields(agent=agent.name))
        return job

    async def start(self, job_id: str, max_tokens: Optional[int] = None) -> Dict[str, Any]:
        job = load(self.config, job_id)
        if job is None or job["state"] != "proposed":
            raise JobError("this task isn't waiting to start")
        self._may_run(job["agent"])
        if max_tokens:
            job["max_tokens"] = max_tokens
        await self._launch(job, f"{job['title']}\n\n{job['prompt']}")
        return task_view(job)

    def _may_run(self, agent: str) -> None:
        running = [j for j in tasks(self.config) if j["state"] in ("running", "stopping")]
        if any(j["agent"] == agent for j in running):
            raise JobError("this agent is already on a background task")
        if len(running) >= RUNNING_MAX:
            raise JobError("enough background tasks are running here for now")

    def _trim(self, agent: str) -> None:
        ended = sorted((j for j in tasks(self.config) if j["agent"] == agent and j["state"] in ("done", "failed")),
                       key=lambda j: j.get("finished") or j.get("created") or 0, reverse=True)
        for job in ended[KEPT_PER_AGENT:]:
            _path(self.config, job["id"]).unlink(missing_ok=True)

    # -- running -------------------------------------------------------------------------------------------------------

    async def _launch(self, job: Dict[str, Any], text: str) -> None:
        """A run in the task's conversation (made the first time), followed until it ends."""
        agent = self._agent(job["agent"])
        try:
            if not job.get("session_id"):
                job["session_id"] = await self.hermes.create_session(agent, TITLE_MARK + job["title"])
            base = counters(await self.hermes.session(agent, job["session_id"]))
            run_id = await self.hermes.start_run(agent, text, job["session_id"], INSTRUCTIONS)
        except HermesHTTPError as exc:
            raise JobError(f"the agent refused the task ({exc.status})") from exc
        except httpx.HTTPError as exc:
            raise JobError("the agent can't be reached") from exc
        now = time.time()
        job.update(state="running", reason=None, error=None, run_id=run_id, base=base,
                   before=dict(job["tokens"]), step_started=now, started=job.get("started") or now,
                   finished=None, runs=job.get("runs", 0) + 1, stopping=None)
        save(self.config, job)
        if self.hub is not None:
            self.hub.watch(agent, run_id, job["session_id"], end_push=False)   # approvals and questions still pushed
        self._follow(job["id"])
        log.info("job started", extra=fields(agent=agent.name, job=job["id"], budget=job["max_tokens"]))

    def _follow(self, job_id: str) -> None:
        task = self.followers.get(job_id)
        if task is None or task.done():
            self.followers[job_id] = asyncio.create_task(self._watch(job_id))

    async def resume_following(self) -> int:
        """After a restart: the tasks that were running are followed again (their runs went on in Hermes)."""
        count = 0
        for job in tasks(self.config):
            if job["state"] in ("running", "stopping") and self.config.agent(job["agent"]) is not None:
                if self.hub is not None and job.get("run_id"):
                    self.hub.watch(self.config.agent(job["agent"]), job["run_id"], job.get("session_id"),
                                   end_push=False)
                self._follow(job["id"])
                count += 1
        return count

    async def _watch(self, job_id: str) -> None:
        while True:
            await asyncio.sleep(self.poll)
            job = load(self.config, job_id)
            if job is None or job["state"] not in ("running", "stopping"):
                return
            agent = self.config.agent(job["agent"])
            if agent is None:
                job.update(state="failed", error="this agent isn't on this install any more", finished=time.time())
                save(self.config, job)
                return
            try:
                run = await self.hermes.run(agent, job["run_id"])
            except HermesHTTPError as exc:
                if exc.status not in (404, 410):
                    continue
                run = {"status": "gone"}   # Hermes restarted, or forgot the run long after it ended
            except httpx.HTTPError:
                continue   # Hermes restarting: asked again at the next round
            try:
                self._count(job, await self.hermes.session(agent, job["session_id"]))
            except (HermesHTTPError, httpx.HTTPError):
                pass
            status = str(run.get("status") or "")
            if status in TERMINAL or status == "gone":
                await self._ended(agent, job, run)
                return
            if job["state"] == "running":
                if spent(job) >= job["max_tokens"]:
                    await self._stop(agent, job, "budget")
                elif time.time() - job.get("step_started", time.time()) >= self.max_seconds:
                    await self._stop(agent, job, "time")
            elif time.time() - job.get("stop_asked", time.time()) >= STOP_GRACE:
                await self._ended(agent, job, {"status": "cancelled"})   # Hermes never confirmed the stop
                return
            save(self.config, job)

    def _count(self, job: Dict[str, Any], session: Dict[str, Any], usage: Optional[Dict[str, Any]] = None) -> None:
        """What the task took so far: the tokens before this run, and this run's (its exact usage when it ended,
        else the conversation's counters since it started)."""
        before, base, now = job.get("before") or job["tokens"], job.get("base") or {}, counters(session)
        if usage and (usage.get("input_tokens") or usage.get("output_tokens")):
            run = {"input": int(usage.get("input_tokens") or 0), "output": int(usage.get("output_tokens") or 0),
                   "cached": int(usage.get("cache_read_tokens") or 0)}
        else:
            run = {key: max(0, now[key] - int(base.get(key, 0))) for key in now}
        if session or usage:
            job["tokens"] = {key: before[key] + run[key] for key in ("input", "output", "cached")}

    async def _stop(self, agent: AgentConfig, job: Dict[str, Any], reason: str) -> None:
        job.update(state="stopping", stopping=reason, stop_asked=time.time())
        save(self.config, job)
        try:
            await self.hermes.stop_run(agent, job["run_id"])
        except (HermesHTTPError, httpx.HTTPError):
            log.info("job stop refused", extra=fields(agent=agent.name, job=job["id"]))
        log.info("job stopping", extra=fields(agent=agent.name, job=job["id"], reason=reason))

    async def _ended(self, agent: AgentConfig, job: Dict[str, Any], run: Dict[str, Any]) -> None:
        status = str(run.get("status") or "")
        usage = run.get("usage") if isinstance(run.get("usage"), dict) else None
        try:
            session = await self.hermes.session(agent, job["session_id"])
        except (HermesHTTPError, httpx.HTTPError):
            session = {}
        self._count(job, session, usage)
        output = run.get("output") if isinstance(run.get("output"), str) else None
        if status == "gone":
            try:
                output = final_reply(await self.hermes.session_messages(agent, job["session_id"]))
            except (HermesHTTPError, httpx.HTTPError):
                output = None
            status = "completed" if output else "interrupted"
        now = time.time()
        if job.get("stopping") or status in ("cancelled", "canceled", "interrupted"):
            job.update(state="paused", reason=job.get("stopping") or "interrupted")
        elif status == "completed":
            job.update(state="done", report=(output or "").strip()[:REPORT_CHARS] or None, finished=now)
        else:
            error = run.get("error") if isinstance(run.get("error"), str) else ""
            job.update(state="failed", error=(error or "the task failed")[:300], finished=now)
        job.update(stopping=None, stop_asked=None, run_id=None)
        save(self.config, job)
        log.info("job ended", extra=fields(agent=agent.name, job=job["id"], state=job["state"],
                                           reason=job.get("reason"), tokens=spent(job)))
        if job["state"] == "done" or job.get("reason") in ("budget", "time"):
            await self._tell(agent, job)

    async def _tell(self, agent: AgentConfig, job: Dict[str, Any]) -> None:
        if self.push is None:
            return
        body = DONE_BODY if job["state"] == "done" else PAUSED_BODY
        line = f"{body} : {job['title']}"
        try:
            await self.push.notify_job(agent.name, job.get("session_id"), line, job["id"], body)
        except Exception:   # noqa: BLE001 - a notification that fails doesn't change the job
            log.warning("job not told", extra=fields(agent=agent.name, job=job["id"]))

    # -- the person's controls -----------------------------------------------------------------------------------------

    async def pause(self, job_id: str) -> Dict[str, Any]:
        if job_id.startswith("mail-scan:"):
            return self._scan(job_id, "pause")
        job = load(self.config, job_id)
        if job is None:
            raise JobError("unknown job")
        if job["state"] == "running":
            await self._stop(self._agent(job["agent"]), job, "stopped")
        return task_view(job)

    async def resume(self, job_id: str, max_tokens: Optional[int] = None, message: Optional[str] = None) -> Dict[str, Any]:
        if job_id.startswith("mail-scan:"):
            return self._scan(job_id, "resume", max_tokens)
        job = load(self.config, job_id)
        if job is None or job["state"] not in ("paused", "failed", "done"):
            raise JobError("nothing to resume")
        self._may_run(job["agent"])
        if max_tokens:
            job["max_tokens"] = max(max_tokens, spent(job) + 1000)
        elif spent(job) >= job["max_tokens"]:
            raise JobError("the budget is used up: give a bigger one")
        await self._launch(job, (message or "").strip()[:2000] or CONTINUE)
        return task_view(job)

    async def forget(self, job_id: str) -> None:
        if job_id.startswith("mail-scan:"):
            self._scan(job_id, "forget")
            return
        job = load(self.config, job_id)
        if job is None:
            raise JobError("unknown job")
        if job["state"] in ("running", "stopping") and job.get("run_id"):
            try:
                await self.hermes.stop_run(self._agent(job["agent"]), job["run_id"])
            except (HermesHTTPError, httpx.HTTPError, JobError):
                pass
        follower = self.followers.pop(job_id, None)
        if follower is not None:
            follower.cancel()
        _path(self.config, job_id).unlink(missing_ok=True)

    def _scan(self, job_id: str, action: str, max_tokens: Optional[int] = None) -> Dict[str, Any]:
        name = job_id.removeprefix("mail-scan:")
        agent = self.config.agent(name)
        if agent is None or self.scans is None or mail_scan.load(self.config, name) is None:
            raise JobError("unknown job")
        try:
            if action == "pause":
                self.scans.pause(self.config, name)
            elif action == "resume":
                self.scans.resume(self.config, agent, max_tokens)
            else:
                self.scans.forget(self.config, name)
                return {"id": job_id, "state": "none"}
        except mail_scan.ScanError as exc:
            raise JobError(str(exc)) from exc
        return scan_view(mail_scan.load(self.config, name) or {"agent": name}, self.scans.running(name))


# -- the agents' tool (agent_tools.PROPOSE_TASK) ---------------------------------------------------------------------------------------------------

async def propose_tool(jobs: Optional[Jobs], agent: AgentConfig, arguments: Dict[str, Any]) -> Dict[str, Any]:
    def text(line: str, error: bool = False) -> Dict[str, Any]:
        return {"content": [{"type": "text", "text": line}], "isError": error}
    if jobs is None:
        return text("background tasks aren't available here", error=True)
    try:
        job = await jobs.propose(agent, str(arguments.get("title") or ""), str(arguments.get("prompt") or "")[:8000],
                                 str(arguments.get("budget") or "medium"))
    except JobError as exc:
        return text(str(exc), error=True)
    return text(f"Proposed (budget about {job['max_tokens']:,} tokens). It starts only when the person starts it in "
                "« Tâches de fond »: tell them so in your reply.")


# -- the app's routes ---------------------------------------------------------------------------------------------------

class NewTask(BaseModel):
    title: str = Field(..., min_length=1, max_length=200)
    prompt: str = Field(..., min_length=1, max_length=8000)
    max_tokens: int = Field(..., ge=10_000, le=50_000_000)
    template: Optional[str] = Field(default=None, max_length=40)


class Start(BaseModel):
    max_tokens: Optional[int] = Field(default=None, ge=10_000, le=50_000_000)


class Resume(BaseModel):
    max_tokens: Optional[int] = Field(default=None, ge=10_000, le=200_000_000)
    message: Optional[str] = Field(default=None, max_length=2000)


def jobs_of(services: Any) -> Jobs:
    if getattr(services, "jobs", None) is None:
        services.jobs = Jobs(services.config, services.hermes, getattr(services, "hub", None),
                             getattr(services, "push", None), mail_scan.scans_of(services))
    return services.jobs


def jobs_router() -> APIRouter:
    router = APIRouter()

    def refused(exc: Exception) -> HTTPException:
        return HTTPException(status_code=404 if str(exc) == "unknown job" else 409, detail=str(exc))

    @router.get("/jobs")
    async def listing(request: Request) -> Dict[str, Any]:
        return {"jobs": jobs_of(request.app.state.services).list(), "budgets": BUDGETS}

    @router.post("/agents/{agent_id}/jobs")
    async def create(agent_id: str, body: NewTask, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        agent = services.config.agent(agent_id)
        if agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        try:
            return await jobs_of(services).create(agent, body.title, body.prompt, body.max_tokens, body.template)
        except JobError as exc:
            raise refused(exc) from exc

    @router.post("/jobs/{job_id}/start")
    async def start(job_id: str, request: Request, body: Optional[Start] = None) -> Dict[str, Any]:
        try:
            return await jobs_of(request.app.state.services).start(job_id, body.max_tokens if body else None)
        except JobError as exc:
            raise refused(exc) from exc

    @router.post("/jobs/{job_id}/pause")
    async def pause(job_id: str, request: Request) -> Dict[str, Any]:
        try:
            return await jobs_of(request.app.state.services).pause(job_id)
        except JobError as exc:
            raise refused(exc) from exc

    @router.post("/jobs/{job_id}/resume")
    async def resume(job_id: str, request: Request, body: Optional[Resume] = None) -> Dict[str, Any]:
        try:
            return await jobs_of(request.app.state.services).resume(
                job_id, body.max_tokens if body else None, body.message if body else None)
        except JobError as exc:
            raise refused(exc) from exc

    @router.delete("/jobs/{job_id}")
    async def forget(job_id: str, request: Request) -> Dict[str, Any]:
        try:
            await jobs_of(request.app.state.services).forget(job_id)
        except JobError as exc:
            raise refused(exc) from exc
        return {"ok": True}

    return router
