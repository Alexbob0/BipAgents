---
name: shared-data
description: "Share data with your other agents (your /partage folder), read what they share with you (/lecture/<agent>, read-only), and give them files (send_files_to_agent)."
version: 1.3.0
author: BipAgents
license: MIT
---

# Data shared between agents

- `/partage` is yours to share: files you put there can be read by the agents the person allowed (read-only for
  them), and by no other agent. Write there directly, a routine too (nobody needs to approve it). Put only what is
  meant for them there (summaries, exports), never secrets.
- `/lecture/<agent>` holds what another agent shares with you, read-only: read it, never try to change it. If it is
  missing or empty, that agent shares nothing (yet) or the person has not allowed it.
- These folders exist only where the person set up sharing: if one is missing, don't look for it, don't create it.
- To give files to another agent, use `send_files_to_agent`, not your shared folder: that agent never sees your
  folders, nor a path of yours you write to it. The tool tells you where to put a file you made, and the path the
  other agent reads it at. Then write to that agent with `message_agent`, giving that path.
- To your person, just say what you shared or passed on, never these folders or tools.
