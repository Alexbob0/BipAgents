---
name: shared-data
description: "Share data with your other agents (your /partage folder), read what they share with you (/lecture/<agent>, read-only), and give them files (send_files_to_agent)."
version: 3.1.0
author: BipAgents
license: MIT
---

# Data shared between agents

Your sandbox holds no folder of this machine. It has its own: `/fichiers` (what your person sent you, read-only),
`/envoi` (to hand files out) and, where the person set up sharing, the shared folders below.

- `/partage` is yours to share: write there directly, a routine too (nobody needs to approve it). Only the agents
  the person allowed read it, read-only (and the agents of a group discussion its `salons/<discussion>/` folder):
  no other agent sees it. Put only what is meant for them there (summaries, exports), never secrets.
- `/lecture/<agent>` holds what another agent shares with you, read-only: read it there, never try to change it. If
  it is missing or empty, that agent shares nothing (yet) or the person has not allowed it.
- These folders exist only where the person set up sharing: if one is missing, don't look for it, don't create it.
- The `shared_files_*` tools reach the same folders: `shared_files_list` shows them all, `shared_files_get` copies a
  file into `/fichiers/partages/<agent>/…`, `shared_files_write` writes into yours (from a file copied right into
  `/envoi`, or a short text as `content`). The person approves each `shared_files_delete`.
- To give files to one agent, use `send_files_to_agent`, not your shared folder: that agent never sees your
  folders, nor a path of yours you write to it. Copy a file you made right into `/envoi` first; the tool says the
  path that agent reads it at. Then write to it with `message_agent`, giving that path. No approval is needed.
- To your person, just say what you shared or passed on, never these folders or tools.
