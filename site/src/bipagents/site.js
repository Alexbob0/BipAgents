// bipagents.com: « See more » on privacy, the agents' cards, the comparison, the
// two ways and the plan picker; on their own pages, the demo's clips (the real app, recorded) and the install. One
// script for every page: each part runs where its elements are. No framework, no network but this site.
import { Bip, REDUCED, iconSVG, staticBip } from "./bips.js";

const data = JSON.parse(document.getElementById("site-data").textContent);
const $ = (s, root = document) => root.querySelector(s);
const $$ = (s, root = document) => [...root.querySelectorAll(s)];
const wait = ms => new Promise(r => setTimeout(r, ms));
const esc = s => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const once = (el, fn, threshold = 0.3) => new IntersectionObserver((entries, io) => {
  if (entries[0].isIntersecting) { io.disconnect(); fn(); }
}, { threshold }).observe(el);

// -- Bips everywhere -----------------------------------------------------------------------------------------------
const bips = new Map();
for (const el of $$("[data-bip]")) {
  // The Bips one can tap wear today's costume during an event (Halloween, Christmas…), as in the apps.
  const live = el.hasAttribute("data-interactive");
  bips.set(el, new Bip(el, el.dataset.bip, { interactive: live, lines: data.lines, costume: live || null }));
}
for (const el of $$("[data-bip-static]")) el.innerHTML = staticBip(el.dataset.bipStatic);
for (const el of $$("[data-bip-icon]")) el.innerHTML = iconSVG();   // the app's icon (the brand)

// -- Header, progress, reveals, parallax ---------------------------------------------------------------------------
const top = $(".top"), bar = $(".progress i"), hero = $(".hero");
// the home's living room lies under the header: its height pulls the room up, and the header stays light-on-dark over it
const topHeight = () => document.documentElement.style.setProperty("--top-h", top.offsetHeight + "px");
if (hero) { topHeight(); addEventListener("resize", topHeight, { passive: true }); }
const parallax = REDUCED ? [] : $$("[data-parallax]");
let ticking = false;
function onScroll() {
  ticking = false;
  const y = scrollY, max = document.documentElement.scrollHeight - innerHeight;
  top.classList.toggle("scrolled", y > 8);
  if (hero) top.classList.toggle("off-hero", y >= hero.offsetHeight - top.offsetHeight);
  bar.style.transform = `scaleX(${max > 0 ? y / max : 0})`;
  for (const el of parallax) el.style.transform = `translateY(${(y * +el.dataset.parallax).toFixed(1)}px)`;
}
addEventListener("scroll", () => { if (!ticking) { ticking = true; requestAnimationFrame(onScroll); } }, { passive: true });
onScroll();
const revealer = new IntersectionObserver(entries => {
  for (const entry of entries) if (entry.isIntersecting) { entry.target.classList.add("in"); revealer.unobserve(entry.target); }
}, { threshold: 0.12 });
$$(".reveal").forEach(el => revealer.observe(el));

// -- The demo: one phone, the real app recorded; each example's button puts its clip in it (/demo/#<clip> opens on
// it). Only the chosen clip loads. It plays while in view and stops when it leaves; with reduced motion nothing starts
// by itself (its poster, a play button). The button plays or pauses it either way.
const clips = $("[data-clips]");
if (clips) {
  const clip = $(".clip", clips), video = $("video", clip), button = $(".clip-play", clip), cap = $("figcaption", clip);
  const exs = $$(".ex", clips);
  let seen = false, held = false;
  const show = () => {
    const on = !video.paused;
    button.setAttribute("aria-pressed", on ? "true" : "false");
    button.firstElementChild.textContent = on ? clips.dataset.pause : clips.dataset.play;
    clip.classList.toggle("playing", on);
  };
  const play = () => video.play().catch(() => {});
  const wants = () => seen && !REDUCED && !held && !document.hidden;
  video.addEventListener("play", show);
  video.addEventListener("pause", show);
  button.hidden = false;
  show();
  button.addEventListener("click", () => {
    if (video.paused) { held = false; play(); } else { held = true; video.pause(); }
  });
  function choose(ex, { focus = false } = {}) {
    if (ex.getAttribute("aria-pressed") === "true" && video.currentSrc) return;
    for (const other of exs) other.setAttribute("aria-pressed", other === ex ? "true" : "false");
    const d = ex.dataset;
    clip.style.setProperty("--tint", `var(--${d.tone}-tint)`);
    video.pause();
    video.poster = `${d.src}.jpg`;
    $("source", video).src = `${d.src}.mp4`;
    video.setAttribute("aria-label", `${d.name} ${d.title}`);
    video.load();
    $("i", cap).innerHTML = staticBip(d.cat);
    $("span", cap).innerHTML = `<b>${esc(d.name)}</b> ${esc(d.title)}`;
    $("small", cap).textContent = d.cap;
    $("#clip-alt", clip).textContent = d.alt;
    held = false;
    if (wants()) play();
    if (focus) ex.focus({ preventScroll: true });
  }
  for (const ex of exs) ex.addEventListener("click", () => {
    choose(ex);
    history.replaceState(null, "", `#${ex.id}`);
    ex.scrollIntoView({ block: "nearest", inline: "nearest" });   // a phone: the row slides to it
  });
  const fromHash = () => { const ex = exs.find(x => `#${x.id}` === location.hash); if (ex) choose(ex); };
  fromHash();
  addEventListener("hashchange", fromHash);
  new IntersectionObserver(([entry]) => {
    seen = entry.isIntersecting;
    if (wants()) play(); else video.pause();
  }, { threshold: 0.5 }).observe(video);
  addEventListener("visibilitychange", () => { if (wants()) play(); else video.pause(); });
}


// -- Private: « See the details » opens where the data is, who reads it and our limit in a window (#private-detail
// too); without the script its link shows them under the strip. ---------------------------------------------------
const privDetail = $(".pv-detail");
if (privDetail) {
  const openDetail = () => { if (!privDetail.open) privDetail.showModal(); };
  document.addEventListener("click", e => {
    if (!e.target.closest('a[href="#private-detail"]')) return;
    e.preventDefault();
    openDetail();
  });
  if (location.hash === "#private-detail") openDetail();
  $(".v-demo-close", privDetail).addEventListener("click", () => privDetail.close());
  privDetail.addEventListener("click", e => { if (e.target === privDetail) privDetail.close(); });   // the backdrop
}

// -- Open in place: a button that shows or hides the part it controls (aria-expanded), opened as well by its address
// (#install) on arrival and from any link to it on the page.
function disclosure(button, { hash, onOpen } = {}) {
  const panel = document.getElementById(button.getAttribute("aria-controls"));
  if (!panel) return null;
  function set(open, { scroll = false, instant = false } = {}) {
    button.setAttribute("aria-expanded", String(open));
    panel.hidden = !open;
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
  const state = { plan: "famille", period: "month", currency: P.currencies[region.currency] ? region.currency : P.default };
  const buy = $("#buy"), sticky = $(".sticky-buy"), picker = $("select[data-currency]", root);
  const fineTax = $(".fine-tax"), fineDefault = fineTax && fineTax.textContent;

  // The ad campaign this page was opened with (an ad's link: ?utm_campaign=…), read from the page's own address only:
  // no cookie, no storage; another page opened, it is gone (decided 2026-10-29). A short name or nothing, as the
  // guichet keeps it (attribution.rs), which hands it to Stripe Checkout's metadata.
  const campaign = (() => {
    try {
      const c = (new URLSearchParams(location.search).get("utm_campaign") || "").trim().toLowerCase();
      return /^[a-z0-9][a-z0-9._-]{0,47}$/.test(c) ? c : "";
    } catch (e) { return ""; }
  })();

  // The checkout's address, with the currency the visitor sees (and the campaign, when the page came from an ad).
  function withCurrency(url, code) {
    if (!url) return url;
    if (/\{currency\}|%7Bcurrency%7D/i.test(url)) url = url.replace(/\{currency\}|%7Bcurrency%7D/gi, code);
    try {
      const u = new URL(url, location.href);
      if (!/[?&]currency=/.test(url)) u.searchParams.set("currency", code);
      if (campaign) u.searchParams.set("campaign", campaign);
      return u.href;
    } catch (e) { return url; }
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
    $$("[data-famille-month]").forEach(el => { el.textContent = m(web.month.famille); });   // the footnote's price
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

  // The bar at the bottom of a phone's screen: once past the top, until the prices are in view (the home has the
  // team's bar instead).
  if (!sticky) return;
  const seen = new Set(), section = $("#pricing");
  const showSticky = () => {
    const show = scrollY > 300 && !seen.size;
    sticky.classList.toggle("on", show);
    sticky.setAttribute("aria-hidden", String(!show));
    $("a", sticky).tabIndex = show ? 0 : -1;
  };
  const io = new IntersectionObserver(entries => {
    for (const e of entries) e.isIntersecting ? seen.add(e.target) : seen.delete(e.target);
    showSticky();
  });
  [$(".hero"), section, $(".foot")].forEach(el => el && io.observe(el));
})();

// -- Build your team (the home): pick a Bip — it stands on the stage, alive, and says its line; its tile is on, its
// card shows; « Add to the team » puts it in the bar at the bottom, which names the team, and « Ready? » says who waits.
// Left alone, the Bips introduce themselves in turn until one is picked.
(function team() {
  const root = $("[data-pick]");
  if (!root) return;
  const tiles = $$(".rost", root), cards = $$(".fiche-card", root), add = $(".fiche-add", root);
  const stage = $("[data-stage-bip]", root), sayName = $(".stage-say small", root), sayText = $(".stage-say span", root);
  const dock = $("[data-dock]"), bench = $("[data-bench]"), dockN = $("[data-dock-n]"), end = $("[data-end]");
  const picked = new Set([tiles[0].dataset.id]);
  let current = null, bip = null, typing = 0, touched = false;

  function say(text) {
    const run = ++typing;
    if (REDUCED) { sayText.textContent = text; return; }
    sayText.textContent = "";
    bip.mood = "speaking";
    let k = 0;
    (function step() {
      if (run !== typing) return;
      sayText.textContent = text.slice(0, ++k);
      if (k < text.length) setTimeout(step, 22); else bip.mood = "idle";
    })();
  }
  function select(tile) {
    current = tile;
    root.dataset.tone = tile.dataset.tone;
    tiles.forEach(t => t.setAttribute("aria-pressed", String(t === tile)));
    cards.forEach(c => { c.hidden = c.dataset.card !== tile.dataset.id; });
    if (bip) bip.remove();
    bip = new Bip(stage, tile.dataset.cat, { interactive: true, lines: data.lines });
    sayName.textContent = tile.dataset.name;
    say(tile.dataset.line);
    render();
  }
  function render() {
    const on = picked.has(current.dataset.id), team = tiles.filter(t => picked.has(t.dataset.id)), n = team.length;
    add.setAttribute("aria-pressed", String(on));
    $("span", add).textContent = on ? add.dataset.added : add.dataset.add;
    tiles.forEach(t => t.classList.toggle("in-team", picked.has(t.dataset.id)));
    const count = d => (n ? (n > 1 ? d.many : d.one) : d.none).replace("{n}", n);
    if (dock) {
      bench.innerHTML = n ? team.map(t => `<i title="${esc(t.dataset.name)}">${staticBip(t.dataset.cat)}</i>`).join("")
        : '<i class="empty"></i><i class="empty"></i><i class="empty"></i>';
      dockN.textContent = count(dock.dataset);
    }
    if (end) {
      const names = team.map(t => t.dataset.name);
      const list = n > 1 ? names.slice(0, -1).join(", ") + " " + end.dataset.and + " " + names[n - 1] : names[0];
      end.textContent = count(end.dataset).replace("{names}", list);
    }
  }
  tiles.forEach(t => t.addEventListener("click", () => { touched = true; select(t); }));
  add.addEventListener("click", () => {
    touched = true;
    const id = current.dataset.id;
    picked.has(id) ? picked.delete(id) : picked.add(id);
    render();
    if (picked.has(id)) bip.react("boing");
  });
  stage.addEventListener("click", () => { touched = true; });
  select(tiles[0]);
  if (!REDUCED) {
    let seen = false;
    new IntersectionObserver(([e]) => { seen = e.isIntersecting; }, { threshold: 0.4 }).observe(root);
    setInterval(() => {
      if (touched || !seen || document.hidden) return;
      select(tiles[(tiles.indexOf(current) + 1) % tiles.length]);
    }, 5200);
  }
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
    for (let i = 1; i <= command.length; i++) { typed.textContent = command.slice(0, i); await wait(REDUCED ? 0 : 5); }
    for (const out of outs) { await wait(REDUCED ? 120 : 180); out.classList.add("show"); }
  }, 0.4);
  const copy = $(".copy", term);
  copy.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(copy.dataset.copy); } catch { return; }
    const label = copy.textContent;
    copy.textContent = copy.dataset.done;
    setTimeout(() => { copy.textContent = label; }, 1800);
  });
})();
