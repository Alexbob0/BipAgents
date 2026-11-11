# Conversations and notifications: the bridge owns the thread

Design of the « conversations + notifications » rebuild (approved by Alex on 2026-11-02: « Je veux ça propre »).
Sources: `BipAgents-reprise/dette-technique.md` (area #1) and `BipAgents-reprise/notifications-refonte.md`
(phases 1 and 2). This file is the contract between the bridge (`bridge-rs/`) and the apps (iOS, Android, web
console). The apps build on the API described in §6 and §9; the bridge keeps it stable.

Status of each part is in §12.

---

## 1. Why

Today an agent's main conversation has no identity of its own. It is « the Hermes session titled `Bot Chat` », a
string declared four times in the bridge and compared fourteen times in the apps. Each phone rebuilds the thread from
about nine sources (Hermes history, the bridge outbox, Live call records, the pending turn, local unsent items,
attachments kept on the phone, answered approvals kept on the phone, waiting approvals, ids kept from the screen) and
talks to Hermes directly. The results, all seen in October:

- messages drawn twice (a turn already in the history replayed; a reloaded thread that lost its ids);
- the creator's hand-over stored in « Historique » instead of the agent's main thread;
- a notification opening a *copy* of its message at the end of the thread, or an old segment that ends right after it
  (Hermes compressed the conversation and goes on in a new session);
- a photo sent yesterday shown on today's « Yes » (attachments matched back to messages by their text);
- routines on Hermes v0.22.5 (`local-<sha256>` sessions) whose reports cannot be found from the app.

Hermes v0.22.5 also gives the title `Bot Chat` a meaning of its own (its peer FIFO takes text runs on that session),
so a title can no longer be the main thread's identity (branch `botchat-v0225` moves the bridge off it).

The common cause: **the conversation was never a bridge object**. The fix is that the bridge owns it.

## 2. Principles

1. **One stable thread id per conversation, owned by the bridge.** An agent's main conversation has exactly one
   thread id for its whole life, whatever Hermes does with titles and session ids. Side conversations, rooms, shared
   agents' conversations and routine runs get thread ids too.
2. **A thread is a chain of Hermes sessions** (its *segments*). When Hermes compresses a long conversation and goes on
   in a child session, the child becomes the next segment of the same thread. The newest segment is the *tip*: where
   the next turn runs.
3. **Every item has a stable id.** A message keeps its id across compressions, restarts, re-reads and restores.
   Attachments have ids derived from their message's. Nothing on a phone is ever keyed by text, date or position.
4. **The bridge interprets; the apps render.** The bridge reads Hermes (its API or its `state.db`), strips its own
   markers (`<!--bip:…-->` notes, steer wrappers, rescue marks, `MEDIA:` lines, attachment reference lines), merges
   its own items (Boîte messages, queue, approvals, notifications) and serves one ordered timeline. The apps never
   parse Hermes rows or bridge markers again (end state; §11).
5. **A notification is a pointer** to a journal row, which points to `(agent, thread, message)`. It is sent only once
   the message is stored and has its id (§9).
6. **No compatibility layer** (Alex, 2026-11-02: no customers yet, one wave for the bridge, the apps right after):
   what this replaces is deleted (the legacy push fields and builders, `/v1/replies/{id}`, the reply texts kept in
   memory). Routes other features still use stay (`/v1/outbox`). The door's relay to an agent's Hermes
   (`/hermes/<agent>/…`) is gone too (11/11): no app, console or lab used it any more.

## 3. Identities

| What | Id | Example | Stable across |
|---|---|---|---|
| Agent | bridge key (shared agent: `<agent>__<household>`) | `vie`, `cuisine__martin` | renames (display name is separate) |
| Thread | `t_` + 26 chars (time-sortable, random) | `t_01JBX4K9W3M2Q8R6T5Y7Z0A1B2` | titles, compressions, Hermes upgrades, bridge restarts |
| Main thread alias | `main` (accepted wherever a thread id is, with `agent`) | `?agent=vie&thread=main` | — |
| Hermes segment | Hermes session id (`api_…`, `cron_…`, `local-…`) | `api_8f2c…` | — (an implementation detail) |
| Message | `m_` + Hermes `message_uid` (32 hex) | `m_3f9a…` | compressions, restores, re-reads |
| Message (no `message_uid`) | `m_` + first 24 hex of `sha256(agent \0 session \0 row id)` | `m_a1b2…` | re-reads (only on an install whose store has no `message_uid`) |
| Tool step | `<message id>.t<n>` (n = position of the call in the assistant row) | `m_3f9a….t0` | as its message |
| Attachment | `<message id>.a<n>` (n = position in the message) | `m_3f9a….a0` | as its message |
| Boîte message | `o_` + outbox id | `o_9c1e…` | — |
| Queue item | the queue's own id, in `pending.queue` (msg_queue.rs) | `5d2a…` | until it starts (then it is a message) |
| Notification | `n_` + 26 chars | `n_01JBX…` | — |
| Room | `room:<room id>` (thread view in §6.6) | `room:g1a2b3c4d5e6f` | — |
| Room message | `rm_<n>` (n: the room's message number, which only grows); cursor `0.<n>.0` | `rm_12` | the room's life (the oldest of `MESSAGES_KEPT` dropped, cursors hold) |

`client_id`: the id the app gives a message it sends (a UUID: the row it draws at once). Every message goes through
`POST /v1/threads/{id}/messages` (§6.7) with it; the bridge keeps it in `thread_sends` and serves the message at once
as an item `s_<client_id>` (`sending: true`), then gives the stored Hermes row the same `client_id` once Hermes has
written it (matched once, in order, by the bridge that sent the words; kept). The app's row, its voice note and the
files it sent stay on that id, never on words or time (this makes the « Yes shown as an old photo » class of bug
impossible).

## 4. Thread kinds

| `kind` | What | Listed by default | Created by |
|---|---|---|---|
| `main` | The agent's single ongoing conversation (« Discussion »). Exactly one per agent. | yes (first) | adoption (§8) or the first message |
| `side` | Another conversation of the person with the agent (« Historique »), including a background task's own conversation (`origin.kind = task`) | yes | the app, a task, the first sight of an unknown root session |
| `routine` | A routine's run (`cron_<job>_<date>_<time>`, `local-<sha256>` on v0.22.5). Its report goes to its origin thread (Hermes patch 0019) or to `main` as a Boîte message; the run itself is a hidden thread | no | the first sight of a routine session |
| `pair` | Two agents' own conversation (agent_pairs.rs) | no | the household link |
| `live` | A Live call's throwaway session (live_brief.rs) | no | the Live call |
| `room` | A group discussion: rooms.rs keeps its store (`data_dir/rooms/<id>.json`), served as the thread `room:<id>` (§6.6) | `GET /v1/threads?kind=room` | rooms.rs |

`hidden` threads are returned with `include_hidden=1` only. A sub-agent's run (`delegate_task`), a branch or a reset
child is never a segment of anything: it is ignored.

## 5. Lineage: a thread is a chain of sessions

```text
thread t_…  (kind main)
  segment 0  api_aaa   (end_reason = compression)
  segment 1  api_bbb   parent_session_id = api_aaa     ← Hermes compressed and went on here
  segment 2  api_ccc   parent_session_id = api_bbb     ← tip: the next turn runs here
```

- **Continuation rule** (one function, `threads::lineage::is_continuation`): a child session whose
  `parent_session_id` is a segment, whose `model_config` has none of `_branched_from`, `_delegate_from`,
  `_reset_from`, whose `source` is not `tool`. Among several, the latest started.
- **Where it is read**: the agent's `state.db` (read-only, as message_search.rs does) when the bridge knows the profile
  folder; else Hermes' API (`GET /api/sessions?include_children=1`, `GET /api/sessions/{id}`). The same agent always
  uses the same source, so ids never change between reads.
- **When it is read**: at each timeline request (cheap: one indexed query per segment) and when the bridge sees a run
  end. A new continuation is appended to `thread_segments` with `linked = compression`.
- **Manual links**: the bridge may append a session that is not a child (`linked = moved`): the botchat-v0225
  migration moving the person's main conversation to a new session, a hand-over continued elsewhere. The thread's
  history then reads both. `POST /v1/threads/{id}/segments {session_id}` (bridge key; used by migrations and tests).
- **Resolving any session id** (`resolve(agent, session_id)`): a known segment → its thread; else walk
  `parent_session_id` up to a known segment (and append the chain); else a routine session → its routine thread; else
  a root session → a new `side` thread (adopted lazily). This is what makes an old push, a search hit or an old
  `session_id` from an app land on the live thread.

### Storage (bridge.db)

```sql
CREATE TABLE threads (
  id TEXT PRIMARY KEY,            -- t_…
  agent TEXT NOT NULL,
  kind TEXT NOT NULL,             -- main | side | routine | pair | live
  title TEXT,                     -- the bridge's title (Hermes' title is only a fallback for display)
  origin TEXT,                    -- JSON: {kind: task|routine|handover|guide, job?, routine?, from_thread?}
  hidden INTEGER NOT NULL DEFAULT 0,
  version INTEGER NOT NULL DEFAULT 0,   -- grows at every change the apps should refetch for
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX threads_one_main ON threads (agent) WHERE kind = 'main';
CREATE TABLE thread_segments (
  agent TEXT NOT NULL,
  session_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  position INTEGER NOT NULL,      -- 0 = the first segment
  linked TEXT NOT NULL,           -- root | compression | moved | adopted
  added_at TEXT NOT NULL,
  PRIMARY KEY (agent, session_id)
);
CREATE TABLE thread_sends (       -- the messages the apps sent through the bridge (§6.7)
  agent TEXT NOT NULL,
  client_id TEXT NOT NULL,        -- the app's id for it
  thread_id TEXT NOT NULL,
  text TEXT NOT NULL,             -- the person's words as sent
  attachments TEXT,               -- JSON: its files [{kind, name, mime, size, path}] (never a photo's bytes)
  message_id TEXT,                -- m_… once Hermes stored it (NULL: shown as s_<client_id>, for two days at most)
  created_at TEXT NOT NULL,
  PRIMARY KEY (agent, client_id)
);
CREATE TABLE thread_files (       -- files of an agent's message Hermes' row doesn't carry (background_turn.rs, runs.rs)
  agent TEXT NOT NULL,
  message_id TEXT NOT NULL,       -- m_…
  thread_id TEXT NOT NULL,
  files TEXT NOT NULL,            -- JSON: [{kind, name, mime, path}] added, [{path, stale: true}] a MEDIA line hidden
  created_at TEXT NOT NULL,
  PRIMARY KEY (agent, message_id)
);
CREATE TABLE thread_reads (agent TEXT, thread_id TEXT, item_id TEXT, at TEXT, PRIMARY KEY (agent, thread_id));
```

## 6. The timeline API

All routes are under `/v1`, with the bridge key (`Authorization: Bearer <bridge key>`), like every app route.
Errors use the bridge's usual shape (`{"detail": …}`, 404 unknown agent/thread, 422 bad parameters).
`GET /health` lists `threads` in `features` once these routes exist; the apps switch on it.

### 6.1 Threads

```text
GET  /v1/threads?agent=<id>[&include_hidden=1]
  → {threads: [Thread…]}                  main first, then by last activity: the history screen's list (only
                                          what Hermes still keeps); `include_hidden=1` adds routine runs, the
                                          agents' own conversations (`pair`) and hidden ones
GET  /v1/threads?kind=room                the rooms (§6.6), newest first, no agent needed
GET  /v1/threads/{thread}?agent=<id>      (agent required only with thread=main)
  → Thread  (+ segments)
GET  /v1/threads/resolve?agent=<id>&session_id=<hermes session>
GET  /v1/threads/resolve?agent=<id>&c=<pointer>          (c: thread id or `main`)
  → {thread: Thread, tip: <session id>}
POST /v1/threads/{thread}/segments  {session_id, agent?}  (migrations, tests)
  → Thread
PATCH  /v1/threads/{thread}  {title, agent?}   renamed (the bridge's title; a side thread's Hermes session too, never
                                                the main one's: Bot Mode reads it) → Thread
DELETE /v1/threads/{thread}[?agent=]           its Hermes sessions and journal rows deleted → 204; the main thread
                                                keeps its id and starts again empty
```

A shared agent (`agent=<agent>__<household>`): `GET /v1/threads` and `GET /v1/threads/{cid}/timeline` are relayed to
the owner's bridge (§6.6).

`Thread`:

```json
{
  "id": "t_01JBX4K9W3M2Q8R6T5Y7Z0A1B2",
  "agent": "vie",
  "kind": "main",
  "title": "Discussion",
  "hidden": false,
  "origin": null,
  "version": 42,
  "created_at": "2026-11-02T20:10:00.000Z",
  "updated_at": "2026-11-02T21:04:12.511Z",
  "tip": "api_ccc",
  "segments": [{"session_id": "api_aaa", "linked": "adopted"}, {"session_id": "api_bbb", "linked": "compression"},
               {"session_id": "api_ccc", "linked": "compression"}],
  "last_active": 1762117450.5,
  "unread": 2
}
```

`segments` is given by `GET /v1/threads/{thread}` and `resolve` only; `last_active` (Unix seconds, Hermes' last
activity of the thread's sessions) by the list only. `unread` counts the thread's unread journal items (§9.4).

### 6.2 Timeline pages

```text
GET /v1/threads/{thread}/timeline?agent=<id>
      [&limit=50]                 the latest `limit` items (default 50, at most 500)
      [&before=<cursor>]          the `limit` items before the cursor (older page)
      [&after=<cursor>]           every item after the cursor, at most `limit` (catch-up; `more_after` says if cut)
      [&around=<item id>[&before_count=20][&after_count=all|N]]
                                  the item, `before_count` items before it and `after_count` after (default all:
                                  a notification or a search hit opens the conversation there with everything after)
```

Answer:

```json
{
  "thread": { "...": "Thread, as above" },
  "items": [ "Item…, oldest first" ],
  "first": "<cursor of the first item>",
  "last": "<cursor of the last item>",
  "more_before": true,
  "more_after": false,
  "anchor": "m_3f9a…",
  "version": 42,
  "pending": {
    "run": {"run_id": "r_123", "status": "running", "started": 1762117440.2},
    "queue": {"version": 3, "paused": false, "items": [{"id": "q_…", "text": "…", "created_at": "…"}]},
    "approvals": [{"id": "a_…", "run_id": "r_123", "request_id": "p_9", "command": "…", "choices": ["once", "deny"],
                   "session_id": "api_ccc", "created_at": 1762117450.5}],
    "questions": [{"id": "a_…", "kind": "question", "run_id": "r_123", "request_id": "c_2", "questions": […],
                   "execution_generation": 1, "session_id": "api_ccc"}]
  },
  "copy": {"keep": 200, "pending": false}
}
```

- `items` are ordered as the conversation reads: segments in order, Hermes rows in their display order, the bridge's
  own items (Boîte messages) placed by time among them.
- A **cursor** is opaque to the apps (today `<segment position>.<row order>.<item id>`). The apps keep the `last`
  cursor of what they have and ask `after=` on each `thread` event. An item that moved or vanished (a restore, a
  rewind) still pages correctly: the cursor's position is used when its item is gone.
- `anchor` is the `around` item when it was found (else null and the latest page is given).
- **Where a waiting card goes** (thin apps, brick 5): each row of `pending.approvals` / `pending.questions` (any
  waiting row with a time) carries `after`: the id of the last item that draws something said at or before its
  `created_at`; null when it was asked before them all; the last item when it has no time. Both apps draw it right
  after that item's row (above the rows shown when that item is not shown), every card, not only the first.
- **How a page goes into the phone's window** (thin apps, 5/11; one rule on both apps): `reset: true` — the page is
  the window (latest, around, and below); else `joins: <cursor>` — an `after=` page goes after the held item at that
  cursor, a `before=` page in front of it, and what the phone held beyond that item is gone (rows the bridge no
  longer gives there). With no `limit` the bridge chooses the sizes: latest 100, a catch-up 200 at most (more since:
  the latest page, `reset`), an older page 50; a cursor whose item is gone also gets the latest page, `reset`. A
  `limit` given (a one-off look; apps from before 5/11) pages as before. Every `after=` page also gives again the
  messages sent and not stored yet (`s_` items, always at the end), even the one at the cursor: their state changes
  where they stand (`sending` → `not_sent`), and a phone whose last item was one read nothing after it (6/11, Android
  never showed « Pas envoyé »). The apps drop what they held of the page's ids before joining it, as for any page.
- **The phone's copy** (thin apps, brick 6): every page carries `copy: {keep, pending}`, the same for every phone:
  keep the conversation's latest `keep` items (200: two opening pages) for when the bridge is out of reach, and
  what waits (`pending`: cards, the run) only when `pending` is true — false today: a card is the bridge's state now,
  never answered from a copy. Until 6/11 the iPhone kept 300 items with what waited, Android 200 without; a page
  without `copy` (an older bridge): each app as before.
- `replaced` (only when not empty): ids a phone may hold that this page's rows replace. Today the `s_<client_id>` item
  of a message sent from a phone, once its stored row (same `client_id`, last three days) is in the page. The apps
  remove those ids from their window before merging, nothing else (4/11: the message showed twice).
- `tz` (query, optional): the phone's IANA zone. Each item that draws something gets `day` (`YYYY-MM-DD` there; else
  in the person's zone, `dates`), and the first such item of each day in the whole thread `new_day: true`. The apps
  draw a day divider over a `new_day` row and over the first row with a `day` they show; a row without `day` (it
  draws nothing, or the phone made it) never gets one (5/11: empty « Dimanche 1 novembre » dividers). The iPhone
  reads the same places from the rows' `day` alone (HermesKit `DayDividers`: the first dated row shown, and each
  whose day is not the previous dated row's), so a window still holding rows of an older bridge, with the `new_day`
  of then, never says a day twice (7/11: a second « Vendredi 6 novembre » while the phone's copy gave way to the page).
- `pending` is what is not a stored message yet: the run going on (attach to `/v1/runs/{run_id}/events` for its live
  text, as today; `started`: when the bridge began following it, Unix seconds, the working line's clock on every
  phone), the agent's queue for this thread (msg_queue.rs, each item with its `client_id`), the approvals and
  the questions waiting (the run hub's). Each card carries `id`: the item it becomes once answered (`a_…`, also
  `item_id` in the run's `approval.request` / `clarify.request` events), so a card is one row on every device and
  an answer given on one replaces it in place on the others (the answer bumps the thread: `thread` hint). Once a run ends
  its messages are items; the app reads the page again and draws them in place of its live rows. Items carry no
  `run_id` (the bridge does not know which stored row a run wrote; the apps never needed it).

### 6.3 Items

Every item has `id`, `kind`, `at` (ISO 8601 UTC), `cursor`, `turn` (the id of the person's message that started this
turn; the first item's own id before any) and `segment` (Hermes session id, for the transition and debugging only).
An item that draws something also has `day`, and `new_day` when it opens its day (§6.2, `tz`).

**How its turn lays out** (thin apps, brick 7; threads/layout.rs, decided once for every app on the whole thread):
the page says `layout: {open}`, the first item of the turn still going (null when none is): the items before it are
decided, and carry `layout` when there is something to say (none: nothing; the apps give them an empty one as they
keep them). From `open` on, and on a page without `layout` (an older bridge), the apps' own rule. The lines are worked
out on the whole thread, the fields given for the page's items only. Keys name the
screen's lines: an item's id, `<id>.r` for a message's reasoning, a tool card by its first tool's id. `work: true`
(`reasoning_work` for the reasoning line): the line is the agent's work (tools, reasoning, its short commentary between
calls), folded with the work lines next to it; `turn_end: true`: this text is its turn's reply (its time, « Écouter »);
`ack: {line, index}`: the app's own first word goes before this item's first line, worded from call
`index` of card `line` (a `tool_call` batch unwrapped), the turn's first real call (never the agent's bookkeeping);
`exchange`: the peer message opening the exchange the line folds into, `request: true` on a peer asking this agent
something, `answers: <name>` on the first reply to that request. The apps keep their rule for the lines streamed live.

| `kind` | Fields | From |
|---|---|---|
| `message` | `role` (`person` \| `agent`), `text` (visible words only), `attachments`, `client_id`?, `added_during_run`? (a steer), `reasoning`? , `sending`? (`s_<client_id>`: sent, not stored by Hermes yet) | Hermes user / assistant rows, the bridge's sends |
| `tool` | `tool: {name, call_id, arguments, result, status (done \| failed \| running), via}` | assistant `tool_calls` + their tool rows |
| `notice` | `notice: {code, text, stopped?}`: `failed_turn` (Hermes' failed-turn row; `stopped`: `partial` \| `not_processed`, which of Hermes' two lines, the apps say it in their words), `loop_rescue` (the bridge's rescue turn) | Hermes rows the bridge recognizes |
| `boite` | `text`, `title`, `attachments`, `boite: {outbox_id, audio: bool}` | the bridge's outbox (routine reports, hand-backs, credit, storage, imports) |
| `approval` | `approval: {run_id, request_id, command, description, topic, choice, state: answered}` | approvals_log.rs (Hermes' history has no approval rows) |
| `call` | `call`: the Live call's record as `GET /v1/live/calls` gives it | live_calls.rs |
| `steps` | `steps: {items: [{text, status, depth?}], done, total}`: the turn's task list (« Étapes », §6.3b) | the turn's `todo_list` calls (steps.rs) |
| (role `peer`) | a `message` with `role: peer`, `from: <agent id>`, `from_name` (the name it signed with, Hermes' emoji badge left out), `text` its words alone: another agent's message in Bot Mode's conversation (the apps find the agent by `from`, never by name) | « Message from … (@x): » rows |

**Drawing order: the items' order** (no ordering logic in the apps). Each assistant row gives its words first, then
its own tool calls (their results folded in); the next row's words come after those calls; the final answer last.
`items_come_in_the_order_the_apps_draw_them` (threads/timeline.rs) pins it.

An agent message may carry `renamed: [{agent, name}]` (the agent renamed itself in that turn, guide.rs): the app
updates the name at once, whenever it reads the item.

Never items: hidden rows (`display_kind` hidden, inactive rows unless compacted, compression summaries), the bridge's
own steers (`[bipagents:…]`), restore notes (`<!--bip:restore…-->`), user rows made only of `<!--bip:…-->` notes.

A conversation Hermes compacted in place keeps several rows per message (`message_uid`): the summarized one
(compacted), the copies each compaction makes of the messages it keeps (the originals left inactive), and a summary
holding the uid of a message it folded in. One item per message, at its first row: its place, and so its cursor, never
change when Hermes compacts again (`a_compaction_leaves_every_message_at_its_first_row`, black-box
`test_threads_compaction.py` on rows a real Hermes wrote).

Attachment:

```json
{"id": "m_3f9a….a0", "kind": "image", "name": "photo.jpg", "mime": "image/jpeg", "size": 184223,
 "path": null, "url": "/v1/threads/t_…/attachments/m_3f9a….a0"}
```

`kind`: `image` (a photo in the message: served by the URL), `document` (`[Pièce jointe : …]` lines: `path` on the
server, and a `url` like a photo's that goes on to `/v1/media`), `file` / `page` / `map` / `audio` (the agent's `MEDIA:` lines: `path`, served by the existing file routes;
`offset`: where the line was in `text`, for inline pages). `size` is always a number of bytes or null, never words:
a document's is its file's when the bridge sees it in the agent's uploads, else read from its line (« 12 Ko » ≈
12000; 6/11, the line's words made both apps reject the whole page). The apps read `size` leniently all the same
(a number, a number in a string, « 12 Ko »; else none) and skip an item they cannot read, never the page. An app sends a file with `client_id`; the bridge returns
it on that message's item whether or not Hermes kept it.

Example (`docs/examples/timeline-main.json` has a full page):

```json
{"id": "m_7c21…", "kind": "message", "role": "person", "text": "Yes", "at": "2026-11-02T20:58:01.000Z",
 "cursor": "2.000418.m_7c21…", "turn": "m_7c21…", "segment": "api_ccc", "attachments": [], "client_id": "C0FFEE…"}
{"id": "m_7c21….t0", "kind": "tool", "at": "2026-11-02T20:58:03.000Z", "cursor": "2.000419.m_88aa….t0",
 "turn": "m_7c21…", "segment": "api_ccc",
 "tool": {"name": "web_search", "call_id": "call_1", "arguments": "{\"q\":\"train Lyon\"}",
          "result": "3 results…", "status": "done", "via": null}}
{"id": "m_88ab…", "kind": "message", "role": "agent", "text": "Le train de 9 h part de la voie 4.",
 "at": "2026-11-02T20:58:09.000Z", "cursor": "2.000421.m_88ab…", "turn": "m_7c21…", "segment": "api_ccc",
 "attachments": []}
```

### 6.3b Steps (« Étapes », 6/11)

For work of several steps the agent keeps a short task list with Hermes' `todo_list` tool (its soul's `STEPS_RULE`).
The person sees it as one compact card per turn, live while the turn runs, then folded (« 5/5 étapes »).

**The item.** One `steps` item per turn that wrote a list: the list as the turn's last `todo_list` call left it (the
call's result is always the whole list), placed where that call is, so near the reply. Its id is `<turn>.steps` (the
person's message that started the turn, then `.steps`): the same item, updated in place, whatever the number of calls;
its cursor moves forward with the last call. The turn's `todo_list` calls are never served as `tool` items. A call
through Hermes' tool search (`tool_call` wrapping one `todo_list` call) counts the same; a failed write is left out.

```json
{"id": "m_7c21….steps", "kind": "steps", "at": "…", "cursor": "2.000431.1", "turn": "m_7c21…",
 "steps": {"items": [{"text": "Lire les e-mails", "status": "done"},
                     {"text": "Trier par urgence", "status": "in_progress"},
                     {"text": "Vérifier les pièces jointes", "status": "pending", "depth": 1},
                     {"text": "Appeler la banque", "status": "cancelled"}],
           "done": 1, "total": 3}}
```

`status`: `done`, `in_progress`, `pending`, `cancelled` (Hermes' `completed` is `done`). `depth`: a sub-step's level
(Hermes' `parent`), only when not 0. `total` leaves cancelled steps out, `done` counts the done ones. A step's text is
cut to 200 characters, a list to 30 steps.

**Automatic steps** (Alex, 6/11). A turn of several actions without a list of the agent's own gets one made by the
bridge from its real calls, in the same item and events (`"auto": true` in `steps`): one line per kind of call for the
whole turn, in the order of its first call, its calls counted together (« Chercher dans tes mails », « Lire 6 mails »,
« Ouvrir le site sncf-connect.com »), a line with a call running `in_progress`, the others `done`. Only useful steps
count (7/11, Alex: « Faire un calcul / Faire un calcul / Noter dans ton Carnet », « pas très utile »): never the agent's
workings — calculations and code, files, the terminal, re-reading its notes, pages or group talks, its task board,
choosing a voice, estimating space, checking the allowed sites, going through the phone (`BACKSTAGE` in
scripts/tool-labels.py, `backstage` in tool_kinds.rs). Shown only from 3 distinct useful steps, live and stored alike
(stored cards are made at each read, old conversations too): live, the card comes when the 3rd useful step starts and
stays (steps only grow in a turn). At most 6 lines, the oldest done steps folded into a first « +N étapes » line (`folded`:
how many). The words are the bridge's for the tool's kind, in the agent's language (fr, en, de, es), with a count and
at most a site's domain: never a call's arguments (a mail's subject, an address, a file's words). Calls that are no
step of the work (the tool search, memory, a question to the person, the list itself) are left out. As soon as the
agent writes its own list, its list replaces the automatic one for the rest of the turn. Stored: after the turn's last
call (cursor `<segment>.<row>.99`).

**Live.** While the turn runs, each change reaches the apps in its run's stream as the bridge's `steps.updated`
(`{"event": "steps.updated", "run_id", "steps": {…as above}}`), made from Hermes' `todo.updated` (Hermes patch 0026;
Hermes' own event is not passed on). The apps draw it as the live turn's card (at the bottom of the turn, above the
reply being written); once the run ends the page has the stored item in its place. The open screens of that thread
also get a `thread` hint with `reason: "steps"`, the `steps` and the `run_id`, **without a new version** (nothing to
fetch: apps from before 6/11 skip a hint whose version they hold). Without patch 0026 there is no live list: the item
appears with the turn's rows when the run ends (state.db has the calls).

**Background tasks** (jobs.rs): a running task's `activity` has `steps` (same shape) and `step` (the step in progress,
else the next one); its conversation's page has the item. **Live Activities** (live_activities.rs): `steps` and
`stepsDone` in the content state while a list exists, `step` (its words) only with `[push] previews` (the publisher's
relay keeps the two numbers, never words).

Apps from before 6/11 skip the item (an unknown `kind`) and the event (an unknown type): they show neither the card nor
the `todo_list` calls.

### 6.4 One item, attachments

```text
GET /v1/threads/{thread}/items/{item}?agent=   → Item (404 when gone)
GET /v1/threads/{thread}/attachments/{id}?agent=  → the bytes (image parts of a Hermes row, decoded); a path
                                                    attachment redirects (307) to GET /v1/media?path=&agent=
```

### 6.4b Search hits

`GET /v1/search/messages` (message_search.rs) gives each result its `thread` (the thread's id, `t_…`, the main
thread's too: the app knows it from `GET /v1/threads/main`) and `item` (`m_…`): the app opens
`timeline?around=<item>` on that thread, never by words or dates. One shape for both apps; no `m`, no `main` flag.

### 6.5 Events

```text
GET /v1/threads/events[?agent=<id>]      SSE, keepalive comment every 10 s
event: thread         data: {"thread": "t_…", "agent": "vie", "version": 43, "reason": "run|boite|segment|call|lost"}
event: notification   data: {"n": "n_…", "agent": "vie", "t": "reply", "c": "t_…", "m": "m_…", "state": "open"}
event: read           data: {"agent": "vie", "thread": "t_…", "notifications": ["n_…"]}
event: lagged         data: {}                    (the app missed events: refetch what it shows)
```

`thread` is sent today when a run of the thread ends (`run`: the run hub), a Boîte message for it is stored
(`boite`), a Live call is recorded in it (`call`, live_calls.rs: its item is in the next `after=` page; no journal
row), Hermes continued it in a new session (`segment`), and words steered into a turn that stopped without taking
them become « Pas envoyé » (`lost`: 11 s after the send, nothing running, the message not stored; a turn that ends
later says it with its own `run`). A deleted thread forgets its messages not stored (`thread_sends`): the main thread
starts again under the same id without them. The agent's queue is told in the run's stream
(`bridge.queue`, msg_queue.rs); the timeline's `pending.queue` gives its state at each read.

**Presence**: `GET /v1/threads/events?agent=<a>&thread=<id|main>` means « this conversation is on screen » for as
long as the stream is open: a notification for that thread is journaled, marked read, and not pushed
(`push: skipped:on_screen`). This replaces the `session_state` poll every 5–8 s.

On `thread`, an app showing that thread asks `timeline?after=<its last cursor>`; a list screen refetches
`/v1/threads`. Events are hints: an app that missed some (asleep, offline) compares `version` and refetches. The live
text of a run still comes from `/v1/runs/{run_id}/events` (unchanged): every delta as Hermes gives it, nothing held
but a line that may still become a `MEDIA:` line (until its newline); a character Hermes passed in two UTF-16 halves
over two deltas (a lone surrogate escape each) is joined, a half alone becomes U+FFFD (hermes.rs `SSEParser`). The
apps draw the reply as it comes (iOS: at most 20 times a second, in its own bubble), with no cursor after it, its
new words fading in from 30 % to full in 250 ms (`StreamReveal`; none with Reduce Motion or Android's animations
removed), and show their working line under a reply whose words stopped for 2.5 s (`ReplyPause`: Hermes sends
nothing while the model writes a call's arguments or its own notes).

### 6.6 Rooms and shared agents

- **Rooms** (branch rooms-items, reduced in rooms-reduction) are threads `room:<id>` on the same routes, no agent
  needed (several agents speak):
  - `GET /v1/threads?kind=room` lists them (`{id: room:<id>, kind: room, title, version, tip: null, last_active,
    unread, room}`, `room` the room as `pending.room` gives it, with `last`); `GET /v1/threads/room:<id>` is one;
    `GET /v1/threads/room:<id>/items/rm_<n>` an item and `…/attachments/rm_<n>.a<k>` its file (→ `/v1/media` with
    the agent that handed it), through the conversations' code.
  - `GET /v1/threads/room:<id>/timeline` (same paging): items `rm_<n>`; the person's words and the agents' a `message`
    (`role` person | agent, `from` the agent, `client_id` on the person's), a pass, a failed or capped turn, a task
    handed over a `notice` (`code` pass | failed | capped | handover); `turn` the person's message answered; an agent's
    files its attachments (`path`); what only a room has under `room` (`hop`, `route`, `handoff`, `task`, `handover`,
    `tokens`, `error`, `capped`). `pending.room`: the room (participants, `answering` with the approval each turn waits
    for, goal, proposal, progress, tasks) and `handoffs` (its last 5 relays: the conductor may set one on an item the
    app has already). `thread.version` = the room's version.
  - `POST /v1/threads/room:<id>/messages {client_id, text}` → `{client_id, thread, state: started | queued | known,
    route: {by, agents}, item}` (words only: attachments 422; the message is kept before the router picks who answers,
    so the other devices see it at once); `POST /v1/threads/room:<id>/stop` → `{stopped}`.
  - Every change of a room (rooms.rs `Rooms::edit`) is a `thread` hint `{thread: room:<id>, agent: null, version,
    reason: room}`, sent only to the streams that follow it (`GET /v1/threads/events?thread=room:<id>`, « on screen »:
    presence) and the home's (no agent, no thread).
  - Each agent's reply is journaled (`room_reply`, `c: room:<id>`, `m: rm_<n>`): unread for the apps' dots
    (`/v1/notifications/unread` `threads`), read and not pushed while the room is on screen, read with
    `POST /v1/notifications/read {thread: room:<id>}`. A notification opens the room `around=m`.
  - `/v1/rooms` keeps the room's changes (create, change, delete, tasks); its list and view
    (`GET /v1/rooms[/{id}]`), `GET/POST /v1/rooms/{id}/messages` and `/stop` are gone, so are the apps' polling,
    `RoomsCenter.isUnread`'s phone-side « seen » map and Android's.
- **Shared agents** (agent_shares.rs): the relative's app asks its own bridge with `agent=<agent>__<household>`;
  `GET /v1/threads` lists her conversations with that agent (`kind: shared`, `id` = the conversation's id) and
  `GET /v1/threads/{cid}/timeline` (same parameters, same answer shape) is relayed to the owner's bridge
  (`GET /family/s/{agent}/conversations/{cid}/timeline`, the household key, her own conversations only), which
  serves only words and notices (no attachment, tool step or Boîte item: only text crosses). Its answer has the
  timeline's shape: `thread: {id: <cid>, kind: "shared", title, version}`, `items`, `first`, `last`,
  `more_before`, `more_after`, `anchor`, `version`, and `pending: {run: {run_id, status: "writing" | "approval",
  started: <Unix seconds>} | null, approvals: [{id, run_id, request_id, command, description, choices}],
  questions: [{id, kind: "question", run_id, request_id, questions, question, choices}]}`.
  Each timeline read counts as « on screen » for that conversation (the relative's bridge pushes nothing for it
  meanwhile).
  Relayed the same way (step 4): `GET /v1/threads/events?agent=<x>__<h>&thread=<cid>` (the owner's
  `…/conversations/{cid}/events`: `thread` hints; open = on screen), `GET /v1/runs/{run}/events?agent=<x>__<h>&thread=<cid>`
  (the turn's words live: text deltas, approvals, questions and the end only, no tool step),
  `POST /v1/threads/{cid|new}/messages` (her `client_id` kept on the owner's side; `new` opens a conversation titled by
  its first words), `…/{cid}/stop`, `DELETE /v1/threads/{cid}`, and her answers (an approval's `choice`, a question's
  `answer`) through `POST /v1/approve` with `agent=<x>__<h>` (the owner's `/family/s/{agent}/approval`); her bridge
  pushes a question like an approval while her app isn't looking. Her own person's
  words come unframed. The `/v1/shared-agents/{id}/conversations…` routes are gone.
  The apps open a shared agent in the conversation screen of their own agents (11/11), so `main` works as for one's
  own agent: her latest conversation with it (the page, the events, a turn's words, stop, delete), before her first
  message an empty page `{thread: {id: main, kind: shared, tip: null}}` (nothing to follow: events 404), her first
  message to `main` makes it. The thread's `tip` and a send's `tip` are the conversation's id (the apps key a turn by
  it). A catch-up (`after=`) is answered with the latest page (`reset: true`): her words only, whose cursors move as
  Hermes stores her turn.

### 6.7 Sending, stopping, answering (step 4)

```text
POST /v1/threads/{thread|main|new}/messages
     {agent?, client_id, text, attachments?: [{kind: image, name, mime, data (base64)} | {kind: document, name, mime,
      size, path (POST /v1/files)}], notes?, kinds?, after?, instructions?}
  → {client_id, thread, tip, state: started | steered | queued | known, run_id?, queued? (known only)}
POST /v1/threads/{thread}/stop {agent?}          → {stopped, run_id?}
POST /v1/approve {agent, run_id, request_id?, choice | answer}   an approval's choice or a question's answer (empty:
                                                                 skipped); the bridge's own cards as before
```

One sending path (threads/send.rs): nothing runs in the conversation → a turn starts (followed at
`/v1/runs/{run_id}/events`); a turn goes on → words go into it (Hermes' steer), files, `after` (« Après ») or a turn
that takes no more words → the agent's queue (msg_queue.rs), started next; the same `client_id` again → nothing
is sent, `known` with where the message is now: `queued: true` (still in the queue), or `run_id` of its conversation's
turn going on (the phone follows it again), or neither (its turn is over: the timeline has it). A message that never
reached the bridge stays on the phone « Pas envoyé », and a tap sends it again with the same `client_id`: both apps,
one rule.
The bridge takes what waited for the message (`live_brief::message_notes`: the time, a Live call's recap, the
family's news…), adds the app's `notes` (its 👍 / 👎), builds Hermes' input (photos as image parts of one user message,
documents as `[Pièce jointe : …]` lines), creates the tip session when the thread has none (`new`: a side thread).
A photo also becomes a file with the agent's uploads at once (`keep_photos`, where `POST /v1/files` puts documents;
6/11: every photo sent showed « 0 Ko », nothing to draw): its `s_` item, its queue item and Hermes' stored row
(matched by `client_id`) carry `{kind: image, name, mime, size, path}`, served like any path attachment. A queue item
says what the message carries: `attachments` (the same, never bytes) and `voice: true` (a voice note's transcript);
a message of files alone has an empty `text`, and the apps' « En attente » row always says something (« Photo ·
1,2 Mo », the file's name, « Message vocal », else « Pièce jointe »).
Every answer goes through the bridge, which keeps it (approvals_log.rs) and bumps the thread. Deleted with this:
`POST /v1/agents/{id}/queue` and `/steer`, `GET/POST /v1/approvals/answered`, the apps' Hermes-direct runs, steers,
approvals and questions.

**A sent message followed until its reply** (threads/follow.rs, thin apps brick 6: the quick talk, iPhone and
Android, the same rule once):

```text
GET /v1/threads/{thread}/sends/{client_id}?agent=
  → {client_id, thread, state: unknown | queued | sending | running | needs | reply | not_sent, message,
     run: {run_id, tool, answer} | null, needs: approval | payment | question | pick | task | null,
     reply: {id, text, needs: media | pick | null} | null, next: <ms> | null}
```

`message` is the person's item (where the app opens); `run` while the turn goes on: the tool in use now (raw name,
null: it thinks or writes) and the answer's words so far, as the bridge follows the run (the apps no longer follow the
run's events for a quick talk); `needs`: a card of this turn (placed after the person's words: an older card waiting
is not this turn's), or a tool whose card is to act on (a payment, a task, an agent proposed, a question, a secret);
`reply`: the agent's messages after the person's, joined, once the turn is over (`needs`: a card or a file the voice
can't say); `next`: when to ask again (1 s while it runs, 1.5 s while it waits; null: settled). The thread's hint is a
reason to ask sooner. An older bridge answers 404: the apps read the timeline as before.

Run state comes from the timeline only: `pending.run` (is the turn still going?), `pending.approvals` /
`pending.questions` (what it waits for) and the events stream; the apps ask Hermes nothing about a run (their
`getRun` / `checkRun` and the old `GET /v1/sessions/{id}/state` poll are gone).

**A Live call** (live_turns.rs, live_brief.rs) keeps its own short session, made by the bridge:

```text
POST /v1/live/calls/start {agent, session?, prelude?}           → {ok, live_session}   the warm-up read at once
POST /v1/live/turns {agent, live_session, text, attachments?}   → Hermes' chat/stream SSE, passed on as it comes
POST /v1/live/turns/stop {agent, run_id}                        → {stopped}
POST /v1/live/calls {agent, session, live_session, run_id?, started_at?}   the call put into its conversation
```

The first turn's text starts with the call's hidden prelude (the time, its rules, its brief, the greeting said); the
bridge adds the agent's quick-turn thinking (`quick_turn_options`). One request per turn, streamed through: the first
words come as early as from Hermes itself. The call's recap joins the next written message on the bridge's side
(`message_notes`): `POST /v1/live/recaps` and `/v1/live/recaps/take`, and the phones' own recaps, are gone.

**The web console** (`/console/`) uses the same routes: the thread list, the timeline and its events stream (on screen
while open), the send and stop routes, `POST /v1/approve` for approvals and questions, a run's events at
`/v1/runs/{id}/events`.

## 7. How the apps use it (target)

- **One cache per thread, keyed by item id.** Merge rule: an item with a known id replaces the cached one; a new id is
  inserted at its cursor. No text, date or position comparison anywhere. The optimistic bubble of a sent message
  carries the `client_id`; the server item with that `client_id` replaces it.
- **Opening a conversation**: `timeline?limit=50` (or `around=<m>` from a notification, a Boîte row, a search hit).
  Scroll up: `before=<first>`. Live: the `thread` event → `after=<last>`. Version mismatch after sleep → `after=`.
- **No Hermes calls** for conversations: list, history, sending, run state, approvals, clarify and Live turns go
  through the bridge, as everything else (routines, settings): the bridge has no relay to Hermes any more.
- **No marker parsing**: `text` is already visible text; attachments are structured.
- **Badges and read state** come from the journal (§9.4).

## 8. Migration of existing places

Runs per agent at the first need (lazy), never blocking a request more than one Hermes listing (none on a managed
install: the store is read):

1. If the agent has a `main` thread with segments, done.
2. Else find the main conversation's current session, in this order:
   a. `<data_dir>/main-sessions.json` (`{agent: session_id}`), for a migration that moves the person's conversation
      to another session;
   b. the newest non-hidden session titled `Bot Chat` (case-insensitive), then a hidden one, never a `local-…` one.
      Since botchat-v0225 (Hermes patch 0021) the person's « Bot Chat » stays an API conversation (`api_…`) and a
      peer's own « Bot Chat » is a hidden `local-…` session (a `pair` thread here); a thread an unpatched v0.22.5
      moved aside (« Bot Chat (API <id>) ») is put back at start by welcome.rs, so the title still finds it;
   c. none: the main thread is created empty (its id is stable from then on); it is looked for again at the next
      requests (through Hermes' API at most every 30 s), and the person's first « Bot Chat » becomes its first
      segment even when it is resolved first (an app's session id, a push).
3. The chosen session's whole line (parents up to the root, continuations down to the tip) becomes the main thread's
   segments (`linked = adopted` for the first, `compression` for the others).
4. Other sessions are adopted lazily as `side` / `routine` threads the first time they are listed or resolved.

A later move (a migration, a hand-over continued elsewhere) keeps the main thread's id: the new session is appended as
a `moved` segment (`POST /v1/threads/{id}/segments`). The bridge no longer looks at titles once the main thread has
segments.

## 9. Notification journal (refonte phase 1)

### 9.1 One entry, one table

Every notification of an agent goes through `PushService::emit(Event)` (the `notify_*` functions build the event and
call it). Steps:

1. **Dedupe** on `dedupe_key` (unique): `reply:<agent>:<message id>`, `approval:<agent>:<request id>`,
   `question:<agent>:<request id>`, `boite:<agent>:<outbox id>`, … A run end and the cron watcher noticing the same
   reply now give one notification.
2. **Resolve the conversation** (`c`: thread id, `room:<id>`, or none) from the session the producer gave.
3. **Wait for the message** (replies, questions, sub-task reports): look for the stored row in the thread (by run,
   then by the reply's first words) for at most `[push] message_wait_seconds` (default 10 s); its id becomes `m`.
   Boîte messages are stored already (`m = o_<outbox id>`). On timeout the push goes without `m` and the row is
   filled later (the Boîte then still points at the message).
4. **Insert** the journal row (state `open`).
5. **Policy** (§9.2) decides: push, or keep silently (`push = skipped:<reason>`). Silenced rows are still in the
   journal: the Boîte shows them, unread.
6. **Push** the pointer (v2 fields + every legacy field, §9.3). The journal row records `sent` and how many devices.
7. Emit the `notification` event (§6.5).

```sql
CREATE TABLE notifications (
  id TEXT PRIMARY KEY,           -- n_…
  agent TEXT NOT NULL,
  type TEXT NOT NULL,            -- closed enum below
  thread_id TEXT,                -- c: t_… | room:<id> | NULL
  message_id TEXT,               -- m: m_… | o_… | NULL
  ref_id TEXT,                   -- run / request / card / job / held mail / meet id the item is about
  session_id TEXT,               -- the Hermes session the producer named (legacy apps, debugging)
  title TEXT, body_key TEXT,     -- display name; neutral body code (notif.reply…), apps translate
  preview TEXT,                  -- first words (kept on the bridge; never sent in clear)
  args TEXT,                     -- JSON (names, host, line)
  state TEXT NOT NULL,           -- open | answered | expired
  created_at TEXT NOT NULL,
  read_at TEXT,
  push TEXT,                     -- sent:<n> | skipped:switch | skipped:no_device | pending
  dedupe_key TEXT UNIQUE,
  legacy TEXT                    -- JSON: the legacy payload fields (what an old app reads)
);
```

**Types** (closed enum, `notifications::Type`): `reply`, `question`, `approval`, `room_reply`, `boite`, `task`,
`subtask`, `scan`, `family`, `email_held`, `site`, `login`, `connector`, `secret`, `mail_card`, `meet`. Silent syncs
(`reminders`, `calendar`, `contacts`, `run_finished`, `read`) and Live Activities are not journal rows.

### 9.2 Rules: « Me prévenir »

One table (`notifications::Kind::follows_switch`), not scattered `if`s:

| Type | Follows the agent's « Me prévenir » switch | Notes |
|---|---|---|
| `approval`, `email_held`, `site`, `login`, `connector`, `secret`, `mail_card`, `meet` | no: always notify | an agent is never stuck without the person knowing; security asks always reach the person |
| `question` | no: always notify | **adopted on 2026-11-02** (an unanswered question blocks the agent like an approval) |
| `reply`, `room_reply`, `subtask`, `scan` | yes | « il a fini » |
| `task` (ended, paused, proposed, rescued) | yes | |
| `boite` from a background task's hand-back | yes | **adopted on 2026-11-02** (« il a fini ») |
| `boite` from a routine | no (each routine has its own mute, cronwatch.rs) | |
| `boite` other (credit, storage, imports, watchers) | no | their sources decide |
| `family` | no | |

Two more rules, in `emit`: a notification for a thread on screen (§6.5) is read and not pushed.

### 9.3 The push: a pointer, nothing else

No compatibility window (Alex, 2026-11-02: no customers yet; every place gets this bridge in one wave and the apps
ship right after): the push carries the pointer only, no legacy field (`run_id` as a pseudo-id, `session_id`,
`outbox_id`, `reply_id`, `kind`, `room_id`, `email_held`, `site_request`, `login_request`, `*_request`, `meet`,
`family`, `job_id` are gone).

```json
{"aps": {"alert": {"title": "Vie", "body": "Ta réponse est prête.", "loc-key": "Ta réponse est prête."},
         "thread-id": "vie/t_01JBX…", "mutable-content": 1, "category": "MESSAGE", "sound": "default", "badge": 3},
 "v": 2, "n": "n_01JBX…", "t": "reply", "agent": "vie", "c": "t_01JBX…", "m": "m_3f9a…", "r": "r_123"}
```

- `t`: the type (§9.1); `r`: what it is about (the run, the approval's or question's request, the card, the job, the
  held e-mail, the meet); `c` absent: no conversation (a card for the Boîte, a family note: the app opens the agent
  or the Boîte); `m` absent: the message was not stored within the wait (the app opens the conversation at its end,
  then asks `GET /v1/notifications/{n}`, which has `m` once found).
- Approvals add what their buttons answer: `run_id`, `request_id`, `choices`; questions `run_id`, `request_id`.
- `aps.thread-id` is `<agent>/<c>` (the agent's when there is no conversation): one notification group per
  conversation. `aps.category`: `APPROVAL` for Hermes' approvals (« Approuver une fois » / « Refuser »: one
  command, all of it in the notification); `BRIDGE_CARD` for the bridge's own cards (a site, an account, a
  connector, a secret, a mail card), never allowed from the notification: a tap opens the card, where all it allows
  shows (Alex, 2026-11-05: a sites card allowed from its notification allowed its first host only); `MESSAGE` for the
  rest. `aps.badge`: the journal's unread total.
  Compatibility (2026-11-06): `BRIDGE_CARD` replaced `CARD` (connector, mail card) and `MESSAGE` (site, account,
  secret). Apps released before it don't know the name, so they show no button on these (`CARD` gave « Autoriser une
  fois » on the iPhone, `MESSAGE` « Répondre ») and open the card on a tap as before (they route by `t`, never by
  the category); newer iPhones register it with « Refuser » only, Android never adds a button to it. The apps still
  read `CARD` from a bridge before it, with « Refuser » only.
- The text the extension shows: the sealed `enc` (below); `GET /v1/notifications/{n}` (`preview`, the whole text)
  when the phone is on the tailnet. The alert itself never carries words.
- Collapse ids only where one notification replaces another: a run's reply (`run_id`), an approval's or question's
  request, a room (`room:<id>`), a task (`job:<id>`), a card, the mailbox scan. Autonomous replies, sub-task reports
  and Boîte messages are never collapsed.
- FCM gets the same fields as data (strings).
- The deep link the apps build from it: `bipagents://open?agent=<a>&c=<c>&m=<m>&n=<n>&t=<t>`.
- Sealed text (phase 5 of the refonte, `bridge-rs/src/push_runs/seal.rs`): a device registered with its key
  (`POST /v1/devices {push_key}`, P-256 public key, base64) gets the words only in `enc` (`{v, t, title, body, sub?}`
  sealed for it: ECDH P-256 + HKDF-SHA256 « bipagents-notif-v1 » + AES-256-GCM, AAD = `agent`) and, in clear, the
  neutral `loc-key` `notif.<type>` (contracts/notifications/loc-keys.json), no title. Its phone opens it offline.
  A device without a key gets the neutral line only (no compatibility window: `[push] previews` is gone). `emit`
  stays the only place a push is built. Vectors: contracts/notifications/sealed.json.

### 9.4 Read state

```text
GET    /v1/notifications?agent=&thread=&since=<iso>&unread=1&limit=50   the Boîte's list, newest first
GET    /v1/notifications/{id}                                            one row, `preview` = its whole text
GET    /v1/notifications/unread                                          {total, agents: {a: n}, threads: {t: n}, boite: {a: n}}
POST   /v1/notifications/read   {ids?: [...], agent?, thread? (an id or main)}   marks read; answers {read, unread}
DELETE /v1/notifications/{id}                                            hidden from the Boîte (row kept)
```

- Opening a conversation sends `POST /v1/notifications/read {agent, thread}`: every row of that thread is read.
- After a read the bridge sends a silent push `reason=read` with `{agent, n: "<id>,<id>,…" (one comma-separated
  string, the same in APNs and FCM data), unread: <total>}` to the person's devices
  (they remove those delivered notifications and set the badge) and a `read` event (§6.5).
- `unread.total` is the badge (the app icon's, the Boîte tab's): what the Boîte shows and nothing else, its messages
  unread (`boite`, `reply`, `subtask`, `task`, `scan`, `family`, `question`) and the cards waiting (`approval`, `site`,
  `login`, `connector`, `secret`, `mail_card`: a card's row read when answered, and every minute when its card waits
  no more). `boite` is the same count by agent (an app adds its own agents'). `agents` and `threads` count every
  unread row: the home's dots (a room's reply, a held e-mail, a date to find are not the Boîte's). Alex's iPhone,
  5/11: « Boîte 1 » for ever on an empty Boîte (a card's row whose card had gone).
- A `boite` row's `created_at` is its message's arrival (the outbox's stamp, one per millisecond), not its push's
  (which waits for the message's audio): the apps list the Boîte by `created_at`, so its messages keep the order they
  came in. The list's tie-breaker is the row's `id`.

Journal row as served:

```json
{"id": "n_01JBX…", "agent": "vie", "type": "reply", "c": "t_01JBX…", "m": "m_3f9a…", "ref": "r_123",
 "session_id": "api_ccc", "title": "Vie", "body": "notif.reply", "preview": "Le train de 9 h part…",
 "state": "open", "created_at": "2026-11-02T21:04:12.511Z", "read_at": null, "push": "sent:2"}
```

## 10. Tests

- **Rust unit tests** in each module: lineage (continuation rule, copied tails, manual links), normalization (every
  marker), cursor paging (`before`, `after`, `around`, vanished items), journal (dedupe, rules, payload v2 + legacy).
- **Black-box** (`bridge/tests_blackbox/test_threads.py`, `test_notifications_journal.py`): the real bridge with the
  fake Hermes extended to model v0.22.5:
  - compression: a session ending with `end_reason = compression` and a child with `parent_session_id`, listed only
    with `include_children=1`;
  - `local-<sha256>` routine sessions;
  - the peer « Bot Chat » behaviour (a text run on the session titled `Bot Chat` goes to the peer FIFO: the fake
    answers it as Hermes v0.22.5 does, so tests prove the bridge never depends on that title);
  - `message_uid` on every row.
  Cases: migration adopts the old main conversation; an old notification's session resolves to the tip with every
  later message; a reply notification carries `m` of the stored row; two producers of one reply give one row; the
  switch silences replies but not questions/approvals; read sync.
- **Example responses** for the app agents: `docs/examples/*.json`, checked by a black-box test that their shape
  matches the bridge's real answers (same keys, same types).

## 11. Plan

| Step | Content | Status |
|---|---|---|
| 1 | This document | done |
| 2a | Thread registry + lineage + adoption; `/v1/threads`, `resolve`, `segments` | done (branch conv-core) |
| 2b | Timeline (normalization, paging, attachments, Boîte, answered approvals, Live calls, pending), events | done (branch conv-core) |
| 3 | Notification journal, rules, v2 payloads, read API, read sync | done (branch conv-core); `POST /v1/notifications/{id}/answer` not yet |
| 4 | Sending through the bridge (§6.7), approvals and clarify through the bridge, shared agents on the thread routes | done (branch conv-step4) |
| 4b | Rooms on the item model (§6.6): timeline, send with `client_id`, hints + presence, journal unread | done (branch rooms-items) |
| 5 | iOS, then Android, on the timeline behind the `threads` feature flag | app agents |
| 6 | Remove direct Hermes calls and marker parsers from the apps | with the apps |

## 12. Status

Kept up to date in `internal/notes/conv-core.md`.

## 13. Faster, lighter, less code (Alex: « plus rapide et plus optimisé, moins de lignes de code »)

### Requests and bytes per screen

Measured by `bridge/tests_blackbox/test_threads_bench.py` (`-s` prints the table): a main conversation of 400
messages over three segments (two compressions) and 30 other conversations, the bridge's debug build, the fake Hermes
on the same Mac, each screen's requests sent one after the other, mean of 20 rounds. Counts and bytes are exact;
times only give the order of magnitude (on a phone each request is a round trip over the tailnet or the door,
typically 50–150 ms, so the count of requests is what the person feels).

| Screen | Before: requests | Before: bytes | After: requests | After: bytes |
|---|---:|---:|---:|---:|
| Open the main conversation | 7 (3 to Hermes, 4 to the bridge) + `session_state` every 5–8 s | 42,127 (only the tip segment: older segments unreachable) | **1** | 17,463 (latest 50 items of the whole line) |
| Open around a message (notification, search hit) | 7 (the old segment fetched whole) | 41,723 | **1** | 19,420 (20 before, 50 after) |

- Server time of one timeline page (400-message line rebuilt, debug build): ~4.5 ms; on the real lab place (Hermes
  v0.22.5, the door over the internet) a timeline page answered in ~125 ms end to end; release builds are several
  times faster. The old screen's 7 requests took ~2.6 ms in all on localhost, i.e. 7 round trips on a phone.
- Per item, the timeline sends visible text, an id and a short cursor only: `segment` with `debug=1`, `turn` when it
  is another item's, `attachments` when there are some. 133 items: 45.7 kB (old: 42 kB for the same 133 Hermes rows,
  but with neither tool steps, attachments, Boîte, approvals nor calls merged: those were 4 more requests).
- Live updates: one SSE connection (`/v1/threads/events`) replaces polling: `session_state` (every 5–8 s per open
  conversation), the home screen's previews, the Boîte's refresh.

### Hermes load

- A store read (`state.db`, managed installs: every offer place) costs Hermes **no call at all**: checked by
  `test_the_hermes_store_is_read_without_calling_hermes`.
- Through Hermes' API (an existing Hermes elsewhere): closed segments are read once (Hermes never writes into a
  compressed session again), the tip again only after a run's end, a Boîte message or a notification touched the
  thread, or 30 s later; the line's continuations likewise. The bench's 60 new screens made **0** Hermes calls.
- The old screens made 3 Hermes calls each, plus the session-list pages to find « Bot Chat ».

### Bridge memory and CPU

- No copy of the conversations is kept in the bridge: the timeline is computed from Hermes' rows per request. The
  API source's cache is bounded (50,000 rows in all, oldest segments dropped first).
- Bridge RSS in the bench: 45.7 MB before, 45.8 MB after 100 screens (+0.1 MB).

### Lines of code

What this branch adds and removes in the bridge (`git diff --numstat` against its base, 2026-11-02 evening):

| Area | Added | Removed | Note |
|---|---:|---:|---|
| `threads/` (registry, lineage, timeline, routes) | 2,185 | 0 | of which ~380 unit tests |
| `notifications.rs` (journal, rules, routes) | 444 | 0 | of which ~45 unit tests |
| `push.rs` | 325 | 317 | **deleted**: the four legacy payload builders (`message_payload`, `approval_payload`, `reply_payload`, `question_payload`), every legacy field (`run_id` pseudo-ids, `session_id`, `outbox_id`, `reply_id`, `kind`, `room_id`, `email_held`, `site_request`, `login_request`, `*_request`, `meet`, `family`, `job_id`), the in-memory reply texts (`remember`, `TEXTS_KEPT`, `TEXT_TTL`) and the seven « Me prévenir » checks; one `payload` builder and one `emit` instead |
| `app.rs` | 8 | 9 | **deleted**: `GET /v1/replies/{id}` |
| `agent_shares.rs`, `message_search.rs`, wiring | ~210 | ~10 | shared agents' threads, search ids, voice replies in the Boîte |
| Tests: golden payload cases | 0 | 531 | **deleted** with the builders (`push_runs.json` 494 lines, the test's arms 37) |
| Tests: black-box and fakes | ~540 | ~100 | the suite asserts the pointer only |

The bridge grows because it now does what each app did twice. The code this makes deletable:

| Where | What goes once the apps read the timeline | Size today (estimate; the app agents confirm) |
|---|---|---:|
| iOS `ConversationModel.swift` | the 9-source merge (`show`, `loadOnce`, `restoreAttachments`, outbox/approvals/calls merging, id keeping, `ThreadCache` fallbacks), `findThread` | ~1,200 of 3,034 lines |
| iOS | `AnsweredApprovals.swift`, `MessageFocus.swift`/`MessageLocator` (except a legacy adapter), `StoppedTurn`, `SteerMarker`, `MessageText` marker stripping, `SentFiles` matching | ~700 |
| iOS | `isBotChat` / `"Bot Chat"` comparisons | 24 sites |
| Android `ChatModel.kt` | `thread()`, `restore()`, `keepAnswer()`, `showPendingApproval()`, outbox merge | ~600 of 1,633 lines |
| Android | `net/Hermes.kt` message parsing (`unwrap`, `isHandoff`, `rescueLine`, `asNotice`, `tidy`), `MessageFocus.kt` | ~350 |
| Android | `"Bot Chat"` comparisons | 18 sites |
| Bridge, step 4 | done: the apps' `/hermes/<agent>/…` runs, steers, approvals and questions, `/v1/approvals/answered`, `/v1/agents/{id}/queue` POST and `/steer`, the shared-agents conversation routes (conv-step4). Left: `/v1/live/calls` GET, `/v1/sessions/{id}/state` (web console) | — |
| Bridge, with botchat-v0225 merged | the four `"Bot Chat"` constants become `threads::LEGACY_MAIN_TITLE` (adoption only) | 6 sites |
