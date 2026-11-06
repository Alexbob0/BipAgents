"""Whatever the bridge sends must be displayable (the release journeys' contract layer, docs/release-journeys.md).

A timeline page is checked against what the two apps need to DRAW each item, not against the bridge's own idea of
it: the types their decoders accept (HermesKit `TimelineModels.swift`, strict Codable: one field of the wrong type and
the whole page is lost; Android `conv/Conversations.kt`, kotlinx), then what each kind needs on screen:

- a person's photo: its `url` gives the image's bytes, and its `size` (> 0) is what the bubble shows before they come
  (« 0 Ko » and no picture since 3/11: the regression that started this layer);
- a person's document: its `size` (> 0, a number) and its `path`, downloadable (`/v1/media`);
- an agent's file, page, map, audio, picture: its `path`, downloadable, not empty;
- a tool step its name and status, a notice its words, a Boîte message its words or files, an answered approval its
  command and choice, a call its record;
- `pending`: each card the fields it draws (its id, run, request; a sites card its hosts, a secret card what it asks,
  a question its words), the queue's messages their words or files.

`problems(bridge, page, agent)` returns every problem found (empty: the page is displayable); `check(...)` asserts it.
Nothing here knows how the bridge builds a page."""
from __future__ import annotations

from datetime import datetime
from typing import Any, Dict, Iterable, List, Optional
from urllib.parse import urlencode

from .common import AUTH

STR, INT, NUM, BOOL, OBJ, LIST = "string", "integer", "number", "boolean", "object", "array"

# The fields both apps decode, with the type their decoders accept (None allowed unless `required`). A field of
# another type makes iOS drop the whole page (JSONDecoder), Android too (kotlinx without isLenient).
ITEM_FIELDS = {
    "id": (STR, True), "kind": (STR, True), "at": (STR, False), "cursor": (STR, True), "turn": (STR, False),
    "role": (STR, False), "text": (STR, False), "attachments": (LIST, False), "reasoning": (STR, False),
    "tool": (OBJ, False), "notice": (OBJ, False), "title": (STR, False), "boite": (OBJ, False),
    "approval": (OBJ, False), "call": (OBJ, False), "from": (STR, False), "room": (OBJ, False), "day": (STR, False),
    "new_day": (BOOL, False), "added_during_run": (BOOL, False), "client_id": (STR, False),
    "from_name": (STR, False), "not_sent": (BOOL, False),
}
ATTACHMENT_FIELDS = {
    "id": (STR, True), "kind": (STR, True), "name": (STR, False), "mime": (STR, False), "size": (INT, False),
    "path": (STR, False), "url": (STR, False), "offset": (INT, False),
}
TOOL_FIELDS = {"name": (STR, True), "call_id": (STR, False), "arguments": (STR, False), "result": (STR, False),
               "status": (STR, False), "via": (STR, False)}
NOTICE_FIELDS = {"text": (STR, True), "code": (STR, False), "stopped": (STR, False)}
BOITE_FIELDS = {"outbox_id": (STR, True), "audio": (BOOL, False)}
RUN_FIELDS = {"run_id": (STR, True), "status": (STR, False), "started": (NUM, False)}
AGENT_FILE_KINDS = {"file", "page", "map", "audio", "picture"}


def _type_ok(value: Any, kind: str) -> bool:
    if kind == STR:
        return isinstance(value, str)
    if kind == INT:
        return isinstance(value, int) and not isinstance(value, bool)
    if kind == NUM:
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if kind == BOOL:
        return isinstance(value, bool)
    if kind == OBJ:
        return isinstance(value, dict)
    if kind == LIST:
        return isinstance(value, list)
    return True


def _fields(where: str, obj: Any, spec: Dict[str, tuple], out: List[str]) -> None:
    if not isinstance(obj, dict):
        out.append(f"{where}: not an object ({obj!r:.80})")
        return
    for name, (kind, required) in spec.items():
        if name not in obj or obj[name] is None:
            if required:
                out.append(f"{where}: `{name}` missing (the apps cannot decode it)")
            continue
        if not _type_ok(obj[name], kind):
            out.append(f"{where}: `{name}` is {type(obj[name]).__name__} {obj[name]!r:.60}, the apps decode a {kind} "
                       f"(the whole page is lost)")


def _download(bridge, url: str) -> tuple:
    """(status, bytes, content type) of a file as an app fetches it (a 307 to /v1/media followed)."""
    resp = bridge.get(url, headers=AUTH)
    if resp.status_code in (301, 302, 307, 308):
        resp = bridge.get(resp.headers["location"], headers=AUTH)
    return resp.status_code, resp.content, resp.headers.get("content-type", "")


def _media(bridge, path: str, agent: Optional[str]) -> tuple:
    query = {"path": path, **({"agent": agent} if agent else {})}
    return _download(bridge, "/v1/media?" + urlencode(query))


def _attachment(bridge, where: str, att: Dict[str, Any], role: Optional[str], agent: Optional[str],
                out: List[str], fetch: bool) -> None:
    before = len(out)
    _fields(where, att, ATTACHMENT_FIELDS, out)
    if len(out) > before:
        return
    kind, size = att["kind"], att.get("size")
    if kind == "image":
        if not (isinstance(size, int) and size > 0):
            out.append(f"{where}: photo without its size ({size!r}): the bubble shows « 0 Ko »")
        if not att.get("url"):
            out.append(f"{where}: photo without a url: never shown")
        elif fetch:
            status, body, ctype = _download(bridge, att["url"])
            if status != 200 or not body:
                out.append(f"{where}: photo {att['url']} answers {status} ({len(body)} bytes): never shown")
            elif not ctype.startswith("image/"):
                out.append(f"{where}: photo {att['url']} is {ctype!r}, not an image")
            elif isinstance(size, int) and size > 0 and size != len(body):
                out.append(f"{where}: photo's size {size} is not its bytes' ({len(body)})")
    elif kind == "document":
        if not att.get("name"):
            out.append(f"{where}: document without its name")
        if not (isinstance(size, int) and size > 0):
            out.append(f"{where}: document without its size ({size!r})")
        if not att.get("path"):
            out.append(f"{where}: document without a path: never opens")
        elif fetch:
            status, body, _ = _media(bridge, att["path"], agent)
            if status != 200 or not body:
                out.append(f"{where}: document {att['path']} answers {status} ({len(body)} bytes): never opens")
    elif kind in AGENT_FILE_KINDS:
        if not att.get("path"):
            out.append(f"{where}: {kind} without a path: never shown")
        elif fetch:
            status, body, ctype = _media(bridge, att["path"], agent)
            if status != 200 or not body:
                out.append(f"{where}: {kind} {att['path']} answers {status} ({len(body)} bytes): never opens")
            elif kind == "page" and "html" not in ctype:
                out.append(f"{where}: page {att['path']} is {ctype!r}, not a page")
    else:
        out.append(f"{where}: attachment kind {kind!r} no app draws")


def _item(bridge, item: Dict[str, Any], agent: Optional[str], out: List[str], fetch: bool) -> None:
    where = f"item {item.get('id')} ({item.get('kind')})"
    before = len(out)
    _fields(where, item, ITEM_FIELDS, out)
    if len(out) > before:
        return
    if item.get("at"):
        try:
            datetime.fromisoformat(item["at"].replace("Z", "+00:00"))
        except ValueError:
            out.append(f"{where}: `at` {item['at']!r} is not a date")
    kind = item["kind"]
    atts = item.get("attachments") or []
    for n, att in enumerate(atts):
        _attachment(bridge, f"{where} attachment {n}", att, item.get("role"), agent or item.get("from"), out, fetch)
    if kind == "message":
        if item.get("role") not in ("person", "agent", "peer"):
            out.append(f"{where}: role {item.get('role')!r}")
        if not (item.get("text") or "").strip() and not atts and not (item.get("reasoning") or "").strip():
            out.append(f"{where}: a message with nothing to draw")
        if item.get("role") == "peer" and not item.get("from"):
            out.append(f"{where}: another agent's message without `from`")
    elif kind == "tool":
        before = len(out)
        _fields(f"{where} tool", item.get("tool"), TOOL_FIELDS, out)
        if len(out) == before and item["tool"].get("status") not in (None, "done", "failed", "running"):
            out.append(f"{where}: tool status {item['tool'].get('status')!r}")
    elif kind == "notice":
        before = len(out)
        _fields(f"{where} notice", item.get("notice"), NOTICE_FIELDS, out)
        if len(out) == before and not item["notice"]["text"].strip() and not item["notice"].get("stopped"):
            out.append(f"{where}: a notice with no words")
    elif kind == "boite":
        _fields(f"{where} boite", item.get("boite"), BOITE_FIELDS, out)
        if not (item.get("text") or "").strip() and not atts:
            out.append(f"{where}: a Boîte message with nothing to draw")
    elif kind == "approval":
        approval = item.get("approval") or {}
        if not (approval.get("command") or approval.get("description") or approval.get("topic")):
            out.append(f"{where}: an answered card that says nothing of what was asked")
        if not approval.get("choice"):
            out.append(f"{where}: an answered card without its answer")
    elif kind == "call":
        if not item.get("call"):
            out.append(f"{where}: a call without its record")
    else:
        out.append(f"{where}: kind {kind!r} no app draws (left out silently)")


def card_problems(card: Dict[str, Any], where: str) -> List[str]:
    """What a waiting card (`pending.approvals`) needs to be drawn and answered."""
    out: List[str] = []
    for name in ("id", "run_id", "request_id"):
        if not isinstance(card.get(name), str) or not card[name]:
            out.append(f"{where}: `{name}` missing: the card cannot be answered or replaced")
    run = str(card.get("run_id") or "")
    if run.startswith("bip-site-"):
        hosts = card.get("bip_sites")
        if not (isinstance(hosts, list) and hosts and all(isinstance(h, str) and h for h in hosts)):
            out.append(f"{where}: a sites card without its hosts ({hosts!r})")
    elif run.startswith("bip-secret-"):
        if not (card.get("description") or card.get("command") or card.get("bip_secret")):
            out.append(f"{where}: a secret card that says nothing of what it asks")
    elif not (card.get("command") or card.get("description") or card.get("topic")):
        out.append(f"{where}: a card that says nothing of what it asks")
    choices = card.get("choices")
    if choices is not None and not isinstance(choices, list):
        out.append(f"{where}: `choices` is {type(choices).__name__}")
    return out


def question_problems(question: Dict[str, Any], where: str) -> List[str]:
    out: List[str] = []
    for name in ("id", "run_id", "request_id"):
        if not isinstance(question.get(name), str) or not question[name]:
            out.append(f"{where}: `{name}` missing: the question cannot be answered")
    asked = question.get("questions")
    words = question.get("question")
    if isinstance(asked, list) and asked:
        for n, q in enumerate(asked):
            if not isinstance(q, dict) or not str(q.get("question") or "").strip():
                out.append(f"{where}: question {n} has no words")
    elif not str(words or "").strip():
        out.append(f"{where}: a question with no words")
    return out


def pending_problems(pending: Any) -> List[str]:
    out: List[str] = []
    if pending is None:
        return out
    if not isinstance(pending, dict):
        return [f"pending: not an object ({pending!r:.60})"]
    if pending.get("run") is not None:
        _fields("pending.run", pending["run"], RUN_FIELDS, out)
    for n, card in enumerate(pending.get("approvals") or []):
        out += card_problems(card, f"pending.approvals[{n}]")
    for n, question in enumerate(pending.get("questions") or []):
        out += question_problems(question, f"pending.questions[{n}]")
    queue = pending.get("queue")
    if queue is not None:
        if not isinstance(queue, dict):
            out.append("pending.queue: not an object")
        else:
            for n, waiting in enumerate(queue.get("items") or []):
                if not isinstance(waiting, dict) or not waiting.get("id"):
                    out.append(f"pending.queue.items[{n}]: no id")
                elif not str(waiting.get("text") or "").strip() and not waiting.get("attachments") \
                        and not waiting.get("kinds"):
                    out.append(f"pending.queue.items[{n}]: « En attente » with nothing to show")
    return out


def problems(bridge, page: Dict[str, Any], agent: Optional[str] = None, fetch: bool = True) -> List[str]:
    """Every reason an app would not draw (part of) this timeline page. `fetch`: download each file too."""
    out: List[str] = []
    if not isinstance(page.get("items"), list):
        return [f"page: no items ({str(page)[:120]})"]
    for item in page["items"]:
        _item(bridge, item, agent, out, fetch)
    return out + pending_problems(page.get("pending"))


def check(bridge, page: Dict[str, Any], agent: Optional[str] = None, fetch: bool = True,
          only: Optional[Iterable[str]] = None) -> None:
    """Asserts the page is displayable (`only`: the items with these ids)."""
    if only is not None:
        wanted = set(only)
        page = {**page, "items": [i for i in page.get("items", []) if i.get("id") in wanted]}
    found = problems(bridge, page, agent, fetch)
    assert not found, "not displayable:\n- " + "\n- ".join(found)
