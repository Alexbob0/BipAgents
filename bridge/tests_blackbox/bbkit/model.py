"""The model behind the fake Hermes for the release journeys (docs/release-journeys.md): a run started with
`POST /v1/runs` is ANSWERED, as a real agent's would be, instead of following a script a test wrote beforehand.

Installed as `FakeHermes.override`, it takes only the runs it starts (every other route stays the fake Hermes'):

- the person's message is stored in the session as Hermes v0.22.5 stores it (`/api/sessions/{id}/messages`: words as
  a string; photos as `image_url` parts of the same row, their data URLs as sent);
- the reply streams word by word (`message.delta`, `word_gap` seconds apart), then is stored, then `run.completed`;
- what the words ask (a marker anywhere in them) changes the turn:

| Marker | The turn |
|---|---|
| (none) | « Bien reçu » and what came: N photos, N documents |
| `#fichier` | writes a PDF in its files and names it (`MEDIA:` line) |
| `#page` | makes a page with the bridge's `create_page` tool, names it |
| `#carte` | a map with the bridge's `map_image` tool (else a picture in the maps' folder), names it |
| `#outil` | a tool call (`tool.started` / `tool.completed`, stored as Hermes' tool rows) before its words |
| `#question` | asks a question (`clarify.request`) and waits for the answer, then says it |
| `#approbation` | asks to run a command (`approval.request`), waits, says what was decided |
| `#secret` | calls the bridge's `ask_secret` (a card in the conversation) and waits for the person |
| `#sites` | asks the bridge for two sites (`sandbox_sites_request`, a sites card) and waits |
| `#connecteur` | registers a connector (`connector_register`, a connector card) and waits |
| `#lent` | a long reply, slowly (to be stopped, or to queue messages behind it) |

`calls`: the agent's own tool calls to the bridge (MCP), given by the harness (`secret`, `sites`, `page`, `map`), each
a function of the run's words returning the text of its result. They run in the run's thread, while the turn waits."""
from __future__ import annotations

import asyncio
import json
import os
import re
import threading
import time
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from starlette.responses import JSONResponse, StreamingResponse

from .common import sse
from .fakes import split_agent, tiny_png

NOTES = re.compile(r"<!--\s*bip:[\s\S]*?(?:-->|$)")
DOC_LINE = re.compile(r"\[Pièce jointe : (.+?) \(")


def minimal_pdf(title: str) -> bytes:
    """A one-page PDF saying `title` (opens in Quick Look / Android's viewer)."""
    text = title.replace("(", "").replace(")", "")
    stream = f"BT /F1 24 Tf 72 720 Td ({text}) Tj ET".encode("latin-1", "replace")
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
               b"/Resources << /Font << /F1 5 0 R >> >> >>",
               b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for n, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += f"{n} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{o:010d} 00000 n \n".encode() for o in offsets)
    out += f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(out)


class Run:
    def __init__(self, run_id: str, agent: str, session: str, words: str, photos: int, documents: List[str]):
        self.id, self.agent, self.session = run_id, agent, session
        self.words, self.photos, self.documents = words, photos, documents
        self.events: List[str] = []
        self.cond = threading.Condition()
        self.done = False
        self.status = "running"
        self.stopped = threading.Event()
        self.answer: Optional[Dict[str, Any]] = None
        self.answered = threading.Event()
        self.pending: List[Dict[str, Any]] = []   # Hermes' `pending_controls` while it waits
        self.output = ""

    def emit(self, event: str, **data: Any) -> None:
        with self.cond:
            self.events.append(sse(event, run_id=self.id, **data))
            self.cond.notify_all()

    def finish(self) -> None:
        with self.cond:
            self.done = True
            self.cond.notify_all()


class ScriptedModel:
    """See the module's docstring. `files_dir` (host) is the agent's files folder the bridge serves under
    `files_root` (`[media] roots`)."""

    def __init__(self, hermes, files_dir: Path, files_root: str = "/home/hermes/.hermes/fichiers",
                 word_gap: float = 0.06, calls: Optional[Dict[str, Callable[[str], str]]] = None,
                 maps_dir: Optional[Path] = None):
        self.hermes = hermes
        self.files_dir, self.files_root = Path(files_dir), files_root.rstrip("/")
        self.files_dir.mkdir(parents=True, exist_ok=True)
        self.maps_dir = maps_dir
        self.word_gap = word_gap
        self.calls = calls or {}
        self.runs: Dict[str, Run] = {}
        self.count = 0
        self.lock = threading.Lock()
        self.inputs: List[Dict[str, Any]] = []   # every run's input as it came (the black-box checks read it)

    def install(self) -> "ScriptedModel":
        self.hermes.override = self.route
        return self

    # -- Hermes' run routes ----------------------------------------------------------------------------------------
    async def route(self, seen):
        agent, path = split_agent(seen.path)
        parts = path.strip("/").split("/")
        if parts[:2] != ["v1", "runs"]:
            return None
        expected = self.hermes.keys.get(agent)
        if expected is not None and seen.header("authorization") != f"Bearer {expected}":
            return None   # the fake's own answer (401)
        if len(parts) == 2 and seen.method == "POST":
            return self.start(agent, seen.json() or {})
        run = self.runs.get(parts[2]) if len(parts) > 2 else None
        if run is None:
            return None
        if len(parts) == 4 and parts[3] == "events":
            return StreamingResponse(self.stream(run), media_type="text/event-stream")
        if len(parts) == 4 and parts[3] in ("approval", "clarify") and seen.method == "POST":
            run.answer = {"kind": parts[3], **(seen.json() or {})}
            run.answered.set()
            kind = "approval_response" if parts[3] == "approval" else "clarify_response"
            return JSONResponse({"object": f"hermes.run.{kind}", "resolved": 1})
        if len(parts) == 4 and parts[3] == "steer" and seen.method == "POST":
            body = seen.json() or {}
            self.hermes.steers.append({"agent": agent, "run_id": run.id, "body": body})
            if run.done or run.status != "running":
                return JSONResponse({"error": {"code": "run_not_accepting_steer"}}, status_code=409)
            return JSONResponse({"object": "hermes.run.steer", "run_id": run.id, "accepted": True}, status_code=202)
        if len(parts) == 4 and parts[3] == "stop" and seen.method == "POST":
            run.stopped.set()
            run.answered.set()
            return JSONResponse({"status": "stopping"})
        if len(parts) == 3 and seen.method == "GET":
            status = {"status": run.status, "pending_controls": list(run.pending)}
            if run.done:
                status.update(output=run.output, usage={"input_tokens": 1200, "output_tokens": 80})
            return JSONResponse(status)
        return None

    def start(self, agent: str, body: Dict[str, Any]):
        session = str(body.get("session_id") or "")
        raw = body.get("input")
        self.inputs.append({"agent": agent, "session": session, "input": raw})
        if isinstance(raw, list):   # [{role: user, content: parts}]
            content = raw[-1].get("content") if raw and isinstance(raw[-1], dict) else raw
        else:
            content = raw if isinstance(raw, str) else ""
        words = content if isinstance(content, str) else "\n".join(
            p.get("text", "") for p in content if isinstance(p, dict) and p.get("type") == "text")
        photos = 0 if isinstance(content, str) else sum(
            1 for p in content if isinstance(p, dict) and p.get("type") in ("image_url", "input_image"))
        with self.lock:
            self.count += 1
            run_id = f"run_j{self.count}"
        visible = NOTES.sub("", words)
        run = Run(run_id, agent, session, visible, photos, DOC_LINE.findall(visible))
        self.runs[run_id] = run
        self.store(agent, session, {"role": "user", "content": content, "timestamp": time.time()})
        threading.Thread(target=self.answer, args=(run,), name=f"model-{run_id}", daemon=True).start()
        return JSONResponse({"run_id": run_id, "status": "started"}, status_code=202)

    async def stream(self, run: Run):
        """Every event of the run from its start (a second listener gets them all too), as they come."""
        sent = 0
        while True:
            with run.cond:
                fresh, done = run.events[sent:], run.done
            for event in fresh:
                yield event.encode("utf-8")
            sent += len(fresh)
            if done and sent >= len(run.events):
                return
            await asyncio.sleep(0.02)

    # -- the session ---------------------------------------------------------------------------------------------
    def store(self, agent: str, session: str, row: Dict[str, Any]) -> None:
        rows = self.hermes.messages.setdefault((agent, session), [])
        rows.append(row)
        meta = self.hermes.sessions.setdefault(agent, {}).setdefault(session, {"id": session, "title": "Discussion"})
        meta["message_count"] = len(rows)
        meta["last_active"] = time.time()

    # -- the turn ------------------------------------------------------------------------------------------------
    def say(self, run: Run, text: str, gap: Optional[float] = None) -> str:
        """Streams `text` word by word; stops early when the run is stopped. Returns what was said."""
        said = ""
        for n, word in enumerate(re.findall(r"\S+\s*|\n+", text)):
            if run.stopped.is_set():
                break
            run.emit("message.delta", delta=word)
            said += word
            time.sleep(self.word_gap if gap is None else gap)
        return said

    def wait(self, run: Run, kind: str, prompt_id: str, **event: Any) -> Optional[Dict[str, Any]]:
        run.pending = [{"kind": kind, "prompt_id": prompt_id, "execution_generation": 1,
                        "choices": event.get("choices", ["once", "deny"])}]
        run.status = "waiting_for_approval"
        run.emit(f"{kind}.request" if kind == "approval" else "clarify.request", request_id=prompt_id,
                 prompt_id=prompt_id, execution_generation=1, **event)
        run.answered.wait(timeout=1800)
        run.pending, run.status = [], "running"
        if run.stopped.is_set():
            return None
        run.emit(f"{'approval' if kind == 'approval' else 'clarify'}.settled", request_id=prompt_id,
                 prompt_id=prompt_id, execution_generation=1)
        return run.answer

    def tool(self, run: Run, name: str, arguments: Dict[str, Any], result: Callable[[], str], calls: List[Dict],
             results: List[Dict]) -> str:
        call_id = f"call_{run.id}_{len(calls)}"
        run.emit("tool.started", tool=name, preview=json.dumps(arguments, ensure_ascii=False)[:120])
        try:
            text = result()
            failed = False
        except Exception as exc:  # noqa: BLE001 - a tool that fails is a failed step, not a crashed run
            text, failed = f"Error: {exc}", True
        run.emit("tool.completed", tool=name, error=failed)
        calls.append({"id": call_id, "type": "function",
                      "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)}})
        results.append({"role": "tool", "tool_call_id": call_id, "tool_name": name, "content": text,
                        "timestamp": time.time()})
        return text

    def media_line(self, text: str) -> str:
        found = re.search(r"MEDIA:\s*(\S+)", text)
        return f"MEDIA:{found.group(1)}" if found else ""

    def answer(self, run: Run) -> None:
        try:
            self._answer(run)
        except Exception as exc:  # noqa: BLE001 - the run ends failed, never hangs
            run.emit("run.failed", error=str(exc))
            run.status = "failed"
            run.finish()

    def _answer(self, run: Run) -> None:
        words = run.words.lower()
        calls: List[Dict] = []
        results: List[Dict] = []
        lines: List[str] = []   # the stored reply
        opening = ""
        if "#outil" in words:
            opening = self.say(run, "Je cherche ça. ")
            self.tool(run, "web_search", {"query": "horaires piscine"},
                      lambda: "3 résultats : piscine Jean-Bouin, ouverte de 7 h à 21 h.", calls, results)
        if "#secret" in words and "secret" in self.calls:
            opening = self.say(run, "Il me faut ton identifiant, je te le demande. ")
            self.tool(run, "mcp_bipagents_connectors_ask_secret", {"name": "exemple", "site": "exemple.com"},
                      lambda: self.calls["secret"](run.words), calls, results)
        if "#sites" in words and "sites" in self.calls:
            opening = self.say(run, "Je demande l’accès à deux sites. ")
            self.tool(run, "mcp_bipagents_files_sandbox_sites_request", {"host": "connect.garmin.example"},
                      lambda: self.calls["sites"](run.words), calls, results)
        if "#connecteur" in words and "connector" in self.calls:
            opening = self.say(run, "J’installe le connecteur Garmin, il te faut l’autoriser. ")
            self.tool(run, "mcp_bipagents_connectors_connector_register", {"name": "Garmin"},
                      lambda: self.calls["connector"](run.words), calls, results)
        if "#question" in words:
            opening = self.say(run, "Une question d’abord. ")
            answer = self.wait(run, "clarify", f"clr_{run.id}", question="Quel train préfères-tu ?",
                               choices=["Le 9 h", "Le 14 h"], multi_select=False)
            chosen = (answer or {}).get("answer") or "rien"
            lines.append(f"Noté : {chosen}.")
        if "#approbation" in words:
            opening = self.say(run, "Je dois lancer une commande. ")
            answer = self.wait(run, "approval", f"apr_{run.id}", command="rm -rf ~/brouillons",
                               description="Supprimer le dossier des brouillons", choices=["once", "deny"])
            choice = (answer or {}).get("choice") or "deny"
            lines.append("C’est fait, le dossier est supprimé." if choice != "deny" else "D’accord, je n’y touche pas.")
        media: List[str] = []
        if "#fichier" in words:
            name = f"rapport-{run.id}.pdf"
            (self.files_dir / name).write_bytes(minimal_pdf(f"Rapport {run.id}"))
            lines.append("Voici le rapport du jour.")
            media.append(f"MEDIA:{self.files_root}/{name}")
        if "#page" in words:
            made = self.tool(run, "mcp_bipagents_create_page", {"title": f"Résumé {run.id}"},
                             lambda: self.calls["page"](run.id), calls, results) if "page" in self.calls else ""
            line = self.media_line(made)
            if line:
                lines.append("J’ai préparé une page.")
                media.append(line)
        if "#carte" in words:
            made = self.tool(run, "mcp_bipagents_map_image", {"markers": [{"place": "Tour Eiffel"}], "area": "Paris"},
                             lambda: self.calls["map"](run.id), calls, results) if "map" in self.calls else ""
            line = self.media_line(made)
            if line:
                lines.append("Voici la carte.")
                media.append(line)
        if "#lent" in words:
            lines.append(" ".join(["Je prends le temps de bien tout vérifier, point par point."] * 12))
        if not lines:
            what = []
            if run.photos:
                what.append(f"{run.photos} photo{'s' if run.photos > 1 else ''}")
            if run.documents:
                what.append(f"{len(run.documents)} document{'s' if len(run.documents) > 1 else ''}")
            lines.append("Bien reçu" + (f" : {' et '.join(what)}." if what else ".")
                         + " Merci, je regarde ça tout de suite et je te réponds en détail très bientôt.")
        body = " ".join(lines)
        said = self.say(run, body, gap=0.25 if "#lent" in words else None)
        if run.stopped.is_set():
            if calls:
                self.store(run.agent, run.session, {"role": "assistant", "content": opening, "tool_calls": calls,
                                                    "timestamp": time.time()})
                for row in results:
                    self.store(run.agent, run.session, row)
            if said or opening:
                self.store(run.agent, run.session, {"role": "assistant", "content": opening + said,
                                                    "timestamp": time.time()})
            run.status = "cancelled"
            run.emit("run.cancelled")
            run.finish()
            return
        for line in media:   # the files' lines come last, whole (the bridge holds a line until its newline)
            run.emit("message.delta", delta="\n" + line)
        reply = said + "".join("\n" + line for line in media)
        if calls:
            self.store(run.agent, run.session, {"role": "assistant", "content": opening, "tool_calls": calls,
                                                "timestamp": time.time()})
            for row in results:
                self.store(run.agent, run.session, row)
            self.store(run.agent, run.session, {"role": "assistant", "content": reply, "timestamp": time.time()})
        else:
            self.store(run.agent, run.session, {"role": "assistant", "content": opening + reply,
                                                "timestamp": time.time()})
        run.status, run.output = "completed", opening + reply
        run.emit("run.completed", output=opening + reply, usage={"input_tokens": 1200, "output_tokens": 80})
        run.finish()


def picture(path: Path) -> Path:
    """A small PNG at `path` (a map's picture when the map tool is not reachable)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(tiny_png(320, 200, (0x8F, 0xC1, 0xE9)))
    os.chmod(path, 0o644)
    return path
