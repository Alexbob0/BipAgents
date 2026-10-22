// The web console: log in (a one-time link, or a code approved in the iPhone app), then the person's agents, their
// conversations and the inbox, as in the app. No build step: Preact + htm, vendored.
import { html, render, useState, useEffect, useRef, useCallback, useMemo } from "./vendor/preact-htm.js";
import { call, hermes, events, blobURL, HttpError } from "./api.js";
import { render as markdown, visible, media } from "./markdown.js";
import { attach, category, CATEGORIES } from "./bips.js";
import { T, lang } from "./text.js";

// ------------------------------------------------------------------------------------------------------------------
// Small helpers
// ------------------------------------------------------------------------------------------------------------------
const store = {
  get(key, fallback) { try { return JSON.parse(localStorage.getItem(key)) ?? fallback; } catch { return fallback; } },
  set(key, value) { try { localStorage.setItem(key, JSON.stringify(value)); } catch {} },
};

function deviceName() {
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : T.browser;
  const os = /Windows/.test(ua) ? "Windows" : /Mac OS X/.test(ua) ? "Mac" : /Android/.test(ua) ? "Android" : /Linux/.test(ua) ? "Linux" : /iPhone|iPad/.test(ua) ? "iOS" : "";
  return os ? `${browser} · ${os}` : browser;
}

const isBotChat = s => (s.title || "").toLowerCase() === "bot chat";
const isScheduled = s => (s.id || "").startsWith("cron_") || (s.source || "").toLowerCase() === "cron";
// Hermes gives times as ISO strings or epoch seconds.
const millis = v => typeof v === "number" ? v * (v < 1e12 ? 1000 : 1) : Date.parse(v) || 0;
const sessionTime = s => millis(s.updated_at || s.last_active || s.last_message_at || s.created_at || s.started_at || 0);
const newestFirst = list => [...list].sort((a, b) => sessionTime(b) - sessionTime(a));

function when(value) {
  const ms = millis(value);
  if (!ms) return "";
  const date = new Date(ms);
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  return sameDay ? date.toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString(lang, { day: "numeric", month: "short" }) + " " + date.toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" });
}

function errorText(error) {
  if (error instanceof HttpError) {
    if (error.status === 401 || error.status === 403) return T.sessionEnded;
    if (error.status === 502 || error.status === 504) return T.unreachable;
    return error.message;
  }
  return T.unreachable;
}

function messageText(m) {
  if (typeof m.content === "string") return m.content;
  if (Array.isArray(m.content)) return m.content.map(p => typeof p === "string" ? p : p.type === "text" ? p.text : "").filter(Boolean).join("\n");
  return m.text || "";
}

function hasImage(m) {
  return Array.isArray(m.content) && m.content.some(p => ["image_url", "input_image", "image"].includes(p?.type));
}

// The transcript as the app shows it: what the person wrote, the agent's replies, and the tools it used between them.
function thread(messages) {
  const out = [];
  let tools = [];
  for (const m of messages) {
    const role = (m.role || "").toLowerCase();
    if (role === "tool") continue;
    if (role === "assistant") {
      for (const call of m.tool_calls || []) tools.push(call.function?.name || call.name || "tool");
      const text = messageText(m);
      if (!visible(text) && !media(text).length) continue;
      out.push({ id: m.id ?? out.length, role, text, tools, at: m.created_at || m.timestamp });
      tools = [];
    } else if (role === "user") {
      if (tools.length) { out.push({ id: `t${out.length}`, role: "tools", tools }); tools = []; }
      out.push({ id: m.id ?? out.length, role, text: messageText(m), image: hasImage(m), at: m.created_at || m.timestamp });
    }
  }
  if (tools.length) out.push({ id: `t${out.length}`, role: "tools", tools });
  return out;
}

// Runs this browser started and may come back to (a reload, another conversation): session → run.
const activeRuns = {
  all() { return store.get("bip-active-runs", {}); },
  get(session) { return this.all()[session]; },
  set(session, run) { const all = this.all(); if (run) all[session] = run; else delete all[session]; store.set("bip-active-runs", all); },
};

// ------------------------------------------------------------------------------------------------------------------
// Bips
// ------------------------------------------------------------------------------------------------------------------
function Bip({ agent, mood = "idle", size = 48 }) {
  const ref = useRef(null);
  const handle = useRef(null);
  const c = agent.c || category(agent);
  useEffect(() => {
    handle.current = attach(ref.current, c, mood);
    return () => handle.current.detach();
  }, []);
  useEffect(() => { handle.current?.set(c, mood); }, [c, mood]);
  return html`<span class="bip" ref=${ref} style=${{ width: `${size}px`, height: `${size}px` }}></span>`;
}

// ------------------------------------------------------------------------------------------------------------------
// Logging in
// ------------------------------------------------------------------------------------------------------------------
function Login({ onIn, notice }) {
  const [pairing, setPairing] = useState(null);
  const [error, setError] = useState(notice);
  const [busy, setBusy] = useState(false);
  const stopped = useRef(false);

  useEffect(() => () => { stopped.current = true; }, []);

  async function showCode() {
    setError(null);
    setBusy(true);
    try {
      const p = await call("/console/api/pairing", { method: "POST" });
      setPairing(p);
      const until = Date.now() + 5 * 60 * 1000;
      while (!stopped.current && Date.now() < until) {
        await new Promise(r => setTimeout(r, 2000));
        const r = await call("/console/api/pairing/wait", { method: "POST", body: { code: p.code, wait: p.wait } });
        if (r.ok) { onIn(); return; }
      }
      if (!stopped.current) { setPairing(null); setError(T.codeExpired); }
    } catch (e) {
      setPairing(null);
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return html`<main class="login">
    <div class="login-bips">${CATEGORIES.slice(0, 5).map((c, i) => html`<${Bip} key=${c.id} agent=${{ c }} size=${64} mood=${i === 2 ? "content" : "idle"}/>`)}</div>
    <h1>${T.consoleTitle}</h1>
    <p class="lede">${T.loginLede}</p>
    <div class="login-ways">
      <section class="card">
        <h2>${T.byApp}</h2>
        ${pairing
          ? html`<p>${T.approveIn}</p><p class="code" aria-label=${T.code}>${pairing.code.slice(0, 3)} ${pairing.code.slice(3)}</p><p class="muted">${T.waiting}</p>`
          : html`<p>${T.byAppHelp}</p><button class="primary" onClick=${showCode} disabled=${busy}>${T.showCode}</button>`}
      </section>
      <section class="card">
        <h2>${T.byLink}</h2>
        <p>${T.byLinkHelp}</p>
        <pre class="command">bipagents console</pre>
      </section>
    </div>
    ${error && html`<p class="error">${error}</p>`}
  </main>`;
}

// ------------------------------------------------------------------------------------------------------------------
// The agent's sessions
// ------------------------------------------------------------------------------------------------------------------
function Sessions({ agent, current, onOpen, refreshKey }) {
  const [sessions, setSessions] = useState(null);
  const [error, setError] = useState(null);
  const [showTasks, setShowTasks] = useState(false);

  const load = useCallback(async () => {
    try {
      const all = [];
      for (let offset = 0, page = 0; page < 4; page++) {
        const r = await hermes(agent.id, `api/sessions?limit=50&offset=${offset}`);
        const list = Array.isArray(r) ? r : r.sessions || r.data || [];
        all.push(...list.filter(s => !all.some(a => a.id === s.id)));
        offset += list.length;
        if (list.length < 50 || all.filter(s => !isScheduled(s)).length >= 30) break;
      }
      setSessions(all);
      setError(null);
      return all;
    } catch (e) {
      setError(errorText(e));
      return null;
    }
  }, [agent.id]);

  useEffect(() => {
    setSessions(null);
    load().then(all => {
      if (all && !current) {
        const main = all.find(isBotChat) || newestFirst(all).find(s => !isScheduled(s));
        if (main) onOpen(main.id);
      }
    });
  }, [agent.id]);
  useEffect(() => { if (refreshKey) load(); }, [refreshKey]);

  // A new conversation exists once its first message goes (titled after it, as in the app).
  const create = () => onOpen("new");

  const row = s => html`<button key=${s.id} class=${`session ${s.id === current ? "on" : ""}`} onClick=${() => onOpen(s.id)}>
    <span class="session-title">${isBotChat(s) ? T.botChat : s.title || s.preview || T.untitled}</span>
    <span class="session-when">${when(sessionTime(s))}</span>
  </button>`;

  const bot = (sessions || []).filter(isBotChat);
  const talks = newestFirst((sessions || []).filter(s => !isBotChat(s) && !isScheduled(s)));
  const tasks = newestFirst((sessions || []).filter(isScheduled));
  return html`<nav class="sessions" aria-label=${T.conversations}>
    <div class="sessions-head">
      <${Bip} agent=${agent} size=${40}/>
      <strong>${agent.name}</strong>
      <button class="ghost" onClick=${create} title=${T.newConversation} aria-label=${T.newConversation}>＋</button>
    </div>
    ${error && html`<p class="error small">${error}</p>`}
    ${sessions === null && !error && html`<p class="muted small pad">${T.loading}</p>`}
    ${bot.map(row)}
    ${talks.length > 0 && html`<h3>${T.conversations}</h3>`}
    ${talks.map(row)}
    ${tasks.length > 0 && html`<button class="section-toggle" onClick=${() => setShowTasks(!showTasks)}>${showTasks ? "▾" : "▸"} ${T.tasks} (${tasks.length})</button>`}
    ${showTasks && tasks.map(row)}
  </nav>`;
}

// ------------------------------------------------------------------------------------------------------------------
// A conversation
// ------------------------------------------------------------------------------------------------------------------
let playing = null;   // one voice at a time
function stopPlaying() { if (playing) { playing.pause(); playing = null; } }

async function play(url, onEnd) {
  stopPlaying();
  const audio = new Audio(url);
  playing = audio;
  audio.onended = () => { if (playing === audio) playing = null; onEnd?.(); };
  await audio.play();
  return audio;
}

function Listen({ agent, text }) {
  const [state, setState] = useState("idle");
  async function listen() {
    if (state === "playing") { stopPlaying(); setState("idle"); return; }
    setState("loading");
    try {
      const url = await blobURL("/v1/tts/message", { method: "POST", body: { text: visible(text), agent: agent.id, use: "reading", format: "mp3" } });
      await play(url, () => setState("idle"));
      setState("playing");
    } catch { setState("error"); }
  }
  return html`<button class="tiny" onClick=${listen} title=${T.listen}>${state === "loading" ? "…" : state === "playing" ? "■ " + T.stop : state === "error" ? "⚠︎ " + T.listen : "▶︎ " + T.listen}</button>`;
}

function MediaFile({ path, agent }) {
  const [url, setUrl] = useState(null);
  const [failed, setFailed] = useState(false);
  const name = path.split("/").pop();
  const ext = (name.split(".").pop() || "").toLowerCase();
  const kind = ["mp3", "m4a", "wav", "ogg", "opus", "aac", "flac"].includes(ext) ? "audio" : ["png", "jpg", "jpeg", "gif", "webp"].includes(ext) ? "image" : ["mp4", "m4v", "mov", "webm"].includes(ext) ? "video" : "file";
  async function open() {
    const of = agent ? `&agent=${encodeURIComponent(agent)}` : "";  // a file in that agent's own places (/envoi…)
    try { setUrl(await blobURL(`/v1/media?path=${encodeURIComponent(path)}${of}`)); } catch { setFailed(true); }
  }
  useEffect(() => { if (kind === "image") open(); }, [path]);
  if (failed) return html`<p class="media muted">📎 ${name} · ${T.fileUnavailable}</p>`;
  if (!url) return html`<button class="media-chip" onClick=${open}>${kind === "audio" ? "🎧" : kind === "video" ? "🎬" : kind === "image" ? "🖼️" : "📎"} ${name}</button>`;
  if (kind === "audio") return html`<audio class="media" controls autoplay src=${url}></audio>`;
  if (kind === "video") return html`<video class="media" controls src=${url}></video>`;
  if (kind === "image") return html`<img class="media" src=${url} alt=${name}/>`;
  return html`<a class="media-chip" href=${url} download=${name}>📎 ${name} · ${T.download}</a>`;
}

function Approval({ request, onChoose }) {
  const choices = (request.choices || ["once", "deny"]).filter(c => c !== "always");
  const label = { once: T.allowOnce, session: T.allowSession, deny: T.deny };
  return html`<div class="card approval" role="alert">
    <h4>${T.approvalTitle}</h4>
    ${request.description && html`<p>${request.description}</p>`}
    ${request.command && html`<pre><code>${request.command}</code></pre>`}
    <div class="row">${choices.map(c => html`<button key=${c} class=${c === "deny" ? "" : "primary"} onClick=${() => onChoose(c)}>${label[c] || c}</button>`)}</div>
  </div>`;
}

function Clarify({ request, onAnswer }) {
  const questions = request.questions?.length ? request.questions : [{ id: "q1", question: request.question || request.text, choices: request.choices || request.options || [] }];
  const [answers, setAnswers] = useState({});
  const set = (id, v) => setAnswers({ ...answers, [id]: v });
  return html`<div class="card approval">
    <h4>${T.question}</h4>
    ${questions.map((q, i) => {
      const id = q.id || q.question_id || `q${i + 1}`;
      const choices = (q.choices || q.options || []).map(c => typeof c === "string" ? c : c.label);
      return html`<div key=${id} class="question"><p>${q.question || q.text || q.prompt}</p>
        <div class="row wrap">${choices.map(c => html`<button key=${c} class=${answers[id] === c ? "primary" : ""} onClick=${() => set(id, c)}>${c}</button>`)}</div>
        ${q.allow_other !== false && html`<input placeholder=${T.yourAnswer} value=${choices.includes(answers[id]) ? "" : answers[id] || ""} onInput=${e => set(id, e.target.value)}/>`}
      </div>`;
    })}
    <div class="row"><button class="primary" onClick=${() => onAnswer(answers, questions)}>${T.answer}</button><button onClick=${() => onAnswer(null, questions)}>${T.skip}</button></div>
  </div>`;
}

function Conversation({ agent, sessionId, onSessionChange, onMood }) {
  const [items, setItems] = useState(null);
  const [error, setError] = useState(null);
  const [draft, setDraft] = useState(() => store.get(`bip-draft-${agent.id}`, ""));
  const [live, setLive] = useState(null);       // the turn in progress: {sent, text, tools, runId}
  const [approval, setApproval] = useState(null);
  const [clarify, setClarify] = useState(null);
  const scroller = useRef(null);
  const following = useRef(null);
  const stick = useRef(true);
  const sending = useRef(false);

  const load = useCallback(async () => {
    if (!sessionId) { setItems([]); return; }
    try {
      const r = await hermes(agent.id, `api/sessions/${encodeURIComponent(sessionId)}/messages?include_compacted=false&inline_images=false`);
      setItems(thread(Array.isArray(r) ? r : r.messages || r.data || []));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [agent.id, sessionId]);

  // A run's events, through the bridge (it holds the one Hermes subscription and replays the run from its start).
  const follow = useCallback(async (runId, sent) => {
    following.current?.abort();
    const controller = new AbortController();
    following.current = controller;
    let text = "", tools = [], done = false;
    setLive({ sent, text, tools, runId });
    onMood("thinking");
    const update = () => setLive(l => l && l.runId === runId ? { ...l, text, tools: [...tools] } : l);
    try {
      await events(`/v1/runs/${encodeURIComponent(runId)}/events?agent=${encodeURIComponent(agent.id)}${sessionId ? `&session_id=${encodeURIComponent(sessionId)}` : ""}`, ({ type, data }) => {
        switch (type) {
          case "assistant.delta": case "message.delta": case "response.output_text.delta": {
            const d = data.delta ?? data.text ?? data.content ?? "";
            text += typeof d === "string" ? d : d.content || d.text || "";
            onMood("speaking");
            update();
            break;
          }
          case "assistant.commentary": case "message.interim":
            if (!data.already_streamed && data.text) { text += (text ? "\n\n" : "") + data.text + "\n\n"; update(); }
            break;
          case "tool.started": case "tool.start":
            tools.push(data.tool || data.name || data.tool_name || "tool");
            onMood("thinking");
            update();
            break;
          case "approval.request": case "approval.required":
            setApproval({ ...data, runId });
            onMood("asking");
            break;
          case "approval.responded": case "approval.resolved":
            setApproval(null);
            break;
          case "clarify.request": case "clarify.required":
            setClarify({ ...data, runId });
            onMood("asking");
            break;
          case "clarify.responded": case "clarify.resolved": case "clarify.cancelled": case "clarify.canceled": case "clarify.expired":
            setClarify(null);
            break;
          case "run.completed": case "run.failed": case "run.cancelled": case "run.canceled": case "run.interrupted":
            done = true;
            if (type === "run.failed" && data.error) setError(typeof data.error === "string" ? data.error : data.error.message || T.failed);
            break;
        }
      }, controller.signal);
    } catch (e) {
      if (e.name === "AbortError") return;
      if (e instanceof HttpError && e.status === 404) done = true;   // the run is over and gone
      else setError(errorText(e));
    }
    if (following.current !== controller) return;
    following.current = null;
    if (done && sessionId) activeRuns.set(sessionId, null);
    setApproval(null);
    setClarify(null);
    await load();
    setLive(null);
    onMood("idle");
    onSessionChange();
  }, [agent.id, sessionId, load]);

  useEffect(() => {
    setItems(null);
    setLive(null);
    setApproval(null);
    setClarify(null);
    setError(null);
    stick.current = true;
    load();
    const pending = store.get("bip-pending-follow", null);
    const run = sessionId && activeRuns.get(sessionId);
    if (pending && pending.session === sessionId) { store.set("bip-pending-follow", null); follow(pending.runId, pending.text); }
    else if (run) follow(run, null);
    else if (sessionId) {   // a run the app started that waits for an approval
      call(`/v1/approvals?agent=${encodeURIComponent(agent.id)}`).then(r => {
        const pending = (r.items || []).find(a => a.session_id === sessionId);
        if (pending && !following.current) follow(pending.run_id, null);
      }).catch(() => {});
    }
    return () => { following.current?.abort(); following.current = null; onMood("idle"); };
  }, [agent.id, sessionId]);

  // Turns the agent takes on its own (a routine, a reply that came while away) show up without reloading.
  useEffect(() => {
    if (!sessionId) return;
    let count = null;
    const timer = setInterval(async () => {
      if (following.current || document.hidden) return;
      try {
        const s = await call(`/v1/sessions/${encodeURIComponent(sessionId)}/state?agent=${encodeURIComponent(agent.id)}`);
        if (count !== null && s.message_count !== count) load();
        count = s.message_count;
      } catch {}
    }, 8000);
    return () => clearInterval(timer);
  }, [agent.id, sessionId]);

  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items, live, approval, clarify]);

  useEffect(() => { store.set(`bip-draft-${agent.id}`, draft); }, [draft]);

  async function send() {
    const text = draft.trim();
    if (!text || following.current || sending.current) return;
    sending.current = true;
    setDraft("");
    setError(null);
    stick.current = true;
    try {
      let session = sessionId;
      if (!session) {
        const s = await hermes(agent.id, "api/sessions", { method: "POST", body: { title: text.slice(0, 60) } });
        session = (s.session || s).id;
      }
      const run = await hermes(agent.id, "v1/runs", { method: "POST", body: { input: text, session_id: session }, headers: { "Idempotency-Key": crypto.randomUUID() } });
      const runId = run.run_id || run.id;
      activeRuns.set(session, runId);
      if (session === sessionId) follow(runId, text);
      else {   // a new conversation: it opens and follows the run from there
        store.set("bip-pending-follow", { session, runId, text });
        onSessionChange(session);
      }
    } catch (e) {
      setDraft(text);
      setError(errorText(e));
    } finally {
      sending.current = false;
    }
  }

  async function stop() {
    if (!live?.runId) return;
    try { await hermes(agent.id, `v1/runs/${encodeURIComponent(live.runId)}/stop`, { method: "POST", body: {} }); } catch (e) { setError(errorText(e)); }
  }

  async function choose(choice) {
    const request = approval;
    setApproval(null);
    try {
      await call("/v1/approve", { method: "POST", body: { agent: agent.id, run_id: request.runId, choice, request_id: request.request_id || request.approval_id || null } });
      onMood("thinking");
    } catch (e) { setApproval(request); setError(errorText(e)); }
  }

  async function answer(answers, questions) {
    const request = clarify;
    setClarify(null);
    const path = `v1/runs/${encodeURIComponent(request.runId)}/clarify`;
    try {
      if (request.request_id) {
        await hermes(agent.id, path, { method: "POST", body: answers ? { request_id: request.request_id, answers } : { request_id: request.request_id, cancel: true } });
      } else {
        const q = questions[0], id = q.id || q.question_id || "q1";
        let response = answers ? answers[id] ?? Object.values(answers)[0] ?? "" : "";
        if (q.multi_select && response) response = JSON.stringify([response]);
        await hermes(agent.id, path, { method: "POST", body: { clarify_id: request.clarify_id || request.id, response } });
      }
    } catch (e) { setClarify(request); setError(errorText(e)); }
  }

  function onKey(e) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); }
  }

  const onScroll = e => { const el = e.currentTarget; stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80; };

  const bubble = it => {
    if (it.role === "tools") return html`<p key=${it.id} class="tools">🔧 ${[...new Set(it.tools)].join(" · ")}</p>`;
    if (it.role === "user") return html`<div key=${it.id} class="msg user"><div class="bubble">${it.image && html`<span class="muted">🖼️ </span>`}${it.text}</div></div>`;
    return html`<div key=${it.id} class="msg agent">
      ${it.tools?.length > 0 && html`<p class="tools">🔧 ${[...new Set(it.tools)].join(" · ")}</p>`}
      <div class="bubble md">${markdown(it.text)}</div>
      ${media(it.text).map(p => html`<${MediaFile} key=${p} path=${p} agent=${agent.id}/>`)}
      <div class="meta">${it.at && html`<span>${when(it.at)}</span>`}${visible(it.text) && html`<${Listen} agent=${agent} text=${it.text}/>`}</div>
    </div>`;
  };

  return html`<section class="conversation">
    <div class="thread" ref=${scroller} onScroll=${onScroll}>
      ${items === null && !error && html`<p class="muted pad">${T.loading}</p>`}
      ${items && items.length === 0 && !live && html`<div class="empty"><${Bip} agent=${agent} size=${96} mood="content"/><p>${T.emptyThread(agent.name)}</p></div>`}
      ${items && items.map(bubble)}
      ${live && live.sent && html`<div class="msg user"><div class="bubble">${live.sent}</div></div>`}
      ${live && html`<div class="msg agent">
        ${live.tools.length > 0 && html`<p class="tools">🔧 ${[...new Set(live.tools)].join(" · ")}</p>`}
        ${live.text ? html`<div class="bubble md">${markdown(live.text)}</div>` : html`<div class="bubble typing"><span></span><span></span><span></span></div>`}
      </div>`}
      ${approval && html`<${Approval} request=${approval} onChoose=${choose}/>`}
      ${clarify && html`<${Clarify} request=${clarify} onAnswer=${answer}/>`}
    </div>
    ${error && html`<p class="error bar">${error}</p>`}
    <div class="composer">
      <textarea rows="1" placeholder=${T.message(agent.name)} value=${draft} onInput=${e => setDraft(e.target.value)} onKeyDown=${onKey}></textarea>
      ${live ? html`<button class="stop" onClick=${stop} title=${T.stop}>■</button>`
             : html`<button class="send primary" onClick=${send} disabled=${!draft.trim()} title=${T.send}>↑</button>`}
    </div>
  </section>`;
}

// ------------------------------------------------------------------------------------------------------------------
// Inbox: what the agents sent on their own (routines, reports), newest first
// ------------------------------------------------------------------------------------------------------------------
function Inbox({ agents, onOpen }) {
  const [items, setItems] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    call("/v1/outbox?limit=100").then(r => setItems((r.items || []).slice().reverse())).catch(e => setError(errorText(e)));
  }, []);
  const byId = Object.fromEntries(agents.map(a => [a.id, a]));
  return html`<section class="inbox">
    <h1>${T.inbox}</h1>
    ${error && html`<p class="error">${error}</p>`}
    ${items === null && !error && html`<p class="muted">${T.loading}</p>`}
    ${items && items.length === 0 && html`<p class="muted">${T.inboxEmpty}</p>`}
    ${items && items.map(it => {
      const agent = byId[it.agent] || { id: it.agent, name: it.agent };
      return html`<article key=${it.id} class="card inbox-item">
        <header><${Bip} agent=${agent} size=${36}/><div><strong>${it.title || agent.name}</strong><span class="muted small"> · ${it.title ? `${agent.name} · ` : ""}${when(it.sent_at || it.created_at)}</span></div></header>
        <div class="md">${markdown(it.text)}</div>
        ${media(it.text).map(p => html`<${MediaFile} key=${p} path=${p} agent=${agent.id}/>`)}
        <div class="meta">
          <${InboxAudio} item=${it}/>
          ${it.session_id && byId[it.agent] && html`<button class="tiny" onClick=${() => onOpen(it.agent, it.session_id)}>${T.openConversation}</button>`}
        </div>
      </article>`;
    })}
  </section>`;
}

function InboxAudio({ item }) {
  const [state, setState] = useState("idle");
  async function listen() {
    if (state === "playing") { stopPlaying(); setState("idle"); return; }
    setState("loading");
    try { await play(await blobURL(`/v1/outbox/${encodeURIComponent(item.id)}/audio`), () => setState("idle")); setState("playing"); }
    catch { setState("error"); }
  }
  return html`<button class="tiny" onClick=${listen}>${state === "loading" ? "…" : state === "playing" ? "■ " + T.stop : state === "error" ? "⚠︎ " + T.listen : "▶︎ " + T.listen}</button>`;
}

// ------------------------------------------------------------------------------------------------------------------
// Home: the Bips
// ------------------------------------------------------------------------------------------------------------------
function Home({ agents, onPick, moods }) {
  return html`<section class="home">
    <h1>${T.hello}</h1>
    <div class="grid">${agents.map(a => html`<button key=${a.id} class="agent-card" style=${{ "--main": a.c.main, "--deep": a.c.deep, "--tint": a.c.tint }} onClick=${() => onPick(a.id)}>
      <${Bip} agent=${a} size=${120} mood=${moods[a.id] || "idle"}/>
      <strong>${a.name}</strong>
      <span class="chip">${T.cats[a.c.id] || a.c.label}</span>
    </button>`)}</div>
  </section>`;
}

// ------------------------------------------------------------------------------------------------------------------
// The console
// ------------------------------------------------------------------------------------------------------------------
function readRoute() {
  const [view = "home", agent = null, session = null] = location.hash.replace(/^#\/?/, "").split("/").map(decodeURIComponent);
  return { view, agent, session };
}

function go(view, agent, session) {
  const hash = "#/" + [view, agent, session].filter(Boolean).map(encodeURIComponent).join("/");
  if (location.hash !== hash) history.pushState(null, "", hash);
  window.dispatchEvent(new Event("bip-route"));
}

function Console({ onOut }) {
  const [agents, setAgents] = useState(null);
  const [error, setError] = useState(null);
  const [route, setRoute] = useState(readRoute);
  const [moods, setMoods] = useState({});
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    call("/v1/agents").then(r => setAgents((r.agents || []).map(a => ({ ...a, c: category(a) }))))
      .catch(e => { if (e.status === 401) onOut(); else setError(errorText(e)); });
    const update = () => setRoute(readRoute());
    window.addEventListener("popstate", update);
    window.addEventListener("bip-route", update);
    return () => { window.removeEventListener("popstate", update); window.removeEventListener("bip-route", update); };
  }, []);

  async function logout() {
    await call("/console/api/logout", { method: "POST" }).catch(() => {});
    onOut();
  }

  if (error) return html`<main class="pad"><p class="error">${error}</p></main>`;
  if (!agents) return html`<main class="pad muted">${T.loading}</main>`;

  const agent = route.view === "agent" && agents.find(a => a.id === route.agent);
  const setMood = id => mood => setMoods(m => m[id] === mood ? m : { ...m, [id]: mood });
  return html`<div class=${`shell ${agent ? "with-sessions" : ""}`}>
    <aside class="rail">
      <button class=${`brand ${route.view === "home" ? "on" : ""}`} onClick=${() => go("home")}>BipAgents</button>
      ${agents.map(a => html`<button key=${a.id} class=${`rail-agent ${agent && agent.id === a.id ? "on" : ""}`} onClick=${() => go("agent", a.id)}>
        <${Bip} agent=${a} size=${34} mood=${moods[a.id] || "idle"}/><span>${a.name}</span>
      </button>`)}
      <div class="rail-foot">
        <button class=${`rail-link ${route.view === "inbox" ? "on" : ""}`} onClick=${() => go("inbox")}>📥 ${T.inbox}</button>
        <button class="rail-link" onClick=${logout}>${T.logout}</button>
      </div>
    </aside>
    ${route.view === "inbox" ? html`<main class="main"><${Inbox} agents=${agents} onOpen=${(a, s) => go("agent", a, s)}/></main>`
      : agent ? html`
        <${Sessions} key=${agent.id} agent=${agent} current=${route.session} refreshKey=${refresh} onOpen=${s => go("agent", agent.id, s)}/>
        <main class="main"><${Conversation} key=${agent.id + (route.session || "")} agent=${agent} sessionId=${route.session === "new" ? null : route.session}
          onMood=${setMood(agent.id)}
          onSessionChange=${s => { setRefresh(r => r + 1); if (s) go("agent", agent.id, s); }}/></main>`
      : html`<main class="main"><${Home} agents=${agents} moods=${moods} onPick=${id => go("agent", id)}/></main>`}
  </div>`;
}

function App() {
  const [state, setState] = useState("checking");
  const check = () => call("/console/api/me").then(r => setState(r.logged_in ? "in" : "out")).catch(() => setState("out"));

  useEffect(() => {
    // A one-time link: its code is in the address' # (never sent to a server log), used once, then wiped.
    const useLink = () => {
      const code = location.hash.match(/^#([A-Za-z0-9_-]{20,})$/)?.[1];
      if (!code) return false;
      history.replaceState(null, "", location.pathname);
      setState("checking");
      call("/console/api/login", { method: "POST", body: { code, name: deviceName() } })
        .then(() => setState("in"))
        .catch(() => setState("link-used"));
      return true;
    };
    if (!useLink()) check();
    window.addEventListener("hashchange", useLink);
    return () => window.removeEventListener("hashchange", useLink);
  }, []);

  if (state === "checking") return html`<main class="pad muted">${T.loading}</main>`;
  if (state === "out" || state === "link-used") return html`<${Login} notice=${state === "link-used" ? T.linkUsed : null} onIn=${() => setState("in")}/>`;
  return html`<${Console} onOut=${() => setState("out")}/>`;
}

document.documentElement.lang = lang;
document.title = T.consoleTitle;
render(html`<${App}/>`, document.getElementById("app"));
