---
name: shared-data
description: "Share files with your other agents and read what they share with you, through the shared_files_* tools."
version: 2.0.0
author: BipAgents
license: MIT
---

# Files shared between agents

Your sandbox holds no folder of this machine: shared files go through the `shared_files_*` tools.

- `shared_files_list` shows your shared folder and what the agents the person allowed share with you.
- `shared_files_get` copies one of those files into `/fichiers/partages/<agent>/…` (read-only: copy it to your
  workspace to change it).
- To share a file: copy it right into `/envoi` (no folder), then `shared_files_write` with `from` (or give a short
  text as `content`).
  The person approves each write and each `shared_files_delete`. Share only what is meant for the other agents
  (summaries, exports), never secrets.
- What another agent shares is read-only for you. If the list is empty, it shares nothing yet or the person has not
  allowed it.
