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
6. **Old paths keep working** during the transition: `/hermes/<agent>/…`, `/v1/outbox`, `/v1/replies/{id}`, legacy
   push fields. Nothing is removed before the oldest supported app stops using it.

## 3. Identities

| What | Id | Example | Stable across |
|---|---|---|---|
| Agent | bridge key (shared agent: `<agent>__<household>`) | `vie`, `cuisine__martin` | renames (display name is separate) |
| Thread | `t_` + 26 chars (time-sortable, random) | `t_01JBX4K9W3M2Q8R6T5Y7Z0A1B2` | titles, compressions, Hermes upgrades, bridge restarts |
| Main thread alias | `main` (accepted wherever a thread id is, with `agent`) | `?agent=vie&thread=main` | — |
| Hermes segment | Hermes session id (`api_…`, `cron_…`, `local-…`) | `api_8f2c…` | — (an implementation detail) |
| Message | `m_` + Hermes `message_uid` (32 hex) | `m_3f9a…` | compressions, restores (restore_reconcile keeps `message_uid`), re-reads |
| Message (no `message_uid`) | `m_` + first 24 hex of `sha256(agent \0 session \0 row id)` | `m_a1b2…` | re-reads (only on an install whose store has no `message_uid`) |
| Tool step | `<message id>.t<n>` (n = position of the call in the assistant row) | `m_3f9a….t0` | as its message |
| Attachment | `<message id>.a<n>` (n = position in the message) | `m_3f9a….a0` | as its message |
| Boîte message | `o_` + outbox id | `o_9c1e…` | — |
| Queue item | `q_` + queue item id | `q_5d2a…` | until it starts (then its user message has the same `client_id`) |
| Notification | `n_` + 26 chars | `n_01JBX…` | — |
| Room | `room:<room id>` (rooms keep their own ids; thread view in §6.6) | `room:r_12ab` | — |

`client_id`: an id the app chooses for a message it sends (UUID). The bridge stores it with the message once Hermes
has written the user row, so the app replaces its optimistic bubble *by id*. The same mechanism carries the
attachments the app sent: they belong to the message with that `client_id`, never to a message with similar words
(this makes the « Yes shown as an old photo » class of bug impossible).

## 4. Thread kinds

| `kind` | What | Listed by default | Created by |
|---|---|---|---|
| `main` | The agent's single ongoing conversation (« Discussion »). Exactly one per agent. | yes (first) | adoption (§8) or the first message |
| `side` | Another conversation of the person with the agent (« Historique »), including a background task's own conversation (`origin.kind = task`) | yes | the app, a task, the first sight of an unknown root session |
| `routine` | A routine's run (`cron_<job>_<date>_<time>`, `local-<sha256>` on v0.22.5). Its report goes to its origin thread (Hermes patch 0019) or to `main` as a Boîte message; the run itself is a hidden thread | no | the first sight of a routine session |
| `pair` | Two agents' own conversation (agent_pairs.rs) | no | the household link |
| `live` | A Live call's throwaway session (live_brief.rs) | no | the Live call |
| `room` | A group discussion (rooms.rs keeps its own store; the thread id is `room:<id>`) | via rooms | rooms.rs |

`hidden` threads are returned with `include_hidden=1` only. A sub-agent's run (`delegate_task`), a branch or a reset
child is never a segment of anything: it is ignored (same rules as `restore_reconcile::continuation`).

## 5. Lineage: a thread is a chain of sessions

```text
thread t_…  (kind main)
  segment 0  api_aaa   (end_reason = compression)
  segment 1  api_bbb   parent_session_id = api_aaa     ← Hermes compressed and went on here
  segment 2  api_ccc   parent_session_id = api_bbb     ← tip: the next turn runs here
```

- **Continuation rule** (one function, `threads::lineage::is_continuation`): a child session whose
  `parent_session_id` is a segment, whose `model_config` has none of `_branched_from`, `_delegate_from`,
  `_reset_from`, whose `source` is not `tool`. Among several, the latest started. Same rule as
  `restore_reconcile.rs` (moved there from it, shared).
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
CREATE TABLE thread_messages (    -- what the bridge knows about a message that Hermes doesn't keep
  agent TEXT NOT NULL,
  message_id TEXT NOT NULL,       -- m_…
  thread_id TEXT NOT NULL,
  client_id TEXT,                 -- the app's id for a message it sent through the bridge
  attachments TEXT,               -- JSON: files the app sent with it [{id, kind, name, mime, size, path}]
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
  → {threads: [Thread…]}                  main first, then by last activity
GET  /v1/threads/{thread}?agent=<id>      (agent required only with thread=main)
  → Thread  (+ segments)
GET  /v1/threads/resolve?agent=<id>&session_id=<hermes session>
GET  /v1/threads/resolve?agent=<id>&c=<pointer>          (c: thread id, `main`, `room:<id>`)
  → {thread: Thread, tip: <session id>}
POST /v1/threads/{thread}/segments  {session_id, agent?}  (migrations, tests)
  → Thread
```

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
  "last": {"id": "m_3f9a…", "role": "agent", "preview": "Le train de 9 h part…", "at": "2026-11-02T21:04:10.000Z"},
  "unread": 2
}
```

`segments` is given by `GET /v1/threads/{thread}` and `resolve` only. `unread` counts the thread's unread journal
items (§9.4).

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
    "run": {"run_id": "r_123", "status": "running"},
    "queue": {"version": 3, "paused": false, "items": [{"id": "q_…", "text": "…", "created_at": "…"}]},
    "approvals": [{"run_id": "r_123", "request_id": "p_9", "command": "…", "choices": ["once", "deny"]}],
    "questions": [{"run_id": "r_123", "request_id": "c_4", "question": "…", "choices": ["…"]}]
  }
}
```

- `items` are ordered as the conversation reads: segments in order, Hermes rows in their display order, the bridge's
  own items (Boîte messages) placed by time among them.
- A **cursor** is opaque to the apps (today `<segment position>.<row order>.<item id>`). The apps keep the `last`
  cursor of what they have and ask `after=` on each `thread` event. An item that moved or vanished (a restore, a
  rewind) still pages correctly: the cursor's position is used when its item is gone.
- `anchor` is the `around` item when it was found (else null and the latest page is given).
- `pending` is what is not a stored message yet: the run going on (attach to `/v1/runs/{run_id}/events` for its live
  text, as today), the agent's queue for this thread (msg_queue.rs), approvals and questions waiting. Once a run ends
  its messages are items; the app drops its live bubble when an item with the run's reply arrives (`run_id` on the
  agent's final message when the bridge knows it).

### 6.3 Items

Every item has `id`, `kind`, `at` (ISO 8601 UTC), `cursor`, `turn` (the id of the person's message that started this
turn; the first item's own id before any) and `segment` (Hermes session id, for the transition and debugging only).

| `kind` | Fields | From |
|---|---|---|
| `message` | `role` (`person` \| `agent`), `text` (visible words only), `attachments`, `client_id`?, `added_during_run`? (a steer), `reasoning`? , `run_id`? | Hermes user / assistant rows |
| `tool` | `tool: {name, call_id, arguments, result, status (done \| failed \| running), via}` | assistant `tool_calls` + their tool rows |
| `notice` | `notice: {code, text}`: `failed_turn` (Hermes' failed-turn row), `loop_rescue` (the bridge's rescue turn), `stopped` | Hermes rows the bridge recognizes |
| `boite` | `text`, `title`, `attachments`, `boite: {outbox_id, audio: bool}` | the bridge's outbox (routine reports, hand-backs, credit, storage, imports) |

Never items: hidden rows (`display_kind` hidden, inactive rows unless compacted, compression summaries), the bridge's
own steers (`[bipagents:…]`), restore notes (`<!--bip:restore…-->`), user rows made only of `<!--bip:…-->` notes.

Attachment:

```json
{"id": "m_3f9a….a0", "kind": "image", "name": "photo.jpg", "mime": "image/jpeg", "size": 184223,
 "path": null, "url": "/v1/threads/t_…/attachments/m_3f9a….a0"}
```

`kind`: `image` (a photo in the message: served by the URL), `document` (`[Pièce jointe : …]` lines: `path` on the
server), `file` / `page` / `map` / `audio` (the agent's `MEDIA:` lines: `path`, served by the existing file routes;
`offset`: where the line was in `text`, for inline pages). An app sends a file with `client_id`; the bridge returns
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

### 6.4 One item, attachments

```text
GET /v1/threads/{thread}/items/{item}?agent=   → Item (404 when gone)
GET /v1/threads/{thread}/attachments/{id}?agent=  → the bytes (image parts of a Hermes row, decoded); a path
                                                    attachment redirects (307) to the file route that serves it
```

### 6.5 Events

```text
GET /v1/threads/events[?agent=<id>]      SSE, keepalive comment every 10 s
event: thread         data: {"thread": "t_…", "agent": "vie", "version": 43, "reason": "run|message|boite|queue|approval|segment|title|read"}
event: notification   data: {"id": "n_…", "agent": "vie", "t": "reply", "c": "t_…", "m": "m_…", "state": "open"}
event: read           data: {"agent": "vie", "thread": "t_…", "notifications": ["n_…"], "until": "m_…"}
```

On `thread`, an app showing that thread asks `timeline?after=<its last cursor>`; a list screen refetches
`/v1/threads`. Events are hints: an app that missed some (asleep, offline) compares `version` and refetches. The live
text of a run still comes from `/v1/runs/{run_id}/events` (unchanged).

### 6.6 Rooms and shared agents

- **Rooms** keep rooms.rs' store and routes for now; their pointer is `room:<id>` (resolve answers it, notifications
  carry it). Moving rooms onto the same item model is a later step.
- **Shared agents**: the relative's bridge owns the thread; the person's app reaches it through the existing
  household proxy with `agent = <agent>__<household>`. Same API, same ids, minted by the owner bridge.

## 7. How the apps use it (target)

- **One cache per thread, keyed by item id.** Merge rule: an item with a known id replaces the cached one; a new id is
  inserted at its cursor. No text, date or position comparison anywhere. The optimistic bubble of a sent message
  carries the `client_id`; the server item with that `client_id` replaces it.
- **Opening a conversation**: `timeline?limit=50` (or `around=<m>` from a notification, a Boîte row, a search hit).
  Scroll up: `before=<first>`. Live: the `thread` event → `after=<last>`. Version mismatch after sleep → `after=`.
- **No Hermes calls** for conversations (end state): list, history, approvals and clarify go through the bridge.
  `/hermes/<agent>/…` stays for older apps.
- **No marker parsing**: `text` is already visible text; attachments are structured.
- **Badges and read state** come from the journal (§9.4).

## 8. Migration of existing places

Runs once per agent, at the first need (lazy), never blocking a request more than one Hermes listing:

1. If the agent has a `main` thread, done.
2. Else find the main conversation's current session, in this order:
   a. the session botchat-v0225's migration names as the person's main conversation (when that branch lands, its
      record is read here; until then not present);
   b. the newest non-hidden session titled `Bot Chat` (case-insensitive), then a hidden one;
   c. none: the main thread is created empty and gets its first segment when the first turn is started through the
      bridge, or when the app opens a session for it (`POST /v1/threads/{id}/segments`).
3. The chosen session's whole line (parents up to the root, continuations down to the tip) becomes the main thread's
   segments (`linked = adopted` for the first, `compression` for the others).
4. Other sessions are adopted lazily as `side` / `routine` threads the first time they are listed or resolved.

After the botchat-v0225 move, the main thread keeps its id: the new session is appended as a `moved` segment. Titles
are free again: the bridge no longer looks for `Bot Chat` once the main thread exists.

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
  preview TEXT,                  -- first words (kept on the bridge; sent in clear only when previews are on)
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

One table (`notifications::RULES`, served by `GET /v1/notifications/rules`), not scattered `if`s:

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

Presence (the conversation on screen) is still decided by the producers, as today.

### 9.3 Payload v2 (pointer) alongside v1

Every alert push gets, next to its legacy fields (unchanged, for apps that don't read v2):

```json
{"v": 2, "n": "n_01JBX…", "t": "reply", "c": "t_01JBX…", "m": "m_3f9a…"}
```

- `c` absent: no conversation (a card for the Boîte, a family note: the app opens the agent or the Boîte).
- `m` absent: the message was not stored within the wait; the app opens the conversation at its end, then asks the
  journal (`GET /v1/notifications/{n}`), which has `m` once found.
- Collapse id: one per journal row (`n`) for replies, sub-task reports, tasks and Boîte messages (an autonomous reply
  no longer replaces the previous one); approvals and questions keep their request id.
- The deep link the apps build from it: `bipagents://open?agent=<a>&c=<c>&m=<m>&n=<n>&t=<t>`.
- Sealed text (phase 5 of the refonte, `bridge-rs/src/push_runs/seal.rs`): a device registered with its key
  (`POST /v1/devices {push_key}`, P-256 public key, base64) gets the words only in `enc` (`{v, t, title, body, sub?}`
  sealed for it: ECDH P-256 + HKDF-SHA256 « bipagents-notif-v1 » + AES-256-GCM, AAD = `agent`) and, in clear, the
  neutral `loc-key` `notif.<type>` (contracts/notifications/loc-keys.json), no title. Its phone opens it offline.
  Devices without a key get the legacy alert. Vectors: contracts/notifications/sealed.json.

### 9.4 Read state

```text
GET    /v1/notifications?agent=&thread=&since=<iso>&unread=1&limit=50   the Boîte's list, newest first
GET    /v1/notifications/{id}                                            one row (with `preview`): replaces /v1/replies
GET    /v1/notifications/unread                                          {total, agents: {a: n}, threads: {t: n}}
POST   /v1/notifications/read   {ids?: [...], agent?, thread?, until?}   marks read; answers {read: [ids], unread}
DELETE /v1/notifications/{id}                                            hidden from the Boîte (row kept)
GET    /v1/notifications/rules                                           the table of §9.2
```

- Opening a conversation sends `POST /v1/notifications/read {agent, thread}`: every row of that thread is read.
- After a read the bridge sends a silent push `reason=read` with `{agent, n: [ids], unread}` to the person's devices
  (they remove those delivered notifications and set the badge) and a `read` event (§6.5).
- `unread.total` is the badge.

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
| 2a | Thread registry + lineage + adoption; `/v1/threads`, `resolve`, `segments` | §12 |
| 2b | Timeline (normalization, paging, attachments, Boîte merge, pending), events | §12 |
| 3 | Notification journal, rules, v2 payloads, read API, read sync | §12 |
| 4 | Sending through the bridge (`POST /v1/threads/{id}/messages {text, client_id, attachments}`), approvals and clarify through the bridge | later |
| 5 | iOS, then Android, on the timeline behind the `threads` feature flag | app agents |
| 6 | Remove direct Hermes calls and marker parsers from the apps; drop `/v1/replies` after the minimum app version | later |

## 12. Status

Kept up to date in `internal/notes/conv-core.md`.
