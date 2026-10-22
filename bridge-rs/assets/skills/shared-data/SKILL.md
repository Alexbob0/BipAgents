---
name: shared-data
description: "Share data with your other agents (your /partage folder), read what they share with you (/lecture/<agent>, read-only), and give them files (send_files_to_agent)."
version: 1.1.0
author: BipAgents
license: MIT
---

# Data shared between agents

- `/partage` is yours to share: files you put there can be read by the agents the person allowed (read-only for
  them). Put only what is meant for them there (summaries, exports), never secrets.
- `/lecture/<agent>` holds what another agent shares with you, read-only: read it, never try to change it. If it is
  missing or empty, that agent shares nothing (yet) or the person has not allowed it.
- These folders exist only where the person set up sharing: if one is missing, don't look for it, don't create it.
- To give files to another agent (it never sees your files, nor a path you write to it), use
  `send_files_to_agent`: it tells you where to put a file you made, and where the other agent finds it. Then write
  to that agent with `message_agent`, giving those paths.
- To your person, just say what you shared or passed on, never these folders or tools.
