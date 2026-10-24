// bipagents.com: a day with the Bips (with the morning podcast and a Live call in it), the privacy picture, the plan
// picker; on their own pages, the agent at work (the app's own demo scenarios, with its map and its pages) and the
// install. One script for every page: each part runs where its elements are. No framework, no network but this site.
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

// The hero card leans toward the pointer.
const tilt = $("[data-tilt]");
if (tilt && !REDUCED && matchMedia("(hover: hover)").matches) {
  addEventListener("pointermove", e => {
    const x = e.clientX / innerWidth - 0.5, y = e.clientY / innerHeight - 0.5;
    tilt.style.setProperty("--rx", `${(-y * 6).toFixed(2)}deg`);
    tilt.style.setProperty("--ry", `${(x * 8).toFixed(2)}deg`);
  }, { passive: true });
}

// -- Hero: the approval moment, in a loop — you ask, the agent fills the form, asks « Book it in your name? », a tap
// on « Once », booked. With reduced motion (or no script) the whole exchange stays still, « Once » chosen.
(async function ask() {
  const card = $(".ask");
  if (!card || REDUCED) return;
  const step = n => $(`[data-ask="${n}"]`, card), tool = step(2), once = $(".ask-once", card);
  const parts = $$("[data-ask]", card);
  let seen = true;   // paused while off screen (and in a background tab)
  new IntersectionObserver(([e]) => { seen = e.isIntersecting; }).observe(card);
  const visible = async () => { while (!seen || document.hidden) await new Promise(r => setTimeout(r, 400)); };
  card.classList.add("play");
  for (;;) {
    parts.forEach(p => p.classList.remove("on"));
    tool.classList.remove("done"); once.classList.remove("chosen", "tap");
    await wait(700); await visible();
    step(1).classList.add("on"); await wait(1100);
    tool.classList.add("on"); await wait(1500);
    tool.classList.add("done"); await wait(500);
    step(3).classList.add("on"); await wait(2200); await visible();
    once.classList.add("tap"); await wait(180);
    once.classList.add("chosen"); await wait(900);
    step(4).classList.add("on"); await wait(4200);
    parts.forEach(p => p.classList.remove("on")); await wait(600);
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
if (chat) once(chat, () => play(current), 0.35);   // the demo has its own page (/demo/)

// -- A day with the Bips: the phone follows the story -------------------------------------------------------------
(function day() {
  const steps = $$(".day-step"), scenes = $$(".scene"), clock = $(".day-clock"), stage = $(".day-stage");
  if (!steps.length) return;
  const bip = bips.get($(".day-bip"));
  // On a phone, each step carries its own copy of the screen (the sticky phone is for wide screens).
  steps.forEach((step, i) => {
    const copy = document.createElement("div");
    copy.className = "day-inline";
    copy.setAttribute("aria-hidden", "true");
    copy.append(scenes[i].cloneNode(true));
    step.prepend(copy);
  });
  let active = -1;
  const catOf = i => steps[i].dataset.cat;
  function show(i) {
    if (i === active) return;
    active = i;
    steps.forEach((s, j) => s.classList.toggle("on", j === i));
    scenes.forEach((s, j) => s.classList.toggle("on", j === i));
    clock.textContent = steps[i].dataset.time;
    stage.style.setProperty("--day-tint", `var(--${TONE[catOf(i)]}-tint)`);
    if (bip) { bip.c = CATEGORIES[catOf(i)]; bip.mood = "happy"; bip.level = null; if (!REDUCED) bip.jump(18); }
  }
  const io = new IntersectionObserver(entries => {
    for (const e of entries) if (e.isIntersecting) show(+e.target.dataset.step);
  }, { rootMargin: "-45% 0px -45% 0px" });
  steps.forEach(s => io.observe(s));
  show(0);

  // 7:30 — the demo's morning podcast. While it plays, the Bip beside the phone speaks with the audio, in the colour
  // of whichever host is talking (scripts/podcast-timeline.py found each turn in the recording).
  const audio = $("audio", steps[0]), button = $(".listen", steps[0]);
  if (audio && button) {
    const label = $(".listen-label", button), time = $(".listen-time", button);
    const playText = label.textContent, pauseText = button.dataset.pause;
    const HOSTS = ["daily", "wellness"], turns = data.podcast || [[0, 0]];
    const speakerAt = t => turns.reduce((who, [start, host]) => (t + 0.05 >= start ? host : who), turns[0][1]);
    const fmt = s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
    let analyser, wave, raf;
    const stop = () => {
      cancelAnimationFrame(raf);
      button.classList.remove("playing");
      label.textContent = playText;
      if (bip && active >= 0) { bip.mood = "happy"; bip.level = null; bip.c = CATEGORIES[catOf(active)]; }
    };
    audio.addEventListener("pause", stop);
    audio.addEventListener("ended", stop);
    button.addEventListener("click", async () => {
      if (!audio.paused) { audio.pause(); return; }
      if (!analyser) {
        try {
          const ctx = audioContext();
          analyser = ctx.createAnalyser(); analyser.fftSize = 1024; analyser.smoothingTimeConstant = 0.6;
          ctx.createMediaElementSource(audio).connect(analyser); analyser.connect(ctx.destination);
          wave = new Uint8Array(analyser.fftSize);
        } catch { analyser = null; }
      }
      button.classList.add("playing");
      label.textContent = pauseText;
      try { await audio.play(); } catch { stop(); return; }
      (function frame() {
        let level = 0.5;
        if (analyser) {
          analyser.getByteTimeDomainData(wave);
          let peak = 0;
          for (const v of wave) peak = Math.max(peak, Math.abs(v - 128));
          level = Math.min(1, peak / 45);
        }
        if (bip && active === 0) {
          bip.c = CATEGORIES[HOSTS[speakerAt(audio.currentTime)] ?? "daily"];
          bip.mood = level > 0.06 ? "speaking" : "happy"; bip.level = level > 0.06 ? level : null;
        }
        time.textContent = audio.duration ? `${fmt(audio.currentTime)} / ${fmt(audio.duration)}` : fmt(audio.currentTime);
        raf = requestAnimationFrame(frame);
      })();
    });
  }

  // 18:30 — a Live call in the car: what you say, what your Bip answers, in a loop while it's on screen.
  const calls = $$(".s-call"), turns = data.live || [];
  if (calls.length && turns.length && !REDUCED) {
    const inline = calls.find(c => c.closest(".day-inline"));
    let inlineSeen = false;
    if (inline) new IntersectionObserver(([e]) => { inlineSeen = e.isIntersecting; }).observe(inline);
    const step = scenes.indexOf(calls.find(c => !c.closest(".day-inline")));
    const each = fn => calls.forEach(fn);
    const setText = (who, text) => each(c => { $(`.s-cap.${who}`, c).textContent = text; });
    (async function loop() {
      for (let i = 0; ; i++) {
        if (active !== step && !inlineSeen) {   // off screen: the last exchange stays, whole
          each(c => c.classList.remove("anim"));
          while (active !== step && !inlineSeen) await wait(400);
        }
        each(c => c.classList.add("anim"));
        const turn = turns[i % turns.length], live = active === step && bip;
        setText("you", turn.you); setText("bip", turn.bip);
        each(c => { c.classList.remove("speaking", "answer"); c.classList.add("asking"); });
        if (live) bip.mood = "listening";
        await wait(2300);
        if (active === step && bip) bip.mood = "thinking";
        await wait(800);
        each(c => { c.classList.add("speaking", "answer"); });
        if (active === step && bip) bip.mood = "speaking";
        await wait(3600);
        each(c => c.classList.remove("asking", "answer", "speaking"));
        if (active === step && bip) bip.mood = "happy";
        await wait(700);
      }
    })();
  }
})();

// -- Why it's private: the message travels, once the picture is on screen ----------------------------------------
const compare = $(".compare");
if (compare) once(compare, () => compare.classList.add("go"), 0.4);

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
    $(".sb-plan", sticky).textContent = L.names[plan];
    $(".sb-price", sticky).textContent = (period === "year" ? m(price / 12) : m(price)) + " " + L.perMonth + tax;
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

  // The bar at the bottom of a phone's screen: once past the top, until the prices are in view; never while
  // « on my computer » is chosen (nothing to buy there).
  const seen = new Set(), section = $("#pricing");
  const showSticky = () => {
    const show = scrollY > 300 && !seen.size && section.dataset.mode !== "self";
    sticky.classList.toggle("show", show);
    sticky.setAttribute("aria-hidden", String(!show));
    $("a", sticky).tabIndex = show ? 0 : -1;
  };
  const io = new IntersectionObserver(entries => {
    for (const e of entries) e.isIntersecting ? seen.add(e.target) : seen.delete(e.target);
    showSticky();
  });
  [$(".hero"), section, $(".foot")].forEach(el => el && io.observe(el));
  section.addEventListener("modechange", showSticky);
})();

// -- The prices' switch: all included, or on my computer (#pricing / #install) --------------------------------------
(function modes() {
  const section = $("#pricing"), stage = $(".mode-stage");
  if (!section || !stage) return;
  const tabs = $$('[role="tab"]', section);
  const panels = { offer: $("#panel-offer"), self: $("#panel-self") };

  function choose(mode, { focus = false, animate = true } = {}) {
    if (section.dataset.mode === mode) return;
    const from = stage.offsetHeight;
    section.dataset.mode = mode;
    tabs.forEach(t => {
      const on = t.dataset.mode === mode;
      t.setAttribute("aria-selected", String(on));
      t.tabIndex = on ? 0 : -1;
      if (on && focus) t.focus();
    });
    for (const [name, panel] of Object.entries(panels)) panel.hidden = name !== mode;
    const shown = panels[mode];
    $$(".reveal", shown).forEach(el => el.classList.add("in"));
    section.dispatchEvent(new Event("modechange"));
    if (!animate || REDUCED) return;
    // The height goes from the old panel's to the new one's, the new panel rises in.
    shown.classList.remove("enter"); void shown.offsetWidth; shown.classList.add("enter");
    const to = stage.offsetHeight;
    stage.classList.remove("moving");
    stage.style.height = from + "px";
    void stage.offsetHeight;
    stage.classList.add("moving");
    stage.style.height = to + "px";
    const done = e => {
      if (e && e.target !== stage) return;
      stage.classList.remove("moving"); stage.style.height = ""; stage.removeEventListener("transitionend", done);
    };
    stage.addEventListener("transitionend", done);
    setTimeout(done, 700);
  }

  tabs.forEach((t, i) => {
    t.addEventListener("click", () => choose(t.dataset.mode));
    t.addEventListener("keydown", e => {   // a tab list: arrows, Home and End move and choose
      const next = { ArrowRight: i + 1, ArrowDown: i + 1, ArrowLeft: i - 1, ArrowUp: i - 1, Home: 0, End: tabs.length - 1 }[e.key];
      if (next === undefined) return;
      e.preventDefault();
      choose(tabs[(next + tabs.length) % tabs.length].dataset.mode, { focus: true });
    });
  });

  // Deep links: #install opens « on my computer », #pricing the offer — on arrival, and from any link on the page.
  const fromHash = (hash, animate) => {
    if (hash === "#install") choose("self", { animate });
    else if (hash === "#pricing") choose("offer", { animate });
  };
  fromHash(location.hash, false);
  addEventListener("hashchange", () => fromHash(location.hash, true));
  document.addEventListener("click", e => {
    const a = e.target.closest('a[href$="#install"], a[href$="#pricing"]');
    if (!a || a.pathname !== location.pathname) return;
    fromHash(a.hash, true);
    if (a.hasAttribute("data-mode-go")) {   // switching from inside the section: stay at its top
      e.preventDefault();
      section.scrollIntoView({ behavior: REDUCED ? "auto" : "smooth" });
      $('[role="tab"][aria-selected="true"]', section).focus({ preventScroll: true });
    }
  });
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
