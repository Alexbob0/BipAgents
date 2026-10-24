🇬🇧 English · [🇫🇷 Français](grok-bot.fr.md) · [🇩🇪 Deutsch](grok-bot.de.md) · [🇪🇸 Español](grok-bot.es.md)

# Coming from Grok Bot (or ChatGPT dots, Muse, another agent)

Grok Bot has no export button: the only way to take what it knows with you is to ask it to write everything out.
The message below makes it write its role, its memory, the people, the tasks, the routines and the skills into
files, then put them in a zip you download.

1. Copy the message and paste it into your Bot's conversation (one Bot at a time).
2. Download the `bipagents-export.zip` file it attaches to its answer. Read it through if you like: you can remove
   what you don't want to bring.
3. In BipAgents: **Add an agent › Coming from another assistant › Import the file**. The app shows you everything it
   will create before anything is saved.

What the Bot writes is its own version of its memory: it may be incomplete. Skills arrive switched off and routines
are not scheduled again without your yes. Nothing that looks like a password, a key or a card number is kept.

## The message

```text
I'm switching assistants and I want to take everything with me. Create a folder bipagents-export in your workspace and write these Markdown files into it, as completely as you can:

1. SOUL.md: your role, your personality, your tone and every standing rule I gave you, in the second person ("You are…", "You never…").
2. USER.md: everything you know about me: who I am, my situation, my tastes, my preferences, how I write.
3. MEMORY.md: everything else in your memory, one fact per line starting with "- ", with the date (YYYY-MM-DD) when you know it.
4. people.md: every person you know, a section "## Name" with how they relate to me and what to know about them.
5. tasks.md: ongoing tasks and projects, one "- " line each, with the deadline and where it stands.
6. routines.md: every routine or scheduled task, a section "## Name" with the exact schedule and time zone, the full instructions, what it produces and the apps it uses.
7. skills/<name>/SKILL.md: every skill or procedure you learned, in full. Start with:
---
name: <lowercase-name-with-dashes>
description: <when to use it, in one sentence>
---
then when to use it, what it needs to start, the numbered steps, how to check and what you deliver.
8. apps.md: the names of the apps and services you are connected to, one per line, no credentials.
9. conversations.md: a summary per topic of our past conversations, a section "## Topic (period)" each, with the decisions made.

Rules:
- Be exhaustive: too much is better than too little. Take several passes if needed. A file with nothing in it: write "(nothing)".
- Don't invent anything. Add "(uncertain)" to what you are not sure of.
- NEVER write a password, code, token, API key, card or bank account number: put "[removed]" instead.
- Write in the language I speak to you in.

Then make a single zip of that folder named bipagents-export.zip, attach it to your answer so I can download it (if you can't make a zip, attach the files one by one) and answer "Ready" with the number of files.
```
