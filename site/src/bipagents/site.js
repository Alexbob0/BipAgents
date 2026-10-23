// bipagents.com: the agent at work (the app's own demo scenarios, with its map and its pages), a day with the Bips,
// the morning podcast, Live, the privacy picture, the plan picker, the install. No framework, no network but this site.
import { Bip, CATEGORIES, REDUCED, audioContext, staticBip } from "./bips.js";

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

// The hero phone leans toward the pointer.
const tilt = $("[data-tilt]");
if (tilt && !REDUCED && matchMedia("(hover: hover)").matches) {
  addEventListener("pointermove", e => {
    const x = e.clientX / innerWidth - 0.5, y = e.clientY / innerHeight - 0.5;
    tilt.style.setProperty("--rx", `${(-y * 6).toFixed(2)}deg`);
    tilt.style.setProperty("--ry", `${(x * 8).toFixed(2)}deg`);
  }, { passive: true });
}

// -- Hero: notifications arriving one after the other ---------------------------------------------------------------
(async function toasts() {
  const all = $$(".toast");
  if (!all.length) return;
  await wait(900);
  for (let i = 0; ; i++) {
    const toast = all[i % all.length];
    toast.classList.remove("leave"); toast.classList.add("show");
    await wait(3600);
    toast.classList.add("leave"); toast.classList.remove("show");
    await wait(500);
    toast.classList.remove("leave");
  }
})();

// -- An agent at work: the app's demo scenarios, played in a chat --------------------------------------------------
const chat = $(".chat"), body = $(".chat-body"), wrap = $(".chat-wrap");
let run = 0;
const ICONS = { memory: "🧠", browser_navigate: "🌐", browser_snapshot: "👀", browser_click: "👆", calendar: "📅",
                ha_get_state: "🏠", ha_call_service: "🔥", web_search: "🔎", email: "✉️", weather: "⛅", map_image: "🗺️",
                health_read: "❤️", page: "🧩", todo: "✅", terminal: "⌨️" };
const md = s => esc(s).replace(/\*\*(.+?)\*\*/g, "<b>$1</b>").replace(/\n\n/g, "<br><br>").replace(/\n/g, "<br>");

function add(html, cls) {
  const el = document.createElement("div");
  el.className = cls;
  el.innerHTML = html;
  body.append(el);
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
  chat.style.setProperty("--chat-deep", c.deep);
  $(".chat-avatar").innerHTML = staticBip(category);
  $(".chat-name").textContent = scenario.agent;
  body.innerHTML = "";
  const alive = () => mine === run;
  await wait(500); if (!alive()) return;
  add(esc(scenario.prompt), "msg you");
  let answer = "", madePage = null;
  const fill = s => s.replaceAll("{answer}", answer);

  async function say(text) {
    const typing = add("<i></i><i></i><i></i>", "typing");
    await wait(1000); typing.remove();
    if (!alive()) return;
    add(`${avatar(category)}<span class="txt">${md(text)}</span>`, "msg bot");
    await wait(1100);
  }

  async function steps(list) {
    for (const step of list) {
      if (!alive()) return false;
      if (step.think) {
        const typing = add("<i></i><i></i><i></i>", "typing");
        await wait(1100); typing.remove();
        add(esc(fill(step.think)), "think");
        await wait(1300);
      } else if (step.tool || step.page) {
        const tool = step.tool ?? "page";
        const el = add(`<span class="ic">${ICONS[tool] ?? "⚙️"}</span><span><b>${esc(tool)}</b><span>${esc(fill(step.preview ?? ""))}</span></span><i class="st"></i>`, "tool");
        await wait(Math.max(700, (step.seconds ?? 1) * 1000));
        el.classList.add("done");
        if (step.page) madePage = step.page;
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
once(chat, () => play(current), 0.35);

// -- A day with the Bips: the phone follows the story -------------------------------------------------------------
(function day() {
  const steps = $$(".day-step"), scenes = $$(".scene"), clock = $(".day-clock"), stage = $(".day-stage");
  if (!steps.length) return;
  const bipEl = $(".day-bip"), bip = bips.get(bipEl);
  // On a phone, each step carries its own copy of the screen (the sticky phone is for wide screens).
  steps.forEach((step, i) => {
    const copy = document.createElement("div");
    copy.className = "day-inline";
    copy.setAttribute("aria-hidden", "true");
    copy.append(scenes[i].cloneNode(true));
    step.prepend(copy);
  });
  let active = -1;
  function show(i) {
    if (i === active) return;
    active = i;
    steps.forEach((s, j) => s.classList.toggle("on", j === i));
    scenes.forEach((s, j) => s.classList.toggle("on", j === i));
    clock.textContent = steps[i].dataset.time;
    const cat = steps[i].dataset.cat;
    stage.style.setProperty("--day-tint", `var(--${TONE[cat]}-tint)`);
    if (bip) { bip.c = CATEGORIES[cat]; if (!REDUCED) bip.jump(18); }
  }
  const io = new IntersectionObserver(entries => {
    for (const e of entries) if (e.isIntersecting) show(+e.target.dataset.step);
  }, { rootMargin: "-45% 0px -45% 0px" });
  steps.forEach(s => io.observe(s));
  show(0);
})();

// -- The morning podcast: two Bips, their mouths on the audio -------------------------------------------------------
(function podcast() {
  const studio = $(".studio");
  if (!studio) return;
  const audio = $("audio", studio), button = $(".play", studio), time = $(".time", studio), bars = $$(".wave i", studio);
  const hosts = $$(".host", studio).map(h => ({ el: h, bip: bips.get($(".host-bip", h)) }));
  const playLabel = button.getAttribute("aria-label"), pauseLabel = button.dataset.pause;
  let analyser, wave, freq, raf, quiet = 0;
  // Who speaks when: each turn's start, found in the recording (scripts/podcast-timeline.py), not guessed from pauses.
  const turns = data.podcast || [[0, 0]];
  const speakerAt = t => turns.reduce((who, [start, host]) => (t + 0.05 >= start ? host : who), turns[0][1]);
  const fmt = s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
  const idle = () => {
    hosts.forEach(h => { h.bip.mood = "happy"; h.bip.level = null; h.el.classList.remove("on"); });
    bars.forEach((b, i) => { b.style.height = `${18 + 14 * Math.abs(Math.sin(i * 1.7))}%`; });
  };
  const stop = () => { studio.classList.remove("playing"); button.setAttribute("aria-label", playLabel); cancelAnimationFrame(raf); idle(); };
  idle();
  audio.addEventListener("ended", stop);
  button.addEventListener("click", async () => {
    const ctx = audioContext();
    if (!analyser) {
      analyser = ctx.createAnalyser(); analyser.fftSize = 1024; analyser.smoothingTimeConstant = 0.6;
      ctx.createMediaElementSource(audio).connect(analyser); analyser.connect(ctx.destination);
      wave = new Uint8Array(analyser.fftSize); freq = new Uint8Array(analyser.frequencyBinCount);
    }
    if (!audio.paused) { audio.pause(); stop(); return; }
    studio.classList.add("playing");
    button.setAttribute("aria-label", pauseLabel);
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

// -- Why it's private: the message travels, once the picture is on screen ----------------------------------------
const compare = $(".compare");
if (compare) once(compare, () => compare.classList.add("go"), 0.4);

// -- The plan picker: the price, live ------------------------------------------------------------------------------
(function pricing() {
  const root = $(".pricing");
  if (!root) return;
  const P = data.prices, L = data.pricing, lang = data.lang;
  const money = v => {
    const [whole, cents] = v.toFixed(2).split(".");
    const group = whole.replace(/\B(?=(\d{3})+(?!\d))/g, lang === "en" ? "," : " ");
    return lang === "en" ? `€${group}.${cents}` : `${group},${cents} €`;
  };
  const say = (template, x) => template.replace("{x}", x);
  const state = { plan: "famille", period: "year" };
  const buy = $("#buy"), sticky = $(".sticky-buy");

  function render() {
    const { plan, period } = state;
    $$("[data-period]", root).forEach(b => b.setAttribute("aria-checked", String(b.dataset.period === period)));
    root.dataset.period = period;
    $$(".plan", root).forEach(b => {
      const p = b.dataset.plan, on = p === plan;
      b.setAttribute("aria-checked", String(on));
      b.tabIndex = on ? 0 : -1;
      $("[data-price]", b).textContent = money(P.web[period][p]);
      $("[data-unit]", b).textContent = period === "year" ? L.perYear : L.perMonth;
      $("[data-equiv]", b).textContent = period === "year" ? say(L.equiv, money(P.web.year[p] / 12)) : "";
      $("[data-store]", b).textContent = say(L.inApp, money(P.store[period][p]));
    });
    const price = P.web[period][plan];
    $(".sum-plan", root).textContent = L.names[plan];
    $(".sum-price", root).textContent = money(price);
    $(".sum-unit", root).textContent = " " + (period === "year" ? L.perYear : L.perMonth);
    const saved = period === "year" ? P.store.month[plan] * 12 - price : P.store.month[plan] - price;
    $(".save", root).textContent = saved > 0 ? "✓ " + say(period === "year" ? L.saveYear : L.saveMonth, money(saved)) : "";
    if (buy) {
      buy.href = data.checkout[plan][period] || buy.href;
      $(".buy-label", buy).textContent = L.buy.replace("{plan}", L.names[plan]).replace("{period}", L.period[period]);
    }
    $(".sb-plan", sticky).textContent = L.names[plan];
    $(".sb-price", sticky).textContent = (period === "year" ? money(price / 12) : money(price)) + " " + L.perMonth;
  }
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
  const seen = new Set();
  const io = new IntersectionObserver(entries => {
    for (const e of entries) e.isIntersecting ? seen.add(e.target) : seen.delete(e.target);
    const show = scrollY > 300 && !seen.size;
    sticky.classList.toggle("show", show);
    sticky.setAttribute("aria-hidden", String(!show));
    $("a", sticky).tabIndex = show ? 0 : -1;
  });
  [$(".hero"), $("#pricing"), $(".end-section"), $(".foot")].forEach(el => el && io.observe(el));
})();

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
