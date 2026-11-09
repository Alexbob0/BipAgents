"""The model behind the fake Hermes for the release journeys (docs/release-journeys.md): a run started with
`POST /v1/runs` is ANSWERED, as a real agent's would be, instead of following a script a test wrote beforehand.

Installed as `FakeHermes.override`, it takes only the runs it starts (every other route stays the fake Hermes'):

- the person's message is stored in the session as Hermes v0.22.5 stores it (`durable`: text only; each photo a file
  of the profile, `[Image attached at: <path as Hermes sees it>]` and a `[screenshot]` line; without a profile given,
  the parts as sent, data URLs included);
- the reply streams word by word (`message.delta`, `word_gap` seconds apart), then is stored, then `run.completed`;
- what the words ask (a marker anywhere in them) changes the turn:

| Marker | The turn |
|---|---|
| (none) | « Bien reçu » and what came: N photos, N documents |
| `#fichier` | writes a PDF in its files and names it (`MEDIA:` line) |
| `#page` | makes a page with the bridge's `create_page` tool, names it |
| `#carte` | a map with the bridge's `map_image` tool (else a picture in the maps' folder), names it |
| `#trajet` | a map with `map_image`, its card pasted into a `create_page` page, names the page only (as agents do) |
| `#outil` | a tool call (`tool.started` / `tool.completed`, stored as Hermes' tool rows) before its words |
| `#question` | asks a question (`clarify.request`) and waits for the answer, then says it |
| `#approbation` | asks to run a command (`approval.request`), waits, says what was decided |
| `#secret` | calls the bridge's `ask_secret` (a card in the conversation) and waits for the person |
| `#sites` | asks the bridge for two sites (`sandbox_sites_request`, a sites card) and waits |
| `#connecteur` | registers a connector (`connector_register`, a connector card) and waits |
| `#podcast` | makes its podcast (an .m4a in its files, as `create_podcast` gives) and names it |
| `#lent` | a long reply, slowly (to be stopped, or to queue messages behind it) |

`calls`: the agent's own tool calls to the bridge (MCP), given by the harness (`secret`, `sites`, `page`, `map`,
`page_with`: a page of the given HTML), each a function of the run's words returning the text of its result. They run
in the run's thread, while the turn waits."""
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
                 maps_dir: Optional[Path] = None, profile: Optional[Path] = None,
                 profile_seen_as: Optional[str] = None):
        self.hermes = hermes
        # where Hermes keeps the agent's profile: on this machine (`profile`), and as Hermes names it in what it
        # stores (`profile_seen_as`: its container's path for an isolated agent, as on the offer's places)
        self.profile = Path(profile) if profile else None
        self.profile_seen_as = (profile_seen_as or (str(profile) if profile else "")).rstrip("/")
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
        self.store(agent, session, {"role": "user", "content": self.durable(content), "timestamp": time.time()})
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
    def durable(self, content: Any) -> Any:
        """The person's row as Hermes v0.22.5 keeps it (its API turn: gateway/session_api_media.py, then
        agent/session_persistence.py `_durable_content`): each inline photo committed as a file of the profile
        (`native-inputs/<sha256>/api_<sha256[:32]>.<ext>`) and named in the words (`[Image attached at: <path>]`, the
        path as HERMES sees it), the photo itself a `[screenshot]` line; a photo sent with no words gets Hermes' own
        « What do you see in this image? ». Text only: no data URL stays in the row. Without a profile: as sent."""
        if not isinstance(content, list) or self.profile is None:
            return content
        import base64
        import hashlib
        hints = []
        for part in content:
            if not isinstance(part, dict) or part.get("type") not in ("image_url", "input_image"):
                continue
            ref = part.get("image_url")
            url = ref.get("url", "") if isinstance(ref, dict) else str(ref or "")
            head, _, data = url.partition(",")
            if not head.startswith("data:image/") or ";base64" not in head:
                hints.append(f"[Image attached: {url}]")
                continue
            raw = base64.b64decode(data)
            ext = {"image/png": ".png", "image/gif": ".gif", "image/webp": ".webp"}.get(head[5:].split(";")[0], ".jpg")
            digest = hashlib.sha256(raw).hexdigest()
            name = f"api_{digest[:32]}{ext}"
            folder = self.profile / "native-inputs" / digest
            folder.mkdir(parents=True, exist_ok=True)
            (folder / name).write_bytes(raw)
            hints.append(f"[Image attached at: {self.profile_seen_as}/native-inputs/{digest}/{name}]")
        texts = [p.get("text", "") for p in content if isinstance(p, dict) and p.get("type") == "text"]
        words = (texts[0] if texts else "What do you see in this image?") + ("\n\n" + "\n".join(hints) if hints else "")
        lines = [words] + texts[1:] + ["[screenshot]"] * len(hints)
        return "\n".join(lines)

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
        if "#podcast" in words:
            name = f"podcast-{run.id}.m4a"
            (self.files_dir / name).write_bytes(b"\x00\x00\x00\x18ftypM4A podcast")
            lines.append("Ton podcast est prêt.")
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
        if "#trajet" in words and "map" in self.calls and "page_with" in self.calls:
            # as the agents are told (VISUAL_RULE, create_page): the map's card pasted into the trip's page, whose
            # line alone the reply gives (7/11: the bridge added the map's own page at the end, the map twice)
            markers = {"markers": [{"place": "Tour Eiffel"}, {"place": "Louvre"}], "area": "Paris"}
            made = self.tool(run, "mcp_bipagents_map_image", markers, lambda: self.calls["map"](run.id), calls, results)
            card = re.search(r'<figure class="map-card">.*?</figure>', made)
            html = f"<h1>Ton trajet</h1><p>Deux étapes à pied.</p>{card.group(0) if card else ''}<p>Bonne balade !</p>"
            page = self.tool(run, "mcp_bipagents_create_page", {"html": html, "title": f"Trajet {run.id}"},
                             lambda: self.calls["page_with"](run.id, html), calls, results)
            line = self.media_line(page)
            if line:
                lines.append("Voici ton trajet.")
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


# -- the release journeys J32+ (people and this release's features): more markers, in a model of their own ----------

class PeopleModel(ScriptedModel):
    """The scripted model with this release's turns (test_journeys_people.py, test_journeys_features.py). Every marker
    of `ScriptedModel` still works; these ones call the bridge's own tools as the agent (`calls[name](run)`, the run
    given: its agent, its words), each a step of the turn, then say what was done:

| Marker | The turn |
|---|---|
| `#routine` | creates a routine (`routine_schedule`), then says so: the routine's one question comes in this turn |
| `#tache` | starts a small background task (`propose_task`, « small »): a choice the approval level may take |
| `#famille` | proposes to tell the family (`tell_family`): nothing leaves before the person's « Envoyer » |
| `#etapes` | three useful steps (a web search, the weather, the agenda) and a calculation, then its own list (`todo_list` + `todo.updated`) |
| `#relais` | writes to another agent (`message_agent`) five times in one turn (the hop cap) |

    A run started with `message` instead of `input` (another household's agent through `/peer/…`) is read the same."""

    MARKERS = ("#routine", "#tache", "#famille", "#etapes", "#relais")

    @classmethod
    def upgrade(cls, model: ScriptedModel) -> "PeopleModel":
        """The same model (its runs, files, calls), answering with these markers too; installed in its place."""
        made = cls.__new__(cls)
        made.__dict__.update(model.__dict__)
        made.step_results: List[Dict[str, Any]] = []
        return made.install()

    def start(self, agent: str, body: Dict[str, Any]):
        if body.get("input") is None and isinstance(body.get("message"), str):
            body = {**body, "input": body["message"]}
        return super().start(agent, body)

    def _answer(self, run: Run) -> None:
        words = run.words.lower()
        if not any(m in words for m in self.MARKERS):
            return super()._answer(run)
        calls: List[Dict] = []
        results: List[Dict] = []
        lines: List[str] = []
        opening = ""

        def step(name: str, tool: str, arguments: Dict[str, Any]) -> str:
            made = self.tool(run, tool, arguments, lambda: self.calls[name](run), calls, results)
            self.step_results.append({"run": run.id, "tool": tool, "result": made})
            return made

        if "#routine" in words and "routine" in self.calls:
            opening += self.say(run, "Je crée la routine. ")
            step("routine", "mcp_bipagents_routine_schedule", {"action": "create", "name": "Le point du soir"})
            lines.append("C’est fait, la routine « Le point du soir » est créée.")
        if "#tache" in words and "task" in self.calls:
            opening += self.say(run, "Je lance une petite tâche de fond. ")
            made = step("task", "mcp_bipagents_propose_task", {"title": "Dossier location", "budget": "small"})
            lines.append("La tâche est lancée." if "Started" in made or "lancée" in made else "Je te demande avant de la lancer.")
        if "#famille" in words and "family" in self.calls:
            opening += self.say(run, "Je prépare un mot pour la famille. ")
            step("family", "mcp_bipagents_tell_family", {"to": ["marie"]})
            lines.append("Je te propose de prévenir Marie : rien ne part sans toi.")
        if "#etapes" in words:
            opening += self.say(run, "Je m’organise. ")
            self.tool(run, "web_search", {"query": "météo du week-end"}, lambda: "Beau temps samedi.", calls, results)
            self.tool(run, "execute_code", {"code": "print(21 - 3)"}, lambda: "18", calls, results)   # backstage
            self.tool(run, "mcp_bipagents_weather", {"place": "Lyon"}, lambda: "Samedi : 21 °C, soleil.", calls, results)
            # the automatic « Étapes » card shows from its 3rd useful step (the calculation doesn't count)
            self.tool(run, "mcp_bipagents_calendar_list", {"days": 2}, lambda: "Samedi : libre.", calls, results)
            time.sleep(1.0)
            todos = [{"id": "1", "content": "Regarder la météo", "status": "completed"},
                     {"id": "2", "content": "Choisir le parc", "status": "completed"},
                     {"id": "3", "content": "Préparer le pique-nique", "status": "in_progress"}]
            summary = {"total": 3, "pending": 0, "in_progress": 1, "completed": 2, "cancelled": 0}
            self.tool(run, "todo_list", {"todos": todos},
                      lambda: json.dumps({"todos": todos, "revision": 1, "summary": summary}), calls, results)
            run.emit("todo.updated", todos=todos, revision=1, summary=summary)
            time.sleep(1.0)
            lines.append("Samedi s’annonce beau : pique-nique au parc de la Tête d’Or.")
        if "#relais" in words and "relay" in self.calls:
            opening += self.say(run, "Je demande à Coach. ")
            for n in range(5):
                try:
                    step("relay", "mcp_bipagents_message_agent", {"target": "bip/coach", "n": n + 1})
                except Exception as exc:  # noqa: BLE001 - the 5th is refused: the tool's error is the step's result
                    self.step_results.append({"run": run.id, "tool": "message_agent", "error": str(exc)})
            lines.append("J’ai échangé avec Coach, je te tiens au courant.")
        body = " ".join(lines) or "Bien reçu."
        said = self.say(run, body)
        reply = said
        if calls:
            self.store(run.agent, run.session, {"role": "assistant", "content": opening, "tool_calls": calls,
                                                "timestamp": time.time()})
            for row in results:
                self.store(run.agent, run.session, row)
            self.store(run.agent, run.session, {"role": "assistant", "content": reply, "timestamp": time.time()})
        else:
            self.store(run.agent, run.session, {"role": "assistant", "content": opening + reply, "timestamp": time.time()})
        run.status, run.output = "completed", opening + reply
        run.emit("run.completed", output=opening + reply, usage={"input_tokens": 1200, "output_tokens": 80})
        run.finish()
