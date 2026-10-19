// bipagents.com: the agent at work (the app's own demo scenarios), the morning podcast, Live, the install.
import { Bip, CATEGORIES, REDUCED, audioContext, staticBip } from "./bips.js";

const data = JSON.parse(document.getElementById("site-data").textContent);
const $ = (s, root = document) => root.querySelector(s);
const $$ = (s, root = document) => [...root.querySelectorAll(s)];
const wait = ms => new Promise(r => setTimeout(r, REDUCED ? Math.min(ms, 120) : ms));

// -- Bips everywhere -----------------------------------------------------------------------------------------------
const bips = new Map();
for (const el of $$("[data-bip]")) {
  const bip = new Bip(el, el.dataset.bip, { interactive: el.hasAttribute("data-interactive"), lines: data.lines });
  bips.set(el, bip);
}
for (const el of $$("[data-bip-static]")) el.innerHTML = staticBip(el.dataset.bipStatic);

// -- Header, reveals -------------------------------------------------------------------------------------------------
const top = $(".top");
addEventListener("scroll", () => top.classList.toggle("scrolled", scrollY > 8), { passive: true });
const revealer = new IntersectionObserver(entries => {
  for (const entry of entries) if (entry.isIntersecting) { entry.target.classList.add("in"); revealer.unobserve(entry.target); }
}, { threshold: 0.12 });
$$(".reveal").forEach(el => revealer.observe(el));

// -- Hero: notifications arriving one after the other ---------------------------------------------------------------
(async function toasts() {
  const all = $$(".toast");
  if (!all.length) return;
  let i = 0;
  await wait(900);
  for (;;) {
    const toast = all[i % all.length];
    toast.classList.remove("leave");
    toast.classList.add("show");
    await wait(3600);
    toast.classList.add("leave");
    toast.classList.remove("show");
    await wait(500);
    toast.classList.remove("leave");
    i++;
  }
})();

// -- An agent at work: the app's demo scenarios, played in a chat --------------------------------------------------
const chat = $(".chat"), body = $(".chat-body"), wrap = $(".chat-wrap");
let run = 0;
const ICONS = { memory: "🧠", browser_navigate: "🌐", browser_snapshot: "👀", browser_click: "👆", calendar: "📅",
                ha_get_state: "🏠", ha_call_service: "🔥", web_search: "🔎", email: "✉️" };
const esc = s => s.replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const md = s => esc(s).replace(/\*\*(.+?)\*\*/g, "<b>$1</b>").replace(/\n\n/g, "<br><br>").replace(/\n/g, "<br>");

function add(html, cls) {
  const el = document.createElement("div");
  el.className = cls;
  el.innerHTML = html;
  body.append(el);
  while (body.scrollHeight > body.clientHeight && body.children.length > 1) body.firstElementChild.remove();
  return el;
}

function avatar(category) { return `<span class="av">${staticBip(category)}</span>`; }

async function play(id) {
  const mine = ++run;
  const scenario = data.scenarios[id], category = scenario.category, c = CATEGORIES[category];
  chat.style.setProperty("--chat-tint", `var(--${tone(category)}-tint)`);
  chat.style.setProperty("--chat-main", c.main);
  chat.style.setProperty("--chat-deep", c.deep);
  wrap.style.setProperty("--chat-tint", `var(--${tone(category)}-tint)`);
  $(".chat-avatar").innerHTML = staticBip(category);
  $(".chat-name").textContent = scenario.agent;
  body.innerHTML = "";
  const alive = () => mine === run;
  await wait(500); if (!alive()) return;
  add(esc(scenario.prompt), "msg you");
  let answer = "";
  const fill = s => s.replaceAll("{answer}", answer);

  async function steps(list) {
    for (const step of list) {
      if (!alive()) return false;
      if (step.think) {
        const typing = add("<i></i><i></i><i></i>", "typing");
        await wait(1100); typing.remove();
        add(esc(fill(step.think)), "think");
        await wait(1300);
      } else if (step.tool) {
        const el = add(`<span class="ic">${ICONS[step.tool] ?? "⚙️"}</span><span><b>${esc(step.tool)}</b><span>${esc(fill(step.preview ?? ""))}</span></span><i class="st"></i>`, "tool");
        await wait(Math.max(700, (step.seconds ?? 1) * 1000));
        el.classList.add("done");
        await wait(250);
      } else if (step.ask) {
        const card = add(`<p>${esc(step.ask.question)}</p><div class="opts">${step.ask.choices.map(o => `<span class="opt">${esc(o)}</span>`).join("")}</div>`, "card-q");
        await wait(1700); if (!alive()) return false;
        const choice = $$(".opt", card)[Math.min(1, step.ask.choices.length - 1)];
        choice.classList.add("chosen");
        answer = choice.textContent.replace(/ · ★.*$/, "");
        await wait(900);
        if (step.then && !(await steps(step.then))) return false;
      } else if (step.approve) {
        const card = add(`<p>${esc(fill(step.approve.description))}</p><code>${esc(fill(step.approve.command))}</code>`
          + `<div class="btns"><span>${esc(data.labels.once)}</span><span>${esc(data.labels.session)}</span><span>${esc(data.labels.deny)}</span></div>`, "card-a");
        await wait(1900); if (!alive()) return false;
        $(".btns span", card).classList.add("chosen");
        await wait(800);
        if (step.then && !(await steps(step.then))) return false;
      } else if (step.say) {
        const typing = add("<i></i><i></i><i></i>", "typing");
        await wait(1000); typing.remove();
        add(`${avatar(category)}<span class="txt">${md(fill(step.say))}</span>`, "msg bot");
        await wait(1200);
      } else if (step.pause) {
        await wait(step.pause * 1000);
      }
    }
    return true;
  }
  await steps(scenario.steps);
}

function tone(category) {
  return { daily: "coral", wellness: "green", finance: "gold", home: "teal", work: "blue", learning: "violet", creative: "pink", tech: "blue" }[category];
}

let current = "book_table";
for (const tab of $$(".tab")) {
  tab.addEventListener("click", () => {
    $$(".tab").forEach(t => { t.classList.toggle("active", t === tab); t.setAttribute("aria-selected", String(t === tab)); });
    current = tab.dataset.scenario;
    play(current);
  });
}
$(".replay")?.addEventListener("click", () => play(current));
let started = false;
new IntersectionObserver(([entry]) => {
  if (entry.isIntersecting && !started) { started = true; play(current); }
}, { threshold: 0.35 }).observe(chat);

// -- The morning podcast: two Bips, their mouths on the audio -------------------------------------------------------
(function podcast() {
  const studio = $(".studio");
  if (!studio) return;
  const audio = $("audio", studio), button = $(".play", studio), time = $(".time", studio), bars = $$(".wave i", studio);
  const hosts = $$(".host", studio).map(h => ({ el: h, bip: bips.get($(".host-bip", h)) }));
  let analyser, wave, freq, raf, quiet = 0;
  // Who speaks when: each turn's start, found in the recording (scripts/podcast-timeline.py), not guessed from pauses.
  const turns = data.podcast || [[0, 0]];
  const speakerAt = t => turns.reduce((who, [start, host]) => (t + 0.05 >= start ? host : who), turns[0][1]);
  const fmt = s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
  const idle = () => {
    hosts.forEach(h => { h.bip.mood = "happy"; h.bip.level = null; h.el.classList.remove("on"); });
    bars.forEach((b, i) => { b.style.height = `${18 + 14 * Math.abs(Math.sin(i * 1.7))}%`; });
  };
  idle();
  audio.addEventListener("ended", () => { button.parentElement.parentElement.classList.remove("playing"); cancelAnimationFrame(raf); idle(); });
  button.addEventListener("click", async () => {
    const ctx = audioContext();
    if (!analyser) {
      analyser = ctx.createAnalyser(); analyser.fftSize = 1024; analyser.smoothingTimeConstant = 0.6;
      ctx.createMediaElementSource(audio).connect(analyser); analyser.connect(ctx.destination);
      wave = new Uint8Array(analyser.fftSize); freq = new Uint8Array(analyser.frequencyBinCount);
    }
    const player = button.closest(".studio");
    if (!audio.paused) { audio.pause(); player.classList.remove("playing"); cancelAnimationFrame(raf); idle(); return; }
    player.classList.add("playing");
    await audio.play();
    (function frame() {
      analyser.getByteTimeDomainData(wave); analyser.getByteFrequencyData(freq);
      let peak = 0;
      for (const v of wave) peak = Math.max(peak, Math.abs(v - 128));
      const level = Math.min(1, peak / 45);
      quiet = level < 0.06 ? quiet + 1 : 0;
      const speaker = speakerAt(audio.currentTime);
      hosts.forEach((h, i) => {
        const on = i === speaker && quiet < 20;
        h.bip.mood = on ? "speaking" : "happy"; h.bip.level = on ? level : null; h.el.classList.toggle("on", on);
      });
      const progress = audio.duration ? audio.currentTime / audio.duration : 0;
      bars.forEach((b, i) => {
        const v = freq[Math.floor(4 + i * (freq.length * 0.45) / bars.length)] / 255;
        b.style.height = `${12 + v * 88}%`;
        b.classList.toggle("past", i / bars.length < progress);
      });
      time.textContent = fmt(audio.currentTime);
      raf = requestAnimationFrame(frame);
    })();
  });
})();

// -- Live: a hands-free conversation, in a loop ---------------------------------------------------------------------
(async function live() {
  const stage = $(".live-stage");
  if (!stage) return;
  const bip = bips.get($(".live-bip", stage)), you = $(".caption-bubble.you", stage), said = $(".caption-bubble.bip", stage);
  let visible = false;
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; }).observe(stage);
  for (let i = 0; ; i++) {
    while (!visible) await wait(400);
    const turn = data.live[i % data.live.length];
    bip.mood = "listening"; stage.classList.remove("speaking");
    you.textContent = turn.you; you.classList.add("show");
    await wait(2300);
    bip.mood = "thinking";
    await wait(900);
    bip.mood = "speaking"; stage.classList.add("speaking");
    said.textContent = turn.bip; said.classList.add("show");
    await wait(3600);
    you.classList.remove("show"); said.classList.remove("show");
    bip.mood = "happy"; stage.classList.remove("speaking");
    await wait(900);
  }
})();

// -- Install: the command typed, then what it says -------------------------------------------------------------------
(function terminal() {
  const term = $(".terminal");
  if (!term) return;
  const typed = $(".typed", term), command = typed.dataset.command, outs = $$(".out", term);
  let done = false;
  new IntersectionObserver(async ([e]) => {
    if (!e.isIntersecting || done) return;
    done = true;
    for (let i = 1; i <= command.length; i++) { typed.textContent = command.slice(0, i); await wait(REDUCED ? 0 : 22); }
    for (const out of outs) { await wait(650); out.classList.add("show"); }
  }, { threshold: 0.4 }).observe(term);
  const copy = $(".copy", term);
  copy.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(copy.dataset.copy); } catch { return; }
    const label = copy.textContent;
    copy.textContent = copy.dataset.done;
    setTimeout(() => { copy.textContent = label; }, 1800);
  });
})();
