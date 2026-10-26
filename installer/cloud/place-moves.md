# Planned place moves: the contract between the guichet and the seat agents

Decided 2026-10-26. A planned move takes one place (one account `bip<n>` with Hermes, the bridge, the agents, their
routines, background tasks, browser profiles and files) from one offer server to another while its agents keep
working. Unavailability is about one minute, and nothing is lost. The guichet uses moves to rebalance servers,
starting with calm people, and to empty a server before maintenance.

The place's side is built: `installer/cloud/place-move.py` runs as root, `installer/bipinstall/placemove.py` runs as
the place, and `bridge-rs/src/place_move.rs` is the bridge's part. This document says what the guichet must add.

## The orders

They are signed like the other orders (Ed25519, `body` and `signature`, `order` = the order's id, `server`, `seat`,
`expires`) and delivered in the answer to `POST /v1/server/beat`. `seat-agent.py` checks the fields below, then hands
the order to `place-move.py`.

### `move_in`, to the target server (send it first)

```json
{"order": 41, "server": "vps-7", "seat": 3, "action": "move_in", "expires": 1793030000,
 "move": {"id": "m-5f3a9c1e2b", "key": "<43 base64url characters: 32 random bytes>",
          "source": {"address": "198.51.100.4"}, "port": 2222, "expires": 1793040000}}
```

- `seat`: a **ready, unclaimed** seat on the target server, installed by a `reset` with a pairing secret that nobody
  will use. Its door and ports stay the target's own. The person's files arrive over it, and so do the bridge key and
  certificate.
- `move.id`: 6 to 63 characters from `[a-z0-9-]`. It must not start with `-`. Use the same id in both orders.
- `move.key`: the one-time key, made fresh for each move and sent in both orders. Both servers derive two Ed25519 keys
  from it (HMAC-SHA256 with the labels `bipagents-move/host` and `bipagents-move/client`):
  - the host key of a dedicated `sshd` that the target starts for this move;
  - the client key that only the source uses.

  The data goes from server to server inside that SSH channel. It never passes through the guichet or any other
  Kerfy machine. Forget the key once both orders are done.
- `move.source.address`: the source server's public IPv4 or IPv6 address. Only this address may connect: the `from=`
  option in `authorized_keys` and a `ufw allow from` rule both restrict it.
- `move.port`: the listener's port (default 2222, allowed range 1024-65535, never a place's door 443-450). The
  provider's own firewall must let this port through between the two servers. DigitalOcean, OVH and Hetzner have no
  such firewall by default.
- `move.expires`: when the listener closes on its own if nothing finished. Default: 6 hours.

The target answers "done" at once. The listener then waits for the source.

### `move_out`, to the source server

```json
{"order": 42, "server": "vps-2", "seat": 2, "action": "move_out", "expires": 1793030000,
 "move": {"id": "m-5f3a9c1e2b", "key": "<the same key>",
          "target": {"address": "203.0.113.9", "port": 2222},
          "window": {"start": 1793026800, "end": 1793030400}, "back_in": 60}}
```

- `window` (optional): Unix seconds. The copies start at once. The switch happens inside the window, at the first
  moment with no turn running, and no later than 5 minutes before the window ends. Without a window, the switch
  happens within the next 30 minutes.
- `back_in`: the number of seconds the apps are told to wait (`Retry-After`, « de retour dans une minute »).

The order runs in a unit of its own (`bipagents-move-<id>`), so it can last from minutes to hours. The seat agent
answers "done" once the unit has started. The move's progress comes in the beats.

### `move_cancel`, to either server

```json
{"order": 43, "server": "vps-2", "seat": 2, "action": "move_cancel", "move": {"id": "m-5f3a9c1e2b"}, "expires": ...}
```

- On the source, before the switch: the copies stop. A drained place resumes: tasks and routines go on, and the apps'
  writes are accepted again.
- On the target: the listener closes. Afterwards, reset the target seat with a `reset` order.

### `remove`, to the source server (the guichet's `pool::vacate`)

This order clears the place a move left behind:

- it stops the place's services;
- it deletes the account and all its data (`wipe`, the same one a `reset` uses);
- it frees the door: it deletes the `ufw allow <442+n>/tcp` rule and `/etc/bipagents/cloud<n>.json`.

```json
{"order": 44, "server": "vps-2", "seat": 2, "action": "remove", "expires": ..., "move": {"id": "m-5f3a9c1e2b"}}
{"order": 45, "server": "vps-2", "seat": 2, "action": "remove", "expires": ..., "removed": true}
```

The seat agent accepts the order in two cases.

- **With `move.id`.** The order runs only if this seat's `move_out` with that id reached `done` (or `removed`) on
  this server, and the account still holds that move's mark (`~/.config/bipagents/moved-away.json`). If the seat was
  installed again for someone else in the meantime, the order is refused and logged. The move's state then becomes
  `removed`.
- **With `removed: true`.** The guichet marks the seat as removed, so the seat agent trusts the order without a move
  (an operator's decision).

The order is idempotent: if the account is already gone, the order still counts as done and nothing else happens.
Without a `remove` order, the seat agent deletes the source copy itself 7 days after `done` (`cleanup`), under the
same mark check.

## What the beats report

`POST /v1/server/beat` gets a new top-level `moves` list, one entry per move known on the server:

```json
"moves": [{"id": "m-5f3a9c1e2b", "role": "out", "seat": 2, "phase": "copying", "round": 2,
           "copied_bytes": 812345678, "delta_bytes": 1234567, "files": 42, "started": 1793026000,
           "updated": 1793026300}]
```

`phase` goes through these values.

On the source (`role` = `"out"`):

| Phase | Meaning |
|---|---|
| `connecting` | waiting for the target's listener |
| `copying` | rsync rounds while the place runs |
| `waiting` | waiting for the window and a quiet moment (a new round every 2 minutes) |
| `draining` | the apps' writes are refused, tasks are pausing, routines are postponed |
| `switching` | the place is stopped, the last copy is made with checksums, the target starts it |
| `done` | ends with `downtime_seconds`, `drain_seconds`, `verified` and `kept_until`; the source copy is kept stopped 7 days |
| `failed` | `error` and `result: "rolled back"`: the source runs again as before, and the target copy is stopped |
| `cancelled` | the move was called off |
| `removed` | the source account was deleted after `kept_until` |

On the target (`role` = `"in"`):

| Phase | Meaning |
|---|---|
| `listening` | the listener waits for the source |
| `receiving` | the source has connected and the files are arriving |
| `starting` | the target is adjusting the place and starting it |
| `done` | with `verified` |
| `failed` | the target copy did not start |
| `cancelled` | the move was called off |
| `closed` | the listener has closed |

`verified` holds: the bridge answers, every agent's gateway answers, the number of routines scheduled (never fewer
than before), the tasks resumed, the routines caught up, the files whose owners were moved, and the volumes imported.

Moves also add new fields that the guichet can use to choose calm people. The bridge's
`GET /v1/place/move` reports the following, with the bridge key:

- `turns_running`
- `jobs_running`
- `routines` (each with its `next_run_at`)
- `next_routine_at`
- `quiet`

The seat agent does not put these in the beat yet, because asking would wake sleeping places. A sleeping place
(`asleep: true` in the beat) is the calmest anyway.

## What the guichet does

1. Choose the person. Prefer people whose place is asleep or has a backup and no recent activity.
2. Choose a target server and a ready seat on it, and mark that seat as reserved for the move.
3. Make the key and the move id. Send `move_in` to the target first, then `move_out` to the source in the same round.
4. Follow the beats. While the source reports `draining` or `switching`, say `setup: "moving"` and `planned: true`
   for this place in `GET /v1/places/mine` and the subscription check. The apps then show « Ton espace déménage, de
   retour dans une minute » and hold the person's sends.
5. On `done`:
   - Point the person's place at the target: `seats` row, door address and port. The backup repository and its token
     stay the person's: the target place keeps its own seat token and has the person's key, which came over with
     the files.
   - Tell the phones MOVED through the push relay, as for an outage move. The apps follow the new door with the same
     key and pin.
   - Free the source seat after `kept_until`, when its account is deleted. Do not send it a `reset` before then if
     the copy should stay as a way back.
6. On `failed`: nothing moved, and the person is still on the source. Reset the target seat, then try again later or
   on another server.
7. If the target server dies during the move, the source is unaffected unless it is in `switching`. In that case the
   source rolls back on its own when `finish` fails.

## Choices

- **rsync over a one-time SSH channel, not restic to a target repository.** rsync sends only the changed parts of the
  changed files. Each pre-copy round lands directly where the target place will run, so the switch only copies the
  last minute's changes. There is no restore step during the downtime and no repository to clean up afterwards.
  restic would add a repository's encryption and an extraction during the downtime. The SSH channel already
  encrypts and authenticates both ends with keys derived from the guichet's one-time key, with no fingerprint to
  relay.
- **The last copy runs with the place stopped, with checksums.** SQLite write-ahead logs (`-wal`, `-journal`) are
  copied with their databases. A mid-second write cannot hide behind an identical size and modification time.
- **Owners are moved after the copy.** rsync keeps the source's numeric ids. The target then maps the place's own uid
  and gid, and its sandboxes' subordinate range, to its own.
- **The target keeps its own door, ports, setup files and backup token** (`backup.fresh_state` then `backup.settle`,
  shared with the restore after an outage). The person's bridge key, TLS certificate, phones' keys and backup key
  come with the files, so the apps keep their pin.
- **The drain lives in the bridge.** The state is in `<data_dir>/place-move.json`, which moves with the place. The
  target's bridge therefore starts in « moving » until the move's last step resumes it, so no app write can reach a
  half-started place.
