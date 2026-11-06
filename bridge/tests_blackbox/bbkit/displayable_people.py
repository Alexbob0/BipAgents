"""What friends, relatives and this release's new screens must be able to DRAW (the contract layer of the release
journeys J32+, docs/release-journeys.md), beside `displayable.py` (the timeline page): each check reads what the apps'
decoders take and what they put on screen, and returns the problems found (empty: drawable).

- a shared conversation's items as the friend's phone opens them (HermesKit `ReadShareItem.parse`, Android
  `ViewerShares`): the item names its share and its id (else dropped), a message its role and words, a page its html, a
  map page its places with coordinates (drawn as the native map card: `MapCard.data(inPage:)`, id `p-…-carte-<hex>`),
  a photo bytes that decode;
- the owner's view of the share (`ReadShareView`, strict Codable: `ends` a number, `viewers`/`invites` lists);
- family screens (`/v1/family`, `/v1/shared-agents`, `/v1/family/news`, `/v1/agent-pairs`): what each row names;
- this release's cards and lines: « Validé pour toi » lines (`/v1/approvals/auto`), the routine's one question
  (`bip_routine`), « Étapes » (`kind: steps`), « Continuer l'échange ? » (`bip_hops`), a quick talk's send state.

`check_page(bridge, page, agent)` is `displayable.check` that also knows the `steps` items (drawn by both apps since
this release: StepsCard.swift, StepsCard.kt), which `displayable.py` does not list yet."""
from __future__ import annotations

import base64
import json
import re
from datetime import datetime
from typing import Any, Dict, Iterable, List, Optional

from . import displayable

MAP_DATA = re.compile(r'<script type="application/json" class="map-data">(.*?)</script>', re.S)
MAP_ONLY = re.compile(r"p-.+-carte-[0-9a-f]{6,40}")
STEP_STATUS = {"done", "in_progress", "pending", "cancelled"}


def _assert(found: List[str], what: str) -> None:
    assert not found, f"{what} not drawable:\n- " + "\n- ".join(found)


def _num(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _iso(value: Any) -> bool:
    try:
        datetime.fromisoformat(str(value).replace("Z", "+00:00"))
        return True
    except ValueError:
        return False


def _text(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


# -- a shared conversation, on the friend's phone -------------------------------------------------------------------

def map_places(html: str) -> List[Dict[str, Any]]:
    """The places of a map page as the apps read them (`.map-data`), those with coordinates."""
    found = MAP_DATA.search(html or "")
    if not found:
        return []
    try:
        data = json.loads(found.group(1))
    except ValueError:
        return []
    if data.get("type", "map") != "map":
        return []
    out = []
    for p in data.get("points") or []:
        try:
            lat, lon = float(p.get("lat")), float(p.get("lon"))
        except (TypeError, ValueError):
            continue
        if -90 <= lat <= 90 and -180 <= lon <= 180:
            out.append(p)
    return out


def share_item_problems(items: List[Dict[str, Any]], share: str) -> List[str]:
    """`items`: as `people.Friend.items` gives them (the guichet's listing, each `opened`)."""
    out: List[str] = []
    by_id = {i["id"]: i for i in items}
    if "meta" not in by_id:
        out.append("no `meta` item: the thread has no title nor who shows it")
    for item in items:
        where = f"item {item['id']} ({item.get('kind')})"
        opened = item.get("opened")
        if opened is None:
            out.append(f"{where}: does not open with the content key")
            continue
        if opened.get("s") != share or opened.get("id") != item["id"]:
            out.append(f"{where}: names share {opened.get('s')!r} / id {opened.get('id')!r}: dropped by the apps")
            continue
        kind = opened.get("kind") or item.get("kind")
        if kind == "meta":
            if not _text(opened.get("title")) or not _text(opened.get("from")):
                out.append(f"{where}: no title or no « from »: « {opened.get('from')} te montre un projet »")
        elif kind == "msg":
            if opened.get("role") not in ("user", "assistant"):
                out.append(f"{where}: role {opened.get('role')!r}: dropped")
            media = opened.get("media") or []
            if not _text(opened.get("text")) and not media:
                out.append(f"{where}: a message with nothing to draw")
            if opened.get("at") is not None and not _num(opened.get("at")):
                out.append(f"{where}: `at` is {type(opened.get('at')).__name__}")
            for m in media:
                if m not in by_id:
                    out.append(f"{where}: names {m!r}, not in the share: an empty card")
        elif kind == "page":
            if not _text(opened.get("html")):
                out.append(f"{where}: a page without its html")
            if opened.get("map") and not map_places(opened.get("html", "")):
                out.append(f"{where}: a map page without places with coordinates: no map card")
        elif kind == "photo":
            try:
                raw = base64.b64decode(opened.get("data") or "", validate=True)
            except ValueError:
                raw = b""
            if not raw:
                out.append(f"{where}: a photo without its bytes")
        elif kind == "card":
            if opened.get("type") not in ("vote", "checklist") or not isinstance(opened.get("options"), list):
                out.append(f"{where}: a card the apps cannot read")
        else:
            out.append(f"{where}: kind {kind!r} no app draws")
    return out


def check_share_items(items: List[Dict[str, Any]], share: str) -> None:
    _assert(share_item_problems(items, share), "the shared conversation is")


def owner_view_problems(view: Dict[str, Any]) -> List[str]:
    """`ReadShareView` (HermesKit, strict Codable) and Android's `OwnerShares.View`: « Visible par N personnes »."""
    out: List[str] = []
    for name in ("id", "agent"):
        if not _text(view.get(name)):
            out.append(f"read share: `{name}` missing")
    if not _num(view.get("ends")):
        out.append(f"read share: `ends` is {view.get('ends')!r}, the iPhone decodes a number (the view is lost)")
    if view.get("count") is not None and not (isinstance(view["count"], int) and not isinstance(view["count"], bool)):
        out.append(f"read share: `count` is {view.get('count')!r}")
    for name in ("viewers", "invites"):
        if not isinstance(view.get(name), list):
            out.append(f"read share: `{name}` is not a list (the iPhone loses the view)")
    if view.get("session") is not None and not isinstance(view["session"], str):
        out.append(f"read share: `session` is {view['session']!r} (a conversation's id, or null for a project's)")
    for v in view.get("viewers") or []:
        if not (_text(v.get("id")) and isinstance(v.get("name"), str) and _text(v.get("state"))):
            out.append(f"read share: viewer {v!r:.80} without id, name or state")
    for i in view.get("invites") or []:
        if not (_text(i.get("invite")) and isinstance(i.get("name"), str)):
            out.append(f"read share: invitation {i!r:.80} without invite or name (the iPhone loses the view)")
    return out


# -- family screens -------------------------------------------------------------------------------------------------

def family_problems(family: Dict[str, Any], other: str) -> List[str]:
    """Réglages › Famille: « Ton foyer » (its name), « Proches reliés » (each household's name)."""
    out: List[str] = []
    if not _text(family.get("name")):
        out.append("family: no name for « Ton foyer »")
    rows = family.get("households")
    if not isinstance(rows, list):
        return out + ["family: `households` is not a list"]
    if not [h for h in rows if _text(h.get("id")) and h.get("name") == other]:
        out.append(f"family: « {other} » is not among « Proches reliés » ({rows})")
    return out


def shared_agent_problems(agents: List[Dict[str, Any]], owner: str, name: str, state: str) -> List[str]:
    """`/v1/shared-agents`: « Alex partage Cuisine avec toi » (offered), then « de Alex » on the home (added)."""
    found = [a for a in agents if a.get("owner") == owner and a.get("name") == name]
    if not found:
        return [f"shared agents: no « {owner} partage {name} avec toi » ({agents})"]
    a = found[0]
    out = [f"shared agent: `{k}` missing" for k in ("id", "agent", "household") if not _text(a.get(k))]
    if a.get("state") != state:
        out.append(f"shared agent: state {a.get('state')!r}, expected {state!r}")
    return out


def family_news_problems(news: Dict[str, Any], proposals: int = 0, received: int = 0) -> List[str]:
    """The agent's « Prévenir Léa ? » card (each proposal: its words, at least one relative), and « Léa · 12:04 … Transmis
    par Vie » on the other side (each received note: who, through which agent, what)."""
    out: List[str] = []
    for n, p in enumerate(news.get("proposals") or []):
        if not _text(p.get("id")) or not _text(p.get("text")):
            out.append(f"family proposal {n}: no id or no words")
        if not [r for r in p.get("to") or [] if _text(r.get("id")) and _text(r.get("name"))]:
            out.append(f"family proposal {n}: nobody to send it to (« Prévenir … ? » names no one)")
    for n, r in enumerate(news.get("received") or []):
        for name in ("id", "from", "from_agent", "text"):
            if not _text(r.get(name)):
                out.append(f"family note {n}: `{name}` missing (« {r.get('from')} · … Transmis par {r.get('from_agent')} »)")
    if len(news.get("proposals") or []) < proposals:
        out.append(f"family news: {len(news.get('proposals') or [])} proposal(s), {proposals} expected")
    if len(news.get("received") or []) < received:
        out.append(f"family news: {len(news.get('received') or [])} received, {received} expected")
    return out


def pair_problems(pairs: List[Dict[str, Any]], thread: Dict[str, Any]) -> List[str]:
    """« Vie (Alex) ⇄ Cuisine · 2 messages · Voir l'échange », then the exchange read-only."""
    out: List[str] = []
    if not pairs:
        return ["agent pairs: no line « … ⇄ … » in the conversation"]
    for n, p in enumerate(pairs):
        if not _text(p.get("id")):
            out.append(f"agent pair {n}: no id: « Voir l'échange » opens nothing")
    said = thread.get("thread") or []
    if not said or not all(_text(m.get("text")) and m.get("kind") in ("ask", "answer") for m in said):
        out.append(f"agent pair: the exchange has a line with no words or no kind ({said!r:.200})")
    return out


# -- this release's lines and cards ---------------------------------------------------------------------------------

def auto_line_problems(line: Dict[str, Any]) -> List[str]:
    """A « Validé pour toi » line in the Boîte (InboxAutoSection.swift, AutoApprovals.kt): what was done, when, and
    « Annuler » when it can be."""
    out = [f"auto line: `{k}` missing" for k in ("id", "agent", "kind") if not _text(line.get(k))]
    if not _text(line.get("summary")):
        out.append("auto line: no words (an empty « Validé pour toi » line)")
    at = line.get("at")
    if at is not None and not _num(at) and not _iso(at):
        out.append(f"auto line: `at` is {at!r}, neither a time nor a date (the apps read either)")
    for k in ("undo", "undone"):
        if not isinstance(line.get(k), bool):
            out.append(f"auto line: `{k}` is {line.get(k)!r}, not true/false (« Annuler » / « Annulé »)")
    return out


def routine_card_problems(card: Dict[str, Any]) -> List[str]:
    """« Vie a créé une routine », its name and when, then « Veux-tu être au courant de ce que cette routine
    trouve ? » with « Oui, préviens-moi » / « Non, garde-le sur sa page » (RoutineWay.swift, RoutineWay.kt)."""
    out = displayable.card_problems(card, "routine card")
    routine = card.get("bip_routine")
    if not isinstance(routine, dict):
        return out + ["routine card: no `bip_routine`: drawn as a plain card"]
    for k in ("id", "name", "schedule_display"):
        if not _text(routine.get(k)):
            out.append(f"routine card: `bip_routine.{k}` missing")
    if not _text(routine.get("question")) and "au courant" not in str(card.get("bip_summary") or ""):
        out.append("routine card: no question « Veux-tu être au courant… »")
    if sorted(card.get("bip_choices") or []) != ["notify", "silent"]:
        out.append(f"routine card: choices {card.get('bip_choices')!r}, not notify / silent")
    return out


def steps_problems(item: Dict[str, Any]) -> List[str]:
    """« Étapes, 2 sur 3 », each line its words and state (StepsCard.swift, StepsCard.kt)."""
    where = f"item {item.get('id')} (steps)"
    steps = item.get("steps")
    if not isinstance(steps, dict) or not isinstance(steps.get("items"), list) or not steps["items"]:
        return [f"{where}: no steps to draw"]
    out = []
    for n, s in enumerate(steps["items"]):
        if not _text(s.get("text")):
            out.append(f"{where}: step {n} has no words")
        if s.get("status") not in STEP_STATUS:
            out.append(f"{where}: step {n} status {s.get('status')!r}")
        if s.get("depth") is not None and not isinstance(s.get("depth"), int):
            out.append(f"{where}: step {n} depth {s.get('depth')!r}")
    for k in ("done", "total"):
        if steps.get(k) is not None and not isinstance(steps.get(k), int):
            out.append(f"{where}: `{k}` is {steps.get(k)!r}")
    return out


def hops_card_problems(card: Dict[str, Any]) -> List[str]:
    """« Vie et Coach ont échangé 4 messages entre eux sans toi. Continuer l'échange ? » — Oui, vas-y / Non."""
    out = [p for p in displayable.card_problems(card, "hops card") if "says nothing" not in p]
    if not _text(card.get("bip_summary")) and not _text(card.get("description")):
        out.append("hops card: no words (« Continuer l'échange ? »)")
    hops = card.get("bip_hops")
    if not isinstance(hops, dict) or not isinstance(hops.get("agents"), list):
        out.append(f"hops card: `bip_hops` is {hops!r}")
    return out


def check_page(bridge, page: Dict[str, Any], agent: Optional[str] = None, fetch: bool = True,
               only: Optional[Iterable[str]] = None) -> None:
    """`displayable.check`, the `steps` items checked as both apps draw them (not listed by displayable.py yet)."""
    items = page.get("items") or []
    if only is not None:
        wanted = set(only)
        items = [i for i in items if i.get("id") in wanted]
    found: List[str] = []
    for item in items:
        if item.get("kind") == "steps":
            found += steps_problems(item)
    rest = {**page, "items": [i for i in items if i.get("kind") != "steps"]}
    found += displayable.problems(bridge, rest, agent, fetch)
    _assert(found, "the page is")


SENT_STATES = {"unknown", "queued", "sending", "running", "needs", "reply", "not_sent"}


def sent_state_problems(state: Dict[str, Any]) -> List[str]:
    """`GET /v1/threads/{t}/sends/{client_id}` as Quick Talk decodes it (HermesKit `SentState`, strict Codable;
    Android `Threads.sendState`): one field of another type and Quick Talk loses the reply."""
    out: List[str] = []
    if state.get("state") not in SENT_STATES:
        out.append(f"send state: `state` {state.get('state')!r}")
    for name in ("thread", "message", "needs"):
        if state.get(name) is not None and not isinstance(state[name], str):
            out.append(f"send state: `{name}` is {state[name]!r}, a string is decoded")
    if state.get("next") is not None and not (isinstance(state["next"], int) and not isinstance(state["next"], bool)):
        out.append(f"send state: `next` is {state['next']!r}, an integer is decoded")
    run = state.get("run")
    if run is not None:
        if not isinstance(run, dict):
            out.append(f"send state: `run` is {run!r}")
        else:
            for name in ("run_id", "tool", "answer"):
                if run.get(name) is not None and not isinstance(run[name], str):
                    out.append(f"send state: `run.{name}` is {run[name]!r}")
    reply = state.get("reply")
    if reply is not None:
        if not (isinstance(reply, dict) and isinstance(reply.get("id"), str) and isinstance(reply.get("text"), str)):
            out.append(f"send state: `reply` is {reply!r:.120}, `id` and `text` strings are decoded")
        elif reply.get("needs") is not None and not isinstance(reply["needs"], str):
            out.append(f"send state: `reply.needs` is {reply['needs']!r}")
    if state.get("state") == "reply" and not (reply and str(reply.get("text") or "").strip()):
        out.append("send state: `reply` with no words: Quick Talk says nothing")
    return out
