// bipagents.com: the hero's results and its « See more » on privacy, the agents' cards, the comparison, the
// two ways and the plan picker; on their own pages, the agent at work (the app's own demo scenarios, with its map and its pages) and the
// install. One script for every page: each part runs where its elements are. No framework, no network but this site.
import { Bip, CATEGORIES, REDUCED, staticBip } from "./bips.js";

const data = JSON.parse(document.getElementById("site-data").textContent);
const $ = (s, root = document) => root.querySelector(s);
const $$ = (s, root = document) => [...root.querySelectorAll(s)];
const wait = ms => new Promise(r => setTimeout(r, REDUCED ? Math.min(ms, 120) : ms));
const TONE = { daily: "coral", wellness: "green", finance: "gold", home: "teal", work: "blue", learning: "violet", creative: "pink", tech: "blue" };
const esc = s => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const once = (el, fn, threshold = 0.3) => new IntersectionObserver((entries, io) => {
  if (entries[0].isIntersecting) { io.disconnect(); fn(); }
}, { threshold }).observe(el);

// -- Bips everywhere -----------------------------------------------------------------------------------------------
const bips = new Map();
for (const el of $$("[data-bip]")) {
  bips.set(el, new Bip(el, el.dataset.bip, { interactive: el.hasAttribute("data-interactive"), lines: data.lines }));
}
for (const el of $$("[data-bip-static]")) el.innerHTML = staticBip(el.dataset.bipStatic);

// -- Header, progress, reveals, parallax ---------------------------------------------------------------------------
const top = $(".top"), bar = $(".progress i");
const parallax = REDUCED ? [] : $$("[data-parallax]");
let ticking = false;
function onScroll() {
  ticking = false;
  const y = scrollY, max = document.documentElement.scrollHeight - innerHeight;
  top.classList.toggle("scrolled", y > 8);
  bar.style.transform = `scaleX(${max > 0 ? y / max : 0})`;
  for (const el of parallax) el.style.transform = `translateY(${(y * +el.dataset.parallax).toFixed(1)}px)`;
}
addEventListener("scroll", () => { if (!ticking) { ticking = true; requestAnimationFrame(onScroll); } }, { passive: true });
onScroll();
const revealer = new IntersectionObserver(entries => {
  for (const entry of entries) if (entry.isIntersecting) { entry.target.classList.add("in"); revealer.unobserve(entry.target); }
}, { threshold: 0.12 });
$$(".reveal").forEach(el => revealer.observe(el));

// -- An agent at work: the app's demo scenarios, played in a chat --------------------------------------------------
const chat = $(".chat"), body = $(".chat-body"), wrap = $(".chat-wrap");
let run = 0;
// Tool rows in plain words, as the apps word them (ToolLabel.swift / ToolLabel.kt; the wording comes from
// scripts/tool-labels.py): never a tool's own name. The working line under the thread says what the agent does now.
const TOOLS = data.tools ?? { list: [], words: {}, extra: {} };
const ICONS = { browser: "🌐", webSearch: "🔎", webRead: "📄", calendarRead: "📅", calendarChange: "📅", memory: "🧠",
                weather: "⛅", health: "❤️", home: "🏠", todo: "✅", terminal: "⌨️", compute: "🧮", mailSearch: "✉️",
                mailRead: "✉️", mailDraft: "✉️", readFile: "📄", prepareFile: "📝", messageAgent: "💬", schedule: "⏰",
                createPage: "🧩", route: "🗺️", remindersAdd: "⏰", remindersRead: "⏰", notebookSearch: "📒",
                notebookRead: "📒", notebookNote: "📒" };
const ANY_KEYS = ["query", "words", "page", "title", "name", "subject", "path", "file", "url", "urls", "target", "agent",
                  "to", "text", "message", "prompt", "goal", "question", "command", "place", "destination"];

// `mcp__<server>__<tool>` (earlier `mcp_<server>_<tool>`) → the tool's own name.
function plainTool(tool) {
  if (tool.startsWith("mcp__")) {
    const rest = tool.slice(5), cut = rest.indexOf("__");
    return cut < 0 ? [null, rest] : [rest.slice(0, cut), rest.slice(cut + 2)];
  }
  for (const server of ["bipagents_files", "bipagents_pay", "bipagents", "stealth"]) {
    if (tool.startsWith(`mcp_${server}_`)) return [server, tool.slice(server.length + 5)];
  }
  return [null, tool];
}

// Hermes' `tool_call` wraps the real call: {"calls": [{"name", "arguments"}]}, {"name", "arguments"}, or a label
// (« Bipagents · notebook note +2 »). The first call stands for the row.
function unwrapCall(tool, args) {
  if (tool !== "tool_call" || !args) return [tool, args];
  try {
    const json = JSON.parse(args);
    let calls = json?.calls;
    if (typeof calls === "string") calls = JSON.parse(calls);
    if (!calls && json?.name) calls = json;
    const first = [].concat(calls ?? []).find(c => c && typeof c.name === "string" && c.name.trim());
    if (first) return [first.name.trim(), first.arguments == null ? null : typeof first.arguments === "string" ? first.arguments : JSON.stringify(first.arguments)];
  } catch {
    if (!/^[{[]/.test(args.trim())) {
      const label = args.trim().replace(/\s+\+\d+$/, "").split(" · ").pop();
      return [label.toLowerCase().split(/[ -]+/).join("_"), null];
    }
  }
  return [tool, null];
}

function kindOf(name, server) {
  name = name.toLowerCase();
  const row = TOOLS.list.find(k => k.names.includes(name)) ?? TOOLS.list.find(k => k.prefixes.some(p => name.startsWith(p)));
  if (row) return row;
  return server === "stealth" || name.includes("browser") ? TOOLS.list.find(k => k.id === "browser") : { id: "unknown", keys: ANY_KEYS };
}

// One short line: a path's file name, a link without « https://www. », an agent without its place.
function shown(value, key) {
  let text = String(value).split(/\r?\n/)[0].trim();
  if (["path", "file", "file_path", "files", "text_name", "target", "agent"].includes(key)) text = text.split("/").filter(Boolean).pop() ?? text;
  if (key === "url" || key === "urls") text = text.replace(/^[a-z]+:\/\//, "").replace(/^www\./, "").replace(/\/$/, "");
  text = text.replace(/\s+/g, " ");
  if (!text) return null;
  return text.length > 80 ? text.slice(0, 79) + "…" : text;
}

// The host of a call's page (WorkingStep.site): its url, any web address, or a bare host.
function siteIn(args) {
  const text = (args ?? "").trim();
  for (const re of [/"urls?"\s*:\s*\[?\s*"(?:[a-zA-Z][a-zA-Z0-9+.-]*:\/\/)?([^/"?#\s:]+)/, /https?:\/\/([^/"?#\s:]+)/,
                    /^(?:www\.)?([a-zA-Z0-9-]+(?:\.[a-zA-Z0-9-]+)+)(?:[/?#]\S*)?$/]) {
    const m = text.match(re);
    if (!m) continue;
    const host = m[1].toLowerCase().replace(/^www\./, "");
    if (host.includes(".") && !host.endsWith(".") && /[a-z]/.test(host)) return host;
  }
  return null;
}

// A call → { kind, detail, title, site }: the row's first line in words, what it works on.
function toolLabel(rawTool, rawArgs) {
  const [tool, args] = unwrapCall(rawTool, rawArgs);
  const [server, name] = plainTool(tool);
  const row = kindOf(name, server), kind = row.id;
  const text = (args ?? "").trim();
  let detail = null;
  if (text && kind !== "compute") {
    let object = null;
    if (text.startsWith("{")) { try { object = JSON.parse(text); } catch { object = null; } }
    if (object && typeof object === "object") {
      for (const key of row.keys) {
        const v = object[key], value = Array.isArray(v) ? v.find(x => typeof x === "string" && x.trim()) : v;
        if (typeof value === "string" && (detail = shown(value, key))) break;
      }
    } else if (!text.startsWith("{") && !text.startsWith("[")) {
      detail = shown(text, "");
    }
  }
  const words = TOOLS.words[kind] ?? {};
  return { kind, detail, title: words.one ?? TOOLS.words.tool?.one ?? "", site: siteIn(args) };
}

// The finished rows of a turn, as the app folds them (ToolLabel.folds): three calls or more of one kind in a row make
// one row, « Utilise le navigateur (3 étapes) », counting the pages or files for the kinds that count them.
function foldRows(labels) {
  const rows = [];
  for (let start = 0; start < labels.length;) {
    let end = start + 1;
    while (end < labels.length && labels[end].kind === labels[start].kind) end++;
    const run = labels.slice(start, end);
    if (run.length < 3) rows.push(...run);
    else {
      const first = run[0], words = TOOLS.words[first.kind] ?? {};
      const details = [...new Set(run.map(l => l.detail).filter(Boolean))];
      const counts = TOOLS.list.find(k => k.id === first.kind)?.counts && details.length > 1;
      const n = counts ? details.length : run.length;
      const title = words.many && (counts || !TOOLS.list.find(k => k.id === first.kind)?.counts)
        ? words.many.replace("{n}", n) : TOOLS.extra.toolrow_times.replace("{}", first.title).replace("{n}", run.length);
      rows.push({ kind: first.kind, title, detail: details.at(-1) ?? null });
    }
    start = end;
  }
  return rows;
}

// The working line's words for the call running now (null: the agent thinks).
function workingPhrase(label, site) {
  const extra = TOOLS.extra;
  if (!label) return extra.toolrow_now_thinking;
  if (label.kind === "browser" && site) return extra.toolrow_now_browsing.replace("{}", site);
  if (label.kind === "webRead" && site) return extra.toolrow_now_reading.replace("{}", site);
  if (label.kind === "messageAgent" && label.detail) return extra.toolrow_now_writing_to.replace("{}", label.detail);
  return (TOOLS.words[label.kind] ?? TOOLS.words.unknown).now;
}

const md = s => esc(s).replace(/\*\*(.+?)\*\*/g, "<b>$1</b>").replace(/\n\n/g, "<br><br>").replace(/\n/g, "<br>");

// The working line stays under the thread while the agent works (WorkingLine in the apps).
let workingLine = null;
function add(html, cls, tag = "div") {
  const el = document.createElement(tag);
  el.className = cls;
  el.innerHTML = html;
  if (workingLine?.isConnected) body.insertBefore(el, workingLine); else body.append(el);
  while (body.scrollHeight > body.clientHeight + 2 && body.children.length > 1) body.firstElementChild.remove();
  return el;
}
const avatar = category => `<span class="av">${staticBip(category)}</span>`;

// A page the agent made, as the app shows it under the reply: the map of the walk, or the page kit's real page.
function pageCard(name) {
  const page = data.pages[name];
  if (name === "walk") {
    const pins = page.stops.map((s, i) => `<button type="button" class="pin" style="left:${s.x}%;top:${s.y}%;--c:${s.color}" aria-label="${esc(s.name)}" data-i="${i}"></button>`).join("");
    const list = page.stops.map((s, i) => `<li data-i="${i}" style="--c:${s.color}"><i>${i + 1}</i>${esc(s.name)}</li>`).join("");
    const el = add(`<p class="pc-title">🌳 ${esc(page.title)}</p><p class="pc-sub">${esc(page.subtitle)}</p>`
      + `<div class="map"><img src="${page.src}" alt="${esc(page.alt)}" width="600" height="400" loading="lazy">${pins}</div>`
      + `<ol class="stops">${list}</ol><p class="pc-tip">${esc(page.tip)}</p><p class="pc-src">${esc(page.source)}</p>`, "page-card map-card");
    const pick = i => {
      $$(".pin", el).forEach(p => p.classList.toggle("on", +p.dataset.i === i));
      $$(".stops li", el).forEach(p => p.classList.toggle("on", +p.dataset.i === i));
    };
    $$(".pin, .stops li", el).forEach(p => p.addEventListener("click", () => pick(+p.dataset.i)));
    return el;
  }
  return add(`<div class="frame"><iframe src="${page.src}" title="${esc(page.title)}" loading="lazy"></iframe></div>`
    + `<a class="pc-open" href="${page.src}" target="_blank" rel="noopener">${esc(page.title)} · ${esc(data.labels.open)} ↗</a>`, "page-card kit-card");
}

async function play(id) {
  const mine = ++run;
  const scenario = data.scenarios[id], category = scenario.category, c = CATEGORIES[category];
  for (const el of [chat, wrap]) el.style.setProperty("--chat-tint", `var(--${TONE[category]}-tint)`);
  chat.style.setProperty("--chat-main", c.main);
  chat.style.setProperty("--chat-deep", `var(--${TONE[category]}-deep)`);   // the text-safe shade (site.css)
  $(".chat-avatar").innerHTML = staticBip(category);
  $(".chat-name").textContent = scenario.agent;
  body.innerHTML = "";
  const alive = () => mine === run;
  await wait(500); if (!alive()) return;
  add(esc(scenario.prompt), "msg you");
  let answer = "", madePage = null, work = null, site = null;
  const fill = s => s.replaceAll("{answer}", answer);
  workingLine = null;
  // « Je réfléchis… », « Je navigue sur pastamia.fr… »: one line, a light sweeping across it.
  function working(phrase) {
    if (!alive()) return;
    if (!workingLine?.isConnected) {
      workingLine = add(`<span class="av">${staticBip(category)}</span><span class="ph"></span>`, "working");
      workingLine.setAttribute("role", "status");
    }
    $(".ph", workingLine).textContent = phrase;
  }
  function rest() { if (alive()) { workingLine?.remove(); workingLine = null; } }

  async function say(text) {
    working(workingPhrase(null));
    await wait(1000); rest();
    work = null;   // the next steps fold under the next reply
    if (!alive()) return;
    add(`${avatar(category)}<span class="txt">${md(text)}</span>`, "msg bot");
    await wait(1100);
  }

  async function steps(list) {
    for (const step of list) {
      if (!alive()) return false;
      if (step.think) {
        working(workingPhrase(null));
        await wait(1100);
        add(esc(fill(step.think)), "think");
        await wait(1300);
      } else if (step.tool || step.page) {
        // In plain words as the app shows it: what it does now on the working line, then a row folded under
        // « Détail du travail ».
        const label = toolLabel(step.tool ?? "create_page", fill(step.arguments ?? step.preview ?? ""));
        if (label.kind === "browser" || label.kind === "webRead") site = label.site ?? (label.kind === "browser" ? site : null);
        working(workingPhrase(label, site));
        await wait(Math.max(700, (step.seconds ?? 1) * 1000));
        if (!alive()) return false;
        if (!work?.isConnected) {
          work = add(`<summary>${esc(data.labels.work)}</summary><div class="work-rows"></div>`, "work", "details");
          work.labels = [];
        }
        work.labels.push(label);
        $(".work-rows", work).innerHTML = foldRows(work.labels).map(row =>
          `<div class="tool done"><span class="ic">${ICONS[row.kind] ?? "⚙️"}</span><span><b>${esc(row.title)}</b>`
          + `${row.detail ? `<span>${esc(row.detail)}</span>` : ""}</span><i class="st"></i></div>`).join("");
        if (step.page) madePage = step.page;
        await wait(250);
      } else if (step.ask) {
        rest(); work = null;   // the steps after a question fold under it
        const card = add(`<p>${esc(step.ask.question)}</p><div class="opts">${step.ask.choices.map(o => `<span class="opt">${esc(o)}</span>`).join("")}</div>`, "card-q");
        await wait(1700); if (!alive()) return false;
        const choice = $$(".opt", card)[Math.min(1, step.ask.choices.length - 1)];
        choice.classList.add("chosen");
        answer = choice.textContent.replace(/ · ★.*$/, "");
        await wait(900);
        if (step.then && !(await steps(step.then))) return false;
      } else if (step.approve) {
        rest(); work = null;   // the steps after a question fold under it
        const card = add(`<p>${esc(fill(step.approve.description))}</p><code>${esc(fill(step.approve.command))}</code>`
          + `<div class="btns"><span>${esc(data.labels.once)}</span><span>${esc(data.labels.session)}</span><span>${esc(data.labels.deny)}</span></div>`, "card-a");
        await wait(1900); if (!alive()) return false;
        $(".btns span", card).classList.add("chosen");
        await wait(800);
        if (step.then && !(await steps(step.then))) return false;
      } else if (step.say) {
        // « MEDIA:/demo/page/… » in a reply is where the app shows the page the agent made.
        const [before, after] = fill(step.say).split(/\n*MEDIA:\S+\n*/);
        await say(before);
        if (after !== undefined && alive()) {
          if (madePage && data.pages[madePage]) { pageCard(madePage); await wait(2200); }
          if (after.trim()) await say(after);
        }
      } else if (step.pause) {
        await wait(step.pause * 1000);
      }
    }
    return true;
  }
  await steps(scenario.steps);
  if (alive()) rest();
}

let current = "book_table";
const tabs = $$(".tab");
tabs.forEach((tab, i) => {
  tab.addEventListener("click", () => {
    tabs.forEach(t => { t.classList.toggle("active", t === tab); t.setAttribute("aria-selected", String(t === tab)); t.tabIndex = t === tab ? 0 : -1; });
    current = tab.dataset.scenario;
    play(current);
  });
  tab.tabIndex = i ? -1 : 0;
  tab.addEventListener("keydown", e => {   // arrows move between the tabs (WAI-ARIA tabs)
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const next = tabs[(i + step + tabs.length) % tabs.length];
    next.focus(); next.click();
  });
});
$(".replay")?.addEventListener("click", () => play(current));
if (chat) once(chat, () => play(current), 0.35);   // the demo has its own page (/demo/)

// -- Open in place: a button that shows or hides the part it controls (aria-expanded), opened as well by its address
// (#private, #install) on arrival and from any link to it on the page. Its label may change (« See more » / « See less »).
function disclosure(button, { hash, onOpen } = {}) {
  const panel = document.getElementById(button.getAttribute("aria-controls"));
  if (!panel) return null;
  const label = $(".priv-label", button) || null;
  function set(open, { scroll = false, instant = false } = {}) {
    button.setAttribute("aria-expanded", String(open));
    panel.hidden = !open;
    if (label && button.dataset.more) label.textContent = open ? button.dataset.less : button.dataset.more;
    if (open) {
      $$(".reveal", panel).forEach(el => el.classList.add("in"));
      if (onOpen) onOpen(panel);
      if (scroll) requestAnimationFrame(() => panel.scrollIntoView({ behavior: REDUCED || instant ? "auto" : "smooth", block: "start" }));
    }
  }
  button.addEventListener("click", () => set(button.getAttribute("aria-expanded") !== "true"));
  if (hash) {
    const fromHash = (h, instant) => { if (h === hash) set(true, { scroll: true, instant }); };
    // on arrival, once the page is laid out (the browser could not scroll to a part that was still hidden)
    if (document.readyState === "complete") fromHash(location.hash, true);
    else addEventListener("load", () => fromHash(location.hash, true), { once: true });
    addEventListener("hashchange", () => fromHash(location.hash));
    document.addEventListener("click", e => {
      const a = e.target.closest(`a[href$="${hash}"]`);
      if (!a || a.pathname !== location.pathname) return;
      e.preventDefault();
      set(true, { scroll: true });
      try { history.replaceState(null, "", hash); } catch (err) { /* file:// */ }
    });
  }
  return { set };
}

// -- Hero: « See more », where your data is and who can read it, under the first screen ------------------------------
const privMore = $(".priv-more");
if (privMore) disclosure(privMore, { hash: "#private" });

// -- What they do: a grid on a computer, a row to swipe on a phone (focusable only when it scrolls) ----------------
const uses = $(".uses");
if (uses) {
  const focusable = () => {
    const scrolls = uses.scrollWidth > uses.clientWidth + 2;
    if (scrolls) uses.setAttribute("tabindex", "0"); else uses.removeAttribute("tabindex");
  };
  focusable();
  addEventListener("resize", focusable, { passive: true });
}

// -- Compared: one product face to face with BipAgents (Grok Bot first, the closest), or all of them. Without the
// script the whole table shows and the control stays hidden. ---------------------------------------------------------
(() => {
  const vs = $(".vs"), pick = vs && $(".vs-pick", vs);
  if (!pick) return;
  const buttons = $$("button[data-pick]", pick);
  const show = (which, animate) => {
    vs.dataset.pick = which;
    for (const b of buttons) b.setAttribute("aria-pressed", String(b.dataset.pick === which));
    // a fixed table counts its columns from the widest row: the group headings span only what shows
    for (const th of $$(".vs-group th", vs)) th.colSpan = which === "all" ? 5 : 3;
    if (animate) { vs.classList.remove("switched"); void vs.offsetWidth; vs.classList.add("switched"); }
  };
  for (const b of buttons) b.addEventListener("click", () => show(b.dataset.pick, true));
  pick.hidden = false;
  show("c3", false);
})();

// -- The visitor's region: the menu's language and country, the currency that follows ------------------------------
// A choice made here before (the menu or the price picker), else where the visitor most likely is: the time zone
// (Europe/Paris with an en-GB browser: euros), else the browser's region (en-US → USD, he → ILS…), else the default.
// A menu link (?region=uk) keeps its currency and leaves the page's own address.
const region = (function () {
  const R = data.regions || [], KEY = "bipagents.currency", listeners = [];
  const kept = () => { try { return localStorage.getItem(KEY); } catch (e) { return null; } };
  const keep = code => { try { localStorage.setItem(KEY, code); } catch (e) { /* private mode: not kept */ } };
  const currencyOf = country => (R.find(r => r.country === country) || {}).currency;
  const ZONES = [
    ["GB", /^(Europe\/(London|Belfast|Guernsey|Jersey|Isle_of_Man)|GB|GB-Eire)$/],
    ["IL", /^(Asia\/(Jerusalem|Tel_Aviv)|Israel)$/],
    ["SG", /^(Asia\/Singapore|Singapore)$/],
    ["CA", /^(Canada\/.+|America\/(Toronto|Montreal|Vancouver|Edmonton|Winnipeg|Regina|Halifax|St_Johns|Moncton|Glace_Bay|Goose_Bay|Whitehorse|Dawson|Dawson_Creek|Fort_Nelson|Creston|Iqaluit|Rankin_Inlet|Resolute|Cambridge_Bay|Inuvik|Yellowknife|Swift_Current|Thunder_Bay|Nipigon|Rainy_River|Atikokan|Blanc-Sablon|Pangnirtung|Coral_Harbour))$/],
    ["US", /^(US\/.+|Pacific\/Honolulu|America\/(New_York|Chicago|Denver|Los_Angeles|Phoenix|Anchorage|Juneau|Sitka|Yakutat|Nome|Metlakatla|Adak|Boise|Detroit|Menominee|Indianapolis|Louisville|Fort_Wayne|Knox_IN|Indiana\/.+|Kentucky\/.+|North_Dakota\/.+))$/],
  ];
  function guess() {
    let zone = "";
    try { zone = Intl.DateTimeFormat().resolvedOptions().timeZone || ""; } catch (e) { /* no time zone: the languages */ }
    const hit = ZONES.find(([, re]) => re.test(zone));
    if (hit) return currencyOf(hit[0]) || data.currency;
    if (/^(Europe\/|Atlantic\/(Canary|Madeira|Azores))/.test(zone)) return data.currency;   // in Europe, whatever the language
    for (const tag of navigator.languages || [navigator.language || ""]) {
      const parts = String(tag).split(/[-_]/);
      const country = (parts.slice(1).find(p => /^[A-Za-z]{2}$/.test(p)) || "").toUpperCase()
        || ({ he: "IL", iw: "IL" })[parts[0].toLowerCase()];
      const found = country && currencyOf(country);
      if (found) return found;
      if (country) break;   // a region (fr-FR, de-AT…) no other currency claims: the default
    }
    return data.currency;
  }
  const known = code => (data.currencies || []).includes(code);
  const params = new URLSearchParams(location.search);
  const asked = params.get("region"), chosen = R.find(r => r.id === asked && r.lang === data.lang);
  if (asked !== null) {
    if (chosen) keep(chosen.currency);
    params.delete("region");
    const rest = params.toString();
    try { history.replaceState(null, "", location.pathname + (rest ? "?" + rest : "") + location.hash); } catch (e) { /* file:// */ }
  }
  let currency = chosen ? chosen.currency : known(kept()) ? kept() : guess();
  const summary = $(".lang summary"), links = $$(".lang-menu a[data-region]");
  function show() {
    const here = R.find(r => r.lang === data.lang && r.currency === currency) || R.find(r => r.lang === data.lang && r.id === "eu");
    if (summary && here) summary.textContent = here.short;
    // What only holds in Europe (who can get each agent there) is hidden for the other regions: [data-only="eu"].
    document.documentElement.dataset.region = here ? here.id : "eu";
    for (const a of links) {
      if (here && a.lang === here.lang && a.dataset.region === here.id) a.setAttribute("aria-current", "page");
      else a.removeAttribute("aria-current");
    }
  }
  function set(code) {
    if (!known(code)) return;
    currency = code; keep(code); show();
    listeners.forEach(fn => fn(code));
  }
  // Another country in this language: no reload, the prices follow.
  for (const a of links) a.addEventListener("click", e => {
    const r = R.find(x => x.id === a.dataset.region && x.lang === a.lang);
    if (!r || r.lang !== data.lang) return;
    e.preventDefault();
    set(r.currency);
    a.closest("details").open = false;
  });
  show();
  return { get currency() { return currency; }, set, onChange: fn => listeners.push(fn) };
})();

// -- The plan picker: the price, live ------------------------------------------------------------------------------
(function pricing() {
  const root = $(".pricing");
  if (!root) return;
  const P = data.prices, L = data.pricing, lang = data.lang;
  // Each currency its own round prices (site/config.json), written as the page's language writes them.
  const LOCALES = { en: "en-US", fr: "fr-FR", de: "de-DE", es: "es-ES" };
  const formats = {};
  const money = (v, code) => {
    formats[code] = formats[code] || new Intl.NumberFormat(LOCALES[lang] || lang, { style: "currency", currency: code });
    return formats[code].format(Math.round(v * 100) / 100);
  };
  const say = (template, x) => template.replace("{x}", x);

  // The visitor's currency (the region above): the picker and the header's menu stay together.
  const state = { plan: "famille", period: "year", currency: P.currencies[region.currency] ? region.currency : P.default };
  const buy = $("#buy"), sticky = $(".sticky-buy"), picker = $("select[data-currency]", root);
  const fineTax = $(".fine-tax"), fineDefault = fineTax && fineTax.textContent;

  // The checkout's address, with the currency the visitor sees.
  function withCurrency(url, code) {
    if (!url) return url;
    if (/\{currency\}|%7Bcurrency%7D/i.test(url)) return url.replace(/\{currency\}|%7Bcurrency%7D/gi, code);
    try { const u = new URL(url, location.href); u.searchParams.set("currency", code); return u.href; } catch (e) { return url; }
  }

  function render() {
    const { plan, period, currency } = state;
    const C = P.currencies[currency], web = C.web, store = C.store;
    const m = v => money(v, currency);
    const tax = C.tax === "exclusive" ? " " + L.plusTax : "";
    $$("[data-period]", root).forEach(b => b.setAttribute("aria-checked", String(b.dataset.period === period)));
    root.dataset.period = period;
    root.dataset.shown = currency;
    if (picker) picker.value = currency;
    $$(".plan", root).forEach(b => {
      const p = b.dataset.plan, on = p === plan;
      b.setAttribute("aria-checked", String(on));
      b.tabIndex = on ? 0 : -1;
      $("[data-price]", b).textContent = m(web[period][p]);
      $("[data-unit]", b).textContent = (period === "year" ? L.perYear : L.perMonth) + tax;
      $("[data-equiv]", b).textContent = period === "year" ? say(L.equiv, m(web.year[p] / 12)) : "";
      $("[data-store]", b).textContent = store ? say(L.inApp, m(store[period][p])) : "";
    });
    const price = web[period][plan];
    $(".sum-plan", root).textContent = L.names[plan];
    $(".sum-price", root).textContent = m(price);
    $(".sum-unit", root).textContent = " " + (period === "year" ? L.perYear : L.perMonth) + tax;
    // the savings against the stores, when their price in this currency is known; else the year against 12 months
    const saved = !store ? (period === "year" ? web.month[plan] * 12 - price : 0)
      : period === "year" ? store.month[plan] * 12 - price : store.month[plan] - price;
    const line = !store ? L.saveYearWeb : period === "year" ? L.saveYear : L.saveMonth;
    $(".save", root).textContent = saved > 0 ? "✓ " + say(line, m(saved)) : "";
    if (buy) {
      buy.href = withCurrency(data.checkout[plan][period], currency) || buy.href;
      $(".buy-label", buy).textContent = L.buy.replace("{plan}", L.names[plan]).replace("{period}", L.period[period]);
    }
    $$("[data-from]").forEach(el => { el.textContent = m(web.month[P.plans[0]]); });
    if (fineTax) {
      const name = (L.currencies || {})[currency] || currency;
      fineTax.textContent = currency === P.default ? fineDefault
        : (C.tax === "exclusive" ? L.taxExcl : L.taxIncl).replace("{currency}", name);
    }
  }
  if (picker) picker.addEventListener("change", () => region.set(picker.value));
  region.onChange(code => { if (P.currencies[code]) { state.currency = code; render(); } });
  $$("[data-period]", root).forEach(b => b.addEventListener("click", () => { state.period = b.dataset.period; render(); }));
  const plans = $$(".plan", root);
  plans.forEach((b, i) => {
    b.addEventListener("click", () => { state.plan = b.dataset.plan; render(); });
    b.addEventListener("keydown", e => {   // a radio group: arrows choose
      const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
      if (!step) return;
      e.preventDefault();
      const next = plans[(i + step + plans.length) % plans.length];
      state.plan = next.dataset.plan; render(); next.focus();
    });
  });
  render();

  // The bar at the bottom of a phone's screen: once past the top, until the prices are in view.
  const seen = new Set(), section = $("#pricing");
  const showSticky = () => {
    const show = scrollY > 300 && !seen.size;
    sticky.classList.toggle("show", show);
    sticky.setAttribute("aria-hidden", String(!show));
    $("a", sticky).tabIndex = show ? 0 : -1;
  };
  const io = new IntersectionObserver(entries => {
    for (const e of entries) e.isIntersecting ? seen.add(e.target) : seen.delete(e.target);
    showSticky();
  });
  [$(".hero"), section, $(".foot")].forEach(el => el && io.observe(el));
})();

// -- Prices: « On your computer » opens its install in place (#install opens it too) --------------------------------
const selfOpen = $(".way-open");
if (selfOpen) disclosure(selfOpen, { hash: "#install" });

// -- Install: the command typed, then what it says -------------------------------------------------------------------
(function terminal() {
  const term = $(".terminal");
  if (!term) return;
  const typed = $(".typed", term), command = typed.dataset.command, outs = $$(".out", term);
  once(term, async () => {
    for (let i = 1; i <= command.length; i++) { typed.textContent = command.slice(0, i); await wait(REDUCED ? 0 : 18); }
    for (const out of outs) { await wait(600); out.classList.add("show"); }
  }, 0.4);
  const copy = $(".copy", term);
  copy.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(copy.dataset.copy); } catch { return; }
    const label = copy.textContent;
    copy.textContent = copy.dataset.done;
    setTimeout(() => { copy.textContent = label; }, 1800);
  });
})();
