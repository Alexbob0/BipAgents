#!/usr/bin/env python3
# Does an agent hand long work to a background task? (decided 2026-10-22: LONG_WORK_RULE in souls.rs, start_task /
# notebook_fill / propose_task, the bridge's steer after ~15 tool calls or ~3 minutes in runs.rs.) Not run in CI: it
# talks to a real install and spends a little of the model's credit.
#
# For each typical prompt it opens a conversation of its own (« 🧪 Eval tâches de fond »), sends the prompt through the
# bridge's relay to the agent's Hermes, follows the run through the bridge (so the bridge's safety net applies, as for
# the app), then reads the agent's background tasks and reports whether one was started, proposed, or neither, against
# what was expected. Started tasks are cancelled and proposals forgotten right away (--keep to leave them), the eval
# conversations deleted at the end (--keep-sessions to read them in the app).
#
# On the install (e.g. aibox, as the person whose bridge it is):
#
#     python3 bridge/tools/longwork-eval.py --agent vie
#     python3 bridge/tools/longwork-eval.py --agent vie --lang en --only 1,6 --json
#
# Keys come from the bridge's own config (--config, default $BRIDGE_CONFIG or ~/.config/bipagents/bridge.toml, read
# only here, never printed); --bridge is the bridge's address (default http://127.0.0.1:<port from the config>).
# Python 3.11+ (tomllib) or tomli; nothing else.
import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request

# (prompt fr, prompt en, expected): started = a task started at once (start_task, notebook_fill); proposed = proposed
# (propose_task); either = started or proposed; none = answered in the conversation.
PROMPTS = [
    ("Remplis mon Carnet avec tout ce que tu sais de moi.",
     "Fill my Carnet with everything you know about me.", "started"),
    ("Trie mes mails de la semaine et dis-moi ce qui est important.",
     "Sort this week's e-mails and tell me what matters.", "either"),
    ("Fais une recherche approfondie sur les meilleures mutuelles santé pour une famille de quatre en France.",
     "Do a deep research on the best health insurance for a family of four in France.", "started"),
    ("Compare 10 appartements à louer à Lyon, 3 pièces, moins de 1200 € par mois.",
     "Compare 10 flats to rent in Lyon, 3 rooms, under 1,200 € a month.", "started"),
    ("Prépare-moi un dossier complet d'une trentaine de pages sur l'histoire de l'Union européenne, avec les sources.",
     "Prepare a full thirty-page report on the history of the European Union, with sources.", "either"),
    ("Quelle heure est-il à Tokyo en ce moment ?", "What time is it in Tokyo right now?", "none"),
    ("Donne-moi une idée de dîner rapide pour ce soir.", "Give me a quick dinner idea for tonight.", "none"),
    ("Rappelle-moi en une phrase ce qu'est le Carnet.", "Remind me in one sentence what the Carnet is.", "none"),
]
SESSION_TITLE = "🧪 Eval tâches de fond"
TERMINAL = ("run.completed", "run.failed", "run.cancelled", "run.canceled", "run.interrupted")


def load_toml(path):
    try:
        import tomllib
    except ImportError:  # Python < 3.11
        import tomli as tomllib
    with open(path, "rb") as fh:
        return tomllib.load(fh)


def secret(table, key):
    value = (table.get(key) or "").strip()
    if not value and table.get(f"{key}_file"):
        with open(os.path.expanduser(table[f"{key}_file"])) as fh:
            value = fh.read().strip()
    return value


def find_agent(config, wanted):
    for key, table in (config.get("agents") or {}).items():
        if wanted.lower() in (key.lower(), str(table.get("display_name", "")).lower()):
            return key.lower(), table
    sys.exit(f"no agent « {wanted} » in the bridge's config (agents: {', '.join(config.get('agents') or {})})")


class Client:
    def __init__(self, bridge, bridge_key, agent, hermes_key, timeout):
        self.bridge, self.bridge_key, self.agent, self.hermes_key, self.timeout = bridge, bridge_key, agent, hermes_key, timeout

    def call(self, method, path, body=None, hermes=False, timeout=30):
        url = f"{self.bridge}/hermes/{self.agent}/{path.lstrip('/')}" if hermes else f"{self.bridge}{path}"
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(url, data=data, method=method)
        request.add_header("Authorization", f"Bearer {self.hermes_key if hermes else self.bridge_key}")
        if data is not None:
            request.add_header("Content-Type", "application/json")
        with urllib.request.urlopen(request, timeout=timeout) as response:
            text = response.read().decode() or "{}"
        return json.loads(text)

    def jobs(self):
        return {j["id"]: j for j in self.call("GET", f"/v1/jobs?agent={self.agent}")["jobs"]}

    def follow(self, run_id, session_id):
        """The run's events through the bridge until it ends: (tool calls, steered, reply, seconds, end)."""
        url = f"{self.bridge}/v1/runs/{run_id}/events?agent={self.agent}&session_id={session_id}"
        request = urllib.request.Request(url, headers={"Authorization": f"Bearer {self.bridge_key}",
                                                       "Accept": "text/event-stream"})
        tools, steered, reply, end, kind = [], False, "", "timeout", None
        started = time.monotonic()
        with urllib.request.urlopen(request, timeout=self.timeout) as response:
            for raw in response:
                if time.monotonic() - started > self.timeout:
                    break
                line = raw.decode("utf-8", "replace").rstrip("\n")
                if line.startswith("event: "):
                    kind = line[7:]
                    continue
                if not line.startswith("data: "):
                    continue
                try:
                    data = json.loads(line[6:])
                except ValueError:
                    continue
                if kind == "tool.started":
                    tools.append(str(data.get("tool") or data.get("name") or "?"))
                elif kind and "steer" in kind:
                    steered = True
                elif kind in TERMINAL:
                    reply, end = str(data.get("output") or ""), kind
                    break
        return tools, steered, reply, time.monotonic() - started, end


def outcome(new_jobs):
    started = [j for j in new_jobs if j.get("state") != "proposed"]
    proposed = [j for j in new_jobs if j.get("state") == "proposed"]
    if started:
        return "started", started[0]
    if proposed:
        return "proposed", proposed[0]
    return "none", None


def passed(expected, got):
    return got == expected or (expected == "either" and got in ("started", "proposed"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", required=True, help="the agent's id or name")
    parser.add_argument("--config", default=os.environ.get("BRIDGE_CONFIG", "~/.config/bipagents/bridge.toml"))
    parser.add_argument("--bridge", help="the bridge's address (default http://127.0.0.1:<port>)")
    parser.add_argument("--lang", choices=["fr", "en"], default="fr")
    parser.add_argument("--only", help="prompt numbers, e.g. 1,6")
    parser.add_argument("--timeout", type=float, default=600, help="seconds per prompt at most")
    parser.add_argument("--keep", action="store_true", help="leave started tasks running and proposals waiting")
    parser.add_argument("--keep-sessions", action="store_true", help="keep the eval conversations")
    parser.add_argument("--json", action="store_true", help="the results as JSON lines")
    args = parser.parse_args()

    config = load_toml(os.path.expanduser(args.config))
    agent, table = find_agent(config, args.agent)
    bridge = (args.bridge or f"http://127.0.0.1:{config.get('port', 8643)}").rstrip("/")
    client = Client(bridge, secret(config, "bridge_key"), agent, secret(table, "hermes_key"), args.timeout)
    chosen = {int(n) for n in args.only.split(",")} if args.only else set(range(1, len(PROMPTS) + 1))

    results, sessions = [], []
    try:
        for number, (fr, en, expected) in enumerate(PROMPTS, start=1):
            if number not in chosen:
                continue
            prompt = fr if args.lang == "fr" else en
            before = client.jobs()
            session = client.call("POST", "api/sessions", {"title": SESSION_TITLE}, hermes=True)
            session_id = (session.get("session") or session).get("id") or session.get("session_id")
            sessions.append(session_id)
            run = client.call("POST", "v1/runs", {"input": prompt, "session_id": session_id}, hermes=True)
            try:
                tools, steered, reply, seconds, end = client.follow(run["run_id"], session_id)
            except (urllib.error.URLError, TimeoutError) as exc:
                tools, steered, reply, seconds, end = [], False, "", 0.0, f"error: {exc}"
            time.sleep(1)  # a task started by the last tool call is on disk by now
            new = [j for i, j in client.jobs().items() if i not in before and j.get("kind") == "task"]
            got, job = outcome(new)
            result = {"n": number, "prompt": prompt, "expected": expected, "got": got, "ok": passed(expected, got),
                      "job": job and {"title": job.get("title"), "template": job.get("template"),
                                      "max_tokens": job.get("max_tokens"), "by": job.get("proposed_by")},
                      "tool_calls": len(tools), "tools": tools[:30], "steered": steered,
                      "seconds": round(seconds, 1), "end": end, "reply": reply[:200]}
            results.append(result)
            if not args.keep:
                for j in new:
                    client.call("DELETE", f"/v1/jobs/{j['id']}")
            if args.json:
                print(json.dumps(result, ensure_ascii=False), flush=True)
            else:
                mark = "OK  " if result["ok"] else "FAIL"
                print(f"{mark} {number}. {prompt}\n     expected {expected}, got {got}"
                      + (f" ({job.get('title')}, {job.get('max_tokens'):,} tokens)" if job else "")
                      + f" · {len(tools)} tool calls · {seconds:.0f} s · {end}"
                      + (" · steered by the bridge" if steered else "")
                      + f"\n     « {reply[:160]} »", flush=True)
    finally:
        if not args.keep_sessions:
            for session_id in sessions:
                try:
                    client.call("DELETE", f"api/sessions/{session_id}", hermes=True)
                except urllib.error.URLError:
                    pass
    ok = sum(r["ok"] for r in results)
    if not args.json:
        print(f"\n{ok}/{len(results)} as expected")
    return 0 if ok == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
