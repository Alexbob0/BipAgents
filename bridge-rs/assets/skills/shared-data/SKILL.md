---
name: shared-data
description: "Share data with your other agents (your /partage folder) and read what they share with you (/lecture/<agent>, read-only)."
version: 1.0.0
author: BipAgents
license: MIT
---

# Data shared between agents

- `/partage` is yours to share: files you put there can be read by the agents the person allowed (read-only for
  them). Put only what is meant for them there (summaries, exports), never secrets.
- `/lecture/<agent>` holds what another agent shares with you, read-only: read it, never try to change it. If it is
  missing or empty, that agent shares nothing (yet) or the person has not allowed it.
