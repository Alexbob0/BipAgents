// The 404 page (src/bipagents/404.html): « Un Bip a mangé la page ». A chubby, guilty Bip has swallowed the page; shake
// it (drag it back and forth, tap it, or Space/Enter on it) and it spits out « 4 », « 0 », « 4 » one by one, then burps
// out the link home. Shaken too hard, too fast: dizzy (« Tout tourne… »), then grumpy (« Hé ! Doucement… »).
// No library, no request of its own: the site's Bip engine, drawn from scripts/pixel/pixel.json.
import { Bip, REDUCED, audioContext, iconSVG } from "./bips.js";
import D from "./pixel-data.js";

const data = JSON.parse(document.getElementById("nf-data").textContent);
const $ = (id) => document.getElementById(id);
const pick = (a) => a[Math.floor(Math.random() * a.length)];

// -- The page's own looks, added to the engine's data (the same object bips.js reads) -----------------------------
// A chubby body (8 × 7 instead of 6 × 6) with a paler belly, and a corner of the eaten page sticking out of its mouth.
const BELLY = "#F9D2C6";
const chubby = (paper) => ({
  name: "Repu", body: { rows: ["..####..", ".######.", "########", "########", "##bbbb##", "##bbbb##", ".######."],
    eyeRow: 2, eyeCols: [2, 5], mouthRow: 3 },
  palette: { b: BELLY, w: ["#FFFFFF", "#D3CFC8"], g: "#B9B4AC" },
  items: paper ? [{ id: "page", layer: "top", at: "mouth", dx: 1.4, dy: -1.6, scale: 1.15, rot: 14,
    rows: ["wwww", "wggw", "wwww", "wggw"] }] : [],
});
D.COSTUMES.nf_full = chubby(true);
D.COSTUMES.nf_empty = chubby(false);
// A frown: the engine's own when it has one (branch bip-grumpy-face), else the smile upside down.
if (!D.MOUTHS.frown) D.MOUTHS.frown = { g: [".###.", "#...#"] };
const wave = { y: [0.8, 2.5], sy: [0.012, 2.5] };
// At rest: guilty — eyes darting left and right, worried brows, the mouth pushed to one side.
D.STATES.nf_guilty = { dur: 4.2, still: 0.5, wave, tracks: {
  eyes: [[0, "open"], [3.3, "closed"], [3.42, "open"]],
  gaze: [[0, [-1, 0]], [0.9, [1, 0]], [1.5, [-1, 0]], [2.3, [0, 1]], [3.0, [1, 0]], [3.6, [0, 0]]],
  brows: [[0, [0, -1]]], mouth: [[0, "hmm"]] } };
// Done: happy eyes, a big grin, swaying.
D.STATES.nf_full = { dur: 3, still: 0.5, wave: { ...wave, rot: [3, 3] }, tracks: { eyes: [[0, "happy"]], mouth: [[0, "grin"]] } };
D.REACTIONS.nf_shake = { dur: 0.45, still: 0.12, tracks: {
  eyes: [[0, "wide"], [0.12, "closed"], [0.3, "open"]], mouth: [[0, "o"], [0.12, "wavy"], [0.36, "hmm"]],
  brows: [[0, [0, -1]]], sx: [[0, 1], [0.06, 1.08], [0.18, 0.95], [0.32, 1]], sy: [[0, 1], [0.06, 0.92], [0.18, 1.05], [0.32, 1]] } };
D.REACTIONS.nf_spit = { dur: 1.0, still: 0.2, tracks: {
  eyes: [[0, "closed"], [0.12, "wide"], [0.5, "happy"], [0.9, "open"]], mouth: [[0, "flat"], [0.12, "obig"], [0.45, "grin"], [0.9, "hmm"]],
  y: [[0, 0], [0.12, -6], [0.3, 0]], sy: [[0, 1], [0.08, 0.86], [0.16, 1.1], [0.32, 1]], sx: [[0, 1], [0.08, 1.12], [0.16, 0.94], [0.32, 1]] } };
D.REACTIONS.nf_burp = { dur: 1.3, still: 0.3, tracks: {
  eyes: [[0, "closed"], [0.2, "wide"], [0.8, "happy"]], mouth: [[0, "flat"], [0.2, "obig"], [0.8, "grin"]],
  y: [[0, 0], [0.18, 2], [0.32, -10], [0.55, 0]], sy: [[0, 1], [0.18, 0.84], [0.32, 1.14], [0.55, 1]],
  sx: [[0, 1], [0.18, 1.14], [0.32, 0.92], [0.55, 1]], sparks: [[0.3, 0], [0.45, 1], [0.6, 2], [0.75, 3], [0.95, -1]] } };
// Grumpy: the engine's when it has one, else frowning brows, the frown, a « ! », and it turns a little away.
const GRUMPY = D.REACTIONS.grumpy ? "grumpy" : D.REACTIONS.grognon ? "grognon" : "nf_grumpy";
D.REACTIONS.nf_grumpy = { dur: 2.8, still: 1.0, tracks: {
  eyes: [[0, "open"], [2.2, "closed"], [2.32, "open"]],
  brows: [[0, [1, 1]]], mouth: [[0, "frown"]], bang: [[0, 1], [1.6, -1]],
  rot: [[0, 0], [0.2, -5], [2.4, -5], [2.8, 0]], x: [[0, 0], [0.12, 2], [0.24, -2], [0.36, 1], [0.48, 0]] } };

// -- Language: ?lang=, else the address's (/fr/…), else the browser's ----------------------------------------------
const LANGS = Object.keys(data.texts);
const HOME = { en: "/", fr: "/fr/", de: "/de/", es: "/es/" };
function guess() {
  const asked = new URLSearchParams(location.search).get("lang");
  if (LANGS.includes(asked)) return asked;
  const first = location.pathname.split("/")[1];
  if (LANGS.includes(first)) return first;
  for (const l of navigator.languages || [navigator.language || ""]) {
    const short = String(l).slice(0, 2).toLowerCase();
    if (LANGS.includes(short)) return short;
  }
  return "en";
}
let lang = guess(), t = data.texts[lang];

function applyLang(next) {
  lang = next; t = data.texts[lang];
  document.documentElement.lang = lang;
  document.title = t.title;
  $("nf-line").textContent = t.line;
  $("nf-hint").textContent = done ? t.done : t.hint;
  $("nf-bip").setAttribute("aria-label", t.bip);
  $("nf-langs").setAttribute("aria-label", t.lang);
  for (const a of document.querySelectorAll("[data-home]")) a.href = HOME[lang];
  for (const a of document.querySelectorAll("#nf-back, #nf-prize a")) a.textContent = t.back;
  for (const a of document.querySelectorAll("#nf-langs a")) a.toggleAttribute("aria-current", a.dataset.lang === lang);
}

// -- The scene -------------------------------------------------------------------------------------------------------
for (const el of document.querySelectorAll("[data-bip-icon]")) el.innerHTML = iconSVG();
const el = $("nf-bip"), slots = [...document.querySelectorAll(".nf-slot")];
let done = false, bip = null;
bip = new Bip(el, "daily", { mood: "nf_guilty", costume: "nf_full" });
el.tabIndex = 0;
el.setAttribute("role", "button");
applyLang(lang);
for (const a of document.querySelectorAll("#nf-langs a")) a.addEventListener("click", (e) => { e.preventDefault(); applyLang(a.dataset.lang); });

// A few paper crumbs on the floor.
const crumbs = $("nf-crumbs");
for (const [x, r] of [[8, 20], [19, -30], [74, 12], [86, -18], [93, 40]]) {
  const c = document.createElement("i");
  c.className = "nf-crumb";
  c.style.left = x + "%";
  c.style.transform = `rotate(${r}deg)`;
  crumbs.append(c);
}

// The digits, in pixels like the Bips (5 × 7).
const GLYPH = { 4: ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
  0: [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."] };
function digitSVG(d) {
  let s = "";
  GLYPH[d].forEach((row, y) => [...row].forEach((ch, x) => { if (ch === "#") s += `<rect x="${x * 10 + 0.5}" y="${y * 10 + 0.5}" width="9" height="9" rx="2"/>`; }));
  return `<svg class="nf-digit" viewBox="0 0 50 70" aria-hidden="true">${s}</svg>`;
}

const mouth = () => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height * 0.5 }; };
function fly(node, to, done, { arc = 120, spin = 0, duration = 900 } = {}) {
  node.classList.add("nf-fly");
  document.body.append(node);
  const from = mouth(), w = node.offsetWidth, h = node.offsetHeight;
  const x0 = from.x - w / 2, y0 = from.y - h / 2, x1 = to.x - w / 2, y1 = to.y - h / 2;
  const mid = (a, b) => (a + b) / 2;
  const top = document.querySelector(".nf-top").getBoundingClientRect().bottom - 10;
  const peak = Math.max(top, Math.min(y0, y1) - arc);   // never over the header
  const anim = node.animate([
    { transform: `translate(${x0}px, ${y0}px) scale(.2) rotate(0deg)`, offset: 0 },
    { transform: `translate(${mid(x0, x1)}px, ${peak}px) scale(1.1) rotate(${spin / 2}deg)`, offset: 0.45, easing: "ease-in" },
    { transform: `translate(${x1}px, ${y1}px) scale(1.15, .8) rotate(${spin}deg)`, offset: 0.72 },
    { transform: `translate(${x1}px, ${y1 - 18}px) scale(.95, 1.08) rotate(${spin}deg)`, offset: 0.84 },
    { transform: `translate(${x1}px, ${y1}px) scale(1.05, .95) rotate(${spin}deg)`, offset: 0.93 },
    { transform: `translate(${x1}px, ${y1}px) scale(1) rotate(${spin}deg)`, offset: 1 },
  ], { duration, easing: "ease-out", fill: "forwards" });
  // landed: when the animation ends, or after its time anyway (a page in the background draws no frames)
  let landed = false;
  const end = () => { if (landed) return; landed = true; anim.cancel(); node.remove(); done(); };
  anim.onfinish = end;
  setTimeout(end, duration + 150);
}
const center = (node) => { const r = node.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };

// -- The game --------------------------------------------------------------------------------------------------------
// Each jolt (a change of direction while dragging, a tap, Space/Enter) adds to the charge; a full charge spits a digit.
// Ten jolts within 2.5 s: dizzy, then grumpy, deaf to shaking until it has calmed down.
const NEED = 3, TAP = REDUCED ? NEED : 1.5, TOO_MANY = [10, 2500];
const DIGITS = ["4", "0", "4"];
let charge = 0, next = 0, busy = false, flying = false, recent = [], chatter = 0;

function jolt(weight) {
  if (busy || done) return;
  const now = performance.now();
  recent = recent.filter((s) => now - s < TOO_MANY[1]).concat(now);
  if (recent.length >= TOO_MANY[0] && !REDUCED) return dizzy();
  if (flying) return;
  bip.react("nf_shake");
  charge += weight;
  if (charge >= NEED && next < DIGITS.length) { charge = 0; spit(); return; }
  if (++chatter % 2 === 0 && Math.random() < 0.7) bip.say(pick(t.hmm));
}

function spit() {
  const i = next++, slot = slots[i];
  flying = true;
  bip.react("nf_spit");
  bip.say(i === DIGITS.length - 1 ? pick(t.hic) : pick([...t.hic, ...t.boing]));
  const land = () => {
    slot.innerHTML = digitSVG(DIGITS[i]);
    slot.classList.add("full");
    $("nf-live").textContent = t.digits.replace("{d}", DIGITS.slice(0, i + 1).join(" "));
    flying = false;
    if (next === DIGITS.length) setTimeout(burp, REDUCED ? 300 : 700);   // (after the sulk, if it sulks)
  };
  if (REDUCED) return land();
  const d = document.createElement("span");
  d.innerHTML = digitSVG(DIGITS[i]);
  fly(d, center(slot), land, { arc: 40, spin: i === 1 ? 360 : 0 });
}

function burp() {
  if (busy) { setTimeout(burp, 300); return; }
  done = true;
  bip.react("nf_burp");
  bip.say(pick(t.burp));
  bip.costume = "nf_empty";
  bip.mood = "nf_full";
  $("nf-hint").textContent = t.done;
  const link = document.createElement("a");
  link.className = "pill pill-ink";
  link.dataset.home = "";   // follows the language switch too
  link.href = HOME[lang];
  link.textContent = t.back;
  const prize = $("nf-prize");
  // the plain link under it goes: the same link twice would be one too many
  const land = () => { prize.replaceChildren(link); $("nf-back").parentElement.hidden = true; };
  if (REDUCED) return land();
  const ghost = link.cloneNode(true);
  fly(ghost, center(prize), land, { arc: 70, duration: 1000 });
}

function dizzy() {
  busy = true; recent = []; charge = 0;
  bip.react("vertige");
  bip.say(pick(t.dizzy));
  setTimeout(() => {
    bip.react(GRUMPY);
    bip.say(pick(t.grumpy));
    setTimeout(() => { busy = false; recent = []; }, (D.REACTIONS[GRUMPY].dur || 2.8) * 1000);
  }, (D.REACTIONS.vertige.dur || 2.2) * 1000);
}

// -- Input: drag (mouse, touch, pen), tap, keyboard ---------------------------------------------------------------
let drag = null;
el.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  try { el.setPointerCapture(e.pointerId); } catch { /* a pointer already gone */ }
  el.classList.remove("back");
  el.classList.add("dragging");
  drag = { id: e.pointerId, x0: e.clientX, y0: e.clientY, at: performance.now(), moved: 0, dir: 0, axis: null };
  try { audioContext(); } catch { /* no sound */ }
});
el.addEventListener("pointermove", (e) => {
  if (!drag || e.pointerId !== drag.id) return;
  const dx = e.clientX - drag.x0, dy = e.clientY - drag.y0;
  drag.moved = Math.max(drag.moved, Math.hypot(dx, dy));
  if (!REDUCED) {
    const cx = Math.max(-70, Math.min(70, dx * 0.7)), cy = Math.max(-36, Math.min(36, dy * 0.45));
    el.style.transform = `translate(${cx}px, ${cy}px) rotate(${cx * 0.12}deg)`;
  }
  // a jolt: the pointer turns back after at least 22 px one way, along the axis the drag started on
  if (!drag.axis && drag.moved > 12) {
    drag.axis = Math.abs(dx) >= Math.abs(dy) ? "x" : "y";
    drag.turn = drag.axis === "y" ? drag.y0 : drag.x0;
    drag.ext = drag.turn;
  }
  if (!drag.axis) return;
  const p = drag.axis === "y" ? e.clientY : e.clientX, sign = Math.sign(p - drag.ext);
  if (!sign) return;
  if (!drag.dir || sign === drag.dir) { drag.dir = sign; drag.ext = p; return; }
  if (Math.abs(p - drag.ext) < 6) return;   // a wobble, not a turn yet
  if (Math.abs(drag.ext - drag.turn) >= 22) jolt(1);
  drag.turn = drag.ext; drag.dir = sign; drag.ext = p;
});
function release(e) {
  if (!drag || e.pointerId !== drag.id) return;
  const tap = drag.moved < 10 && performance.now() - drag.at < 500;
  drag = null;
  el.classList.remove("dragging");
  el.classList.add("back");
  el.style.transform = "";
  if (tap && e.type === "pointerup") nudge();
}
el.addEventListener("pointerup", release);
el.addEventListener("pointercancel", release);
el.addEventListener("keydown", (e) => {
  if (e.key !== "Enter" && e.key !== " ") return;
  e.preventDefault();
  if (!e.repeat) nudge();
});
// A tap or a key: a small shake of its own, worth half a good one.
function nudge() {
  if (!REDUCED && !busy && !done) el.animate([{ transform: "rotate(0)" }, { transform: "rotate(-7deg)" }, { transform: "rotate(6deg)" },
    { transform: "rotate(-3deg)" }, { transform: "rotate(0)" }], { duration: 380, easing: "ease-out" });
  jolt(TAP);
}
