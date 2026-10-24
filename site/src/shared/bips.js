// Not under the MIT license: the Bips' drawings and sounds are reserved (LICENSE-ASSETS).
// The Bips for the websites: same geometry as App/Design/MascotView.swift (120×120 design space), same reactions as
// InteractiveMascot.swift, same « Animalese » as BipBabble.swift (docs/play/index.html is the playground).

export const INK = "#16181D";
// main, deep, tint: the apps' pale palette (the « sober » direction, 2026-10-24). deep is for drawing only: as text on
// the site, use site.css's --<tone>-deep, which is dark enough to read.
export const CATEGORIES = {
  wellness: { main: "#A9D9B8", deep: "#5E9E73", tint: "#EEF6EA", body: "blob", acc: "sprout", pitch: 520, voice: "mousse" },
  finance: { main: "#F2D48E", deep: "#B08A2E", tint: "#FAF3E3", body: "squircle", acc: "coin", pitch: 410, voice: "ours" },
  daily: { main: "#F4B5A3", deep: "#C9775F", tint: "#FBEFEA", body: "egg", acc: "ears", pitch: 600, voice: "colibri" },
  work: { main: "#B3C4F5", deep: "#6F86C9", tint: "#ECF0FD", body: "squircle", acc: "antenna", pitch: 450, voice: "galet" },
  learning: { main: "#CABCF3", deep: "#8873C9", tint: "#F2EFFB", body: "cloud", acc: "tuft", pitch: 560, voice: "lumen" },
  home: { main: "#9FD6DA", deep: "#4E9AA0", tint: "#E9F5F4", body: "egg", acc: "roof", pitch: 490, voice: "ours" },
  creative: { main: "#F2B5CE", deep: "#C47396", tint: "#FBEDF2", body: "blob", acc: "star", pitch: 640, voice: "lumen" },
  tech: { main: "#C3C8D3", deep: "#7F8696", tint: "#F1F2F5", body: "squircle", acc: "antenna", pitch: 470, voice: "galet" },
};
Object.entries(CATEGORIES).forEach(([id, c], i) => { c.id = id; c.index = i; });

export const REDUCED = matchMedia("(prefers-reduced-motion: reduce)").matches;

const ell = (cx, cy, rx, ry, a) => `<ellipse cx="${cx}" cy="${cy}" rx="${rx}" ry="${Math.max(0, ry)}" ${a}/>`;
const path = (d, a) => `<path d="${d}" ${a}/>`;
const line = (w = 3) => `fill="none" stroke="${INK}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"`;

function body(c, fill) {
  switch (c.body) {
    case "blob": return path("M60 24c25 0 42 17 42 43 0 25-16 39-42 39S18 92 18 67c0-26 17-43 42-43z", `fill="${fill}"`);
    case "squircle": return `<rect x="19" y="28" width="82" height="78" rx="32" fill="${fill}"/>`;
    case "egg": return path("M60 20c21 0 38 28 38 52 0 21-16 34-38 34S22 93 22 72c0-24 17-52 38-52z", `fill="${fill}"`);
    default: return path("M38 46c2-14 12-22 24-22 13 0 23 9 24 22 10 2 17 11 17 22 0 22-18 38-43 38S17 90 17 68c0-12 8-21 21-22z", `fill="${fill}"`);
  }
}

function accessory(c, front, t) {
  const { main, deep } = c, gold = "#FFD84D";
  const round = w => `fill="none" stroke="${deep}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"`;
  if (!front) {
    switch (c.acc) {
      case "sprout":
        return path("M60 27c0-7 1-13 3-17", round(4)) + path("M63 12c5-8 15-9 21-4-5 7-14 9-21 4z", `fill="${deep}"`)
          + path("M61 15c-5-7-14-8-19-3 5 6 13 8 19 3z", `fill="${main}" stroke="${deep}" stroke-width="2"`);
      case "coin": {
        const p = Math.sin(t * 2 * Math.PI / 2.4);
        return `<g transform="translate(60 ${12 - 1.5 - 1.5 * p}) rotate(${6 + 6 * p})">` + ell(0, 0, 9, 9, `fill="${gold}" stroke="${deep}" stroke-width="3"`)
          + path("M0 -4.5v9", round(2.6)) + `</g>` + path("M42 10l-4-3M78 10l4-3", round(2.5));
      }
      case "ears": return [35, 85].map(x => ell(x, 36, 12, 12, `fill="${main}"`) + ell(x, 36, 5.5, 5.5, `fill="#FFB3C1"`)).join("");
      case "antenna": {
        const glow = 0.775 + 0.225 * Math.cos(t * 2 * Math.PI / 1.6);
        return path("M60 30V13", round(3.5)) + ell(60, 10, 6.5, 6.5, `fill="${main}" fill-opacity="${glow}" stroke="${deep}" stroke-width="3"`);
      }
      case "tuft": return path("M50 28c-2-8 2-14 8-17-1 6 2 9 4 12 1-6 5-10 11-11-3 5-3 10-2 16z", `fill="${deep}"`);
    }
  } else {
    if (c.acc === "star")
      return path("M60 3l3.6 7.4 8.1 1.1-5.9 5.6 1.5 8-7.3-3.9-7.3 3.9 1.5-8-5.9-5.6 8.1-1.1z", `fill="${gold}" stroke="${deep}" stroke-width="2.2" stroke-linejoin="round"`);
    if (c.acc === "roof") return path("M33 35L60 11l27 24", round(6));
  }
  return "";
}

function blink(c, t) {
  const phase = ((t + c.index * 1.37) % 4.6) / 4.6;
  if (phase <= 0.93) return 1;
  return Math.max(0.1, Math.abs(((phase - 0.93) / 0.07) * 2 - 1));
}

function eyes(c, t, gaze, rx, ry, y, xs, glint, blinks = true) {
  const s = blinks ? blink(c, t) : 1;
  const lx = 3 * Math.max(-1, Math.min(1, gaze.x)), ly = 2.5 * Math.max(-1, Math.min(1, gaze.y));
  return xs.map(x0 => ell(x0 + lx, y + ly, rx, ry * s, `fill="${INK}"`)
    + (s > 0.6 ? ell(x0 + lx + rx * 0.38, y + ly - ry * 0.4, glint, glint, `fill="#fff"`) : "")).join("");
}

function spiral(turns, radius) {
  let d = "";
  for (let i = 0; i <= 60; i++) {
    const p = i / 60, a = p * turns * 2 * Math.PI, r = radius * p;
    d += (i ? "L" : "M") + (r * Math.cos(a)).toFixed(2) + " " + (r * Math.sin(a)).toFixed(2);
  }
  return d;
}

function face(c, mood, t, gaze, level) {
  const L = line();
  switch (mood) {
    case "listening": return eyes(c, t, gaze, 7, 9, 63, [46, 74], 2.6) + ell(60, 81, 3.6, 4.2, `fill="${INK}"`);
    case "thinking": {
      const pulse = 0.25 + 0.2 * (1 + Math.sin(t * 2 * Math.PI / 1.4));
      return eyes(c, t, gaze, 5, 6.5, 61, [49, 75], 1.8, false) + path("M54 80h12", L)
        + [[93, 30, 3], [101, 21, 4.4], [110, 10, 6]].map(([x, y, r]) => ell(x, y, r, r, `fill="${c.deep}" fill-opacity="${pulse}"`)).join("");
    }
    case "speaking": {
      const open = 0.3 + 0.7 * (level ?? (0.5 + 0.5 * Math.sin(t * 2 * Math.PI / 0.56)));
      return eyes(c, t, gaze, 5.5, 7.5, 64, [47, 73], 2)
        + `<g transform="translate(60 81) scale(1 ${open})">` + ell(0, -1, 8, 7, `fill="#2B1A1F"`) + ell(0, 3, 5, 2.6, `fill="#FF7A93"`) + `</g>`;
    }
    case "asking": return eyes(c, t, gaze, 5.5, 7.5, 64, [47, 73], 2) + path("M39 54l11-4M81 54l-11-4", L) + path("M53 81q3.5-3 7 0t7 0", L);
    case "sleeping": return path("M41 65q6 5 12 0M67 65q6 5 12 0", L) + ell(60, 80, 3, 2.2, `fill="${INK}"`);
    case "giggling": return path("M41 66q6-7 12 0M67 66q6-7 12 0", L) + path("M50 75q10 14 20 0z", `fill="#2B1A1F"`) + ell(60, 82, 4.5, 2.2, `fill="#FF7A93"`);
    case "content": return path("M41 64q6 5 12 0M67 64q6 5 12 0", L) + path("M54 78q6 5 12 0", L);
    case "surprised": return eyes(c, t, gaze, 7.5, 8.5, 62, [46, 74], 2.8, false) + path("M39 50l10-2M81 50l-10-2", L) + ell(60, 83, 4.5, 5.5, `fill="${INK}"`);
    case "dizzy":
      return [47, 73].map(x => `<g transform="translate(${x} 64) rotate(${(t * 6 * 180 / Math.PI) % 360})">` + path(spiral(2.2, 7), line(2)) + `</g>`).join("")
        + path("M51 81q3-3 6 0t6 0t6 0", L);
    case "grumpy": return eyes(c, t, gaze, 5.5, 4.5, 66, [47, 73], 1.5, false) + path("M40 55l12 5M80 55l-12 5", line(3.5)) + path("M53 83q7-6 14 0", L);
    case "winking":
      return path("M41 65q6-6 12 0", L) + ell(73, 64, 5.5, 7.5, `fill="${INK}"`) + ell(75, 61, 2, 2, `fill="#fff"`)
        + path("M52 77q8 8 16 0", L) + ell(63, 82, 3, 2.4, `fill="#FF7A93"`);
    default: return eyes(c, t, gaze, 5.5, 7.5, 64, [47, 73], 2) + path("M53 78q7 6 14 0", L);
  }
}

export function renderBip(c, s, t) {
  const period = s.mood === "speaking" || s.mood === "listening" ? 0.9 : 3;
  const bob = REDUCED ? 0 : -2 - 2 * Math.sin(t * 2 * Math.PI / period + c.index);
  const cheek = 0.5 + 0.4 * Math.min(1, Math.max(0, s.blush));
  return `<svg viewBox="0 0 120 120" aria-hidden="true" focusable="false">`
    + `<g transform="translate(${s.offset.x} ${s.offset.y}) translate(60 106) rotate(${s.rotation}) scale(${s.scale.x} ${s.scale.y}) translate(-60 -106)">`
    + ell(60, 113, 32, 4.5, `fill="${INK}" fill-opacity="0.1"`)
    + [45, 75].map(x => ell(x, 106, 10.5, 6.5, `fill="${c.deep}"`) + ell(x - 3, 104.5, 3.5, 1.8, `fill="#fff" fill-opacity="0.25"`)).join("")
    + `<g transform="translate(0 ${bob})">`
    + accessory(c, false, t) + body(c, c.main) + accessory(c, true, t)
    + `<g transform="translate(43 44) rotate(-30)">` + ell(0, 0, 12, 7, `fill="#fff" fill-opacity="0.32"`) + `</g>`
    + ell(37, 76, 6.5, 4, `fill="#FF6F91" fill-opacity="${cheek}"`) + ell(83, 76, 6.5, 4, `fill="#FF6F91" fill-opacity="${cheek}"`)
    + face(c, s.mood, t, s.gaze, s.level)
    + `</g></g></svg>`;
}

/** A still Bip, as SVG markup (avatars, favicons). */
export function staticBip(category, mood = "happy") {
  const c = CATEGORIES[category];
  return renderBip(c, { mood, gaze: { x: 0, y: 0 }, blush: 0, offset: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotation: 0 }, 0.5);
}

// -- Animalese ----------------------------------------------------------------------------------------------------

let audio;
export const sound = { on: true };
const VOWELS = { a: [800, 1250], e: [480, 1850], i: [300, 2300], o: [520, 900], u: [330, 850], y: [300, 2100] };
const PLOSIVES = new Set("bdgkpqt"), FRICATIVES = new Set("cfhjsvxz");

export function audioContext() {
  audio ??= new (window.AudioContext || window.webkitAudioContext)();
  if (audio.state === "suspended") audio.resume();
  return audio;
}

function voiceFor(c, mood) {
  const v = { base: c.pitch, syl: 0.055, spread: 2.5, drift: 0, bounce: 0, vol: 0.28 };
  if (mood === "giggling") { v.base *= 1.25; v.syl = 0.045; v.bounce = 3; }
  if (mood === "surprised") { v.base *= 1.15; v.drift = 5; }
  if (mood === "content") { v.base *= 0.92; v.syl = 0.075; v.spread = 1.2; v.vol = 0.22; }
  if (mood === "dizzy") { v.syl = 0.07; v.drift = -7; v.spread = 3.5; }
  if (mood === "grumpy") { v.base *= 0.72; v.syl = 0.065; v.spread = 1; v.drift = -2; }
  return v;
}

function syllable(f0, [f1, f2], dur, rate, onset, vol) {
  const n = Math.floor(rate * dur), out = new Float32Array(n), harmonics = [];
  for (let k = 1; k * f0 < Math.min(5000, rate / 2.2); k++) {
    const f = k * f0, shape = Math.exp(-(((f - f1) / 260) ** 2)) + 0.7 * Math.exp(-(((f - f2) / 320) ** 2)) + 0.06;
    harmonics.push([k, shape / k ** 0.6]);
  }
  const norm = harmonics.reduce((s, h) => s + h[1], 0), attack = rate * 0.004, release = rate * 0.018;
  const onsetLen = onset ? Math.floor(rate * (onset === "p" ? 0.006 : 0.014)) : 0;
  let phase = 0;
  for (let i = 0; i < n; i++) {
    phase += 2 * Math.PI * f0 * (1 + 0.04 * i / n) / rate;
    let value = 0;
    for (const [k, a] of harmonics) value += a * Math.sin(k * phase);
    let sample = (value / norm) * Math.min(1, i / attack) * Math.min(1, (n - i) / release);
    if (i < onsetLen) {
      const burst = (Math.random() * 2 - 1) * (onset === "p" ? 0.5 : 0.22);
      sample = sample * i / onsetLen + burst * (1 - i / onsetLen);
    }
    out[i] = sample * vol;
  }
  return out;
}

export function babble(text, c, mood) {
  if (!sound.on) return;
  const ctx = audioContext(), rate = ctx.sampleRate, v = voiceFor(c, mood);
  const letters = [...text.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "")];
  const sayable = letters.filter(ch => /\p{L}/u.test(ch));
  if (!sayable.length) return;
  const ending = [...letters].reverse().find(ch => !/\s|♡/.test(ch));
  const drift = v.drift + (ending === "?" ? 4 : ending === "!" ? 2 : ending === "…" ? -3 : 0);
  const out = [];
  let spoken = 0, last = VOWELS.a;
  for (const ch of letters) {
    if (/\s|,/.test(ch)) { out.push(new Float32Array(Math.floor(rate * v.syl * 0.8))); continue; }
    if (!/\p{L}/u.test(ch)) continue;
    const progress = sayable.length > 1 ? spoken / (sayable.length - 1) : 0;
    const semis = ((ch.codePointAt(0) % 5) - 2) / 2 * v.spread + drift * progress + (spoken % 2 === 0 ? v.bounce : -v.bounce / 2);
    if (VOWELS[ch]) last = VOWELS[ch];
    out.push(syllable(v.base * Math.pow(2, semis / 12), VOWELS[ch] ?? last, v.syl, rate, PLOSIVES.has(ch) ? "p" : FRICATIVES.has(ch) ? "f" : "", v.vol));
    spoken++;
  }
  const total = out.reduce((n, a) => n + a.length, 0), buffer = ctx.createBuffer(1, total, rate), data = buffer.getChannelData(0);
  let at = 0;
  for (const part of out) { data.set(part, at); at += part.length; }
  const src = ctx.createBufferSource();
  src.buffer = buffer;
  src.connect(ctx.destination);
  src.start();
}

// -- A Bip on the page ----------------------------------------------------------------------------------------------

const pick = a => a[Math.floor(Math.random() * a.length)];
const living = new Set();
let pointer = null;   // the page's pointer: every Bip looks at it

addEventListener("pointermove", e => { pointer = { x: e.clientX, y: e.clientY }; }, { passive: true });
addEventListener("pointerleave", () => { pointer = null; });

/**
 * `new Bip(element, "daily", { interactive, lines, mood })`: draws a Bip in `element` (any size: the SVG fills it),
 * looks at the pointer anywhere on the page, and, when interactive, reacts to taps, tickles and strokes as in the app.
 * `lines` gives what it says (`{ hop: [...], giggle: [...], ... }`); `bip.mood` and `bip.level` drive it from outside.
 */
export class Bip {
  constructor(element, category, options = {}) {
    this.c = CATEGORIES[category];
    this.el = element;
    this.lines = options.lines ?? {};
    this.base = options.mood ?? "happy";
    this.s = { mood: this.base, gaze: { x: 0, y: 0 }, blush: 0, offset: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotation: 0, level: null };
    this.reaction = null;
    this.visible = false;
    this.el.classList.add("bip");
    this.svg = document.createElement("div");
    this.svg.className = "bip-svg";
    this.el.append(this.svg);
    if (options.interactive) this.listen();
    new IntersectionObserver(([entry]) => { this.visible = entry.isIntersecting; }, { rootMargin: "120px" }).observe(this.el);
    living.add(this);
    this.draw(0);
  }

  set mood(value) { this.base = value; }
  get mood() { return this.base; }
  set level(value) { this.s.level = value; }

  show(mood, seconds) {
    this.reaction = mood;
    clearTimeout(this.moodTimer);
    this.moodTimer = setTimeout(() => { if (!this.sulking) this.reaction = null; }, seconds * 1000);
  }

  say(text, mood) {
    if (!text) return;
    if (!this.bubble) {
      this.bubble = document.createElement("div");
      this.bubble.className = "bip-bubble";
      this.el.append(this.bubble);
    }
    this.bubble.textContent = text;
    this.bubble.classList.add("show");
    clearTimeout(this.bubbleTimer);
    this.bubbleTimer = setTimeout(() => this.bubble.classList.remove("show"), 1600);
    babble(text, this.c, mood ?? this.reaction);
  }

  animate(to, ms = 220) {
    if (REDUCED) return;
    this.anim = { from: structuredClone({ offset: this.s.offset, scale: this.s.scale, rotation: this.s.rotation }), to, start: performance.now(), ms };
  }
  rest(ms = 420) { this.animate({ offset: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotation: 0 }, ms); }
  jump(h = 26) { this.animate({ offset: { x: 0, y: -h }, scale: { x: 0.88, y: 1.14 }, rotation: 0 }, 200); setTimeout(() => this.rest(450), 210); }
  pop(f = 1.1) { this.animate({ offset: this.s.offset, scale: { x: f, y: f }, rotation: 0 }, 180); setTimeout(() => this.rest(380), 190); }
  spin() { this.animate({ offset: { x: 0, y: 0 }, scale: { x: 1, y: 1 }, rotation: 360 }, 700); setTimeout(() => { this.s.rotation = 0; }, 720); }
  shake(times, angle) {
    for (let i = 0; i < times; i++) setTimeout(() => this.animate({ offset: this.s.offset, scale: this.s.scale, rotation: i % 2 ? -angle : angle }, 70), i * 70);
    setTimeout(() => this.rest(300), times * 70);
  }
  burst(kind, count) {
    for (let i = 0; i < count; i++) {
      const p = document.createElement("span");
      p.className = "bip-particle";
      p.textContent = kind === "heart" ? "♥" : "✦";
      p.style.color = kind === "heart" ? "#FF5C8A" : this.c.deep;
      p.style.left = `${50 + (Math.random() * 70 - 35)}%`;
      p.style.animationDelay = `${Math.random() * 0.25}s`;
      this.el.append(p);
      setTimeout(() => p.remove(), 1900);
    }
  }
  blushUp() { this.s.blush = 1; setTimeout(() => { this.s.blush = 0; }, 1600); }
  line(kind) { return pick(this.lines[kind] ?? [""]); }

  tapped() {
    if (this.sulking) return;
    pick([
      () => { this.show("happy", 0.8); this.say(this.line("hop")); this.jump(); },
      () => { this.show("winking", 1.1); this.say(this.line("wink")); this.pop(1.08); },
      () => { this.show("giggling", 0.9); this.say(this.line("spin")); this.spin(); },
      () => { this.show("surprised", 1.0); this.say(this.line("surprised")); this.pop(1.14); },
      () => { this.show("content", 1.4); this.say(this.line("thanks")); this.burst("heart", 3); this.blushUp(); },
    ])();
  }
  giggle() { this.show("giggling", 1.8); this.say(this.line("giggle")); this.burst("sparkle", 4); this.shake(8, 9); }
  pet() { this.show("content", 2); this.say(this.line("pet")); this.burst("heart", 2); this.blushUp(); }
  annoyed() {
    this.sulking = true;
    this.show("dizzy", 1.8); this.say(this.line("dizzy")); this.shake(5, 14);
    setTimeout(() => { this.reaction = "grumpy"; this.say(this.line("grumpy")); }, 1800);
    setTimeout(() => { this.sulking = false; this.reaction = null; }, 4200);
  }

  listen() {
    let down = null, handled = false, squeezeTimer, squeezed = false, moved = 0, rub, taps = [], pokes = [];
    const rubber = d => Math.sign(d) * Math.min(Math.abs(d), 40 * (1 - Math.exp(-Math.abs(d) / 40)));
    this.el.tabIndex = 0;
    this.el.setAttribute("role", "button");
    this.el.addEventListener("pointerdown", e => {
      this.el.setPointerCapture(e.pointerId);
      audioContext();
      const now = performance.now();
      down = { x: e.clientX, y: e.clientY };
      moved = 0; squeezed = false; handled = false;
      rub = { last: null, dir: 0, reversals: [], travelled: 0, start: now };
      taps = taps.filter(t => now - t < 1200).concat(now);
      pokes = pokes.filter(t => now - t < 6000).concat(now);
      if (this.sulking) handled = true;
      else if (pokes.length >= 8) { pokes = []; taps = []; this.annoyed(); handled = true; }
      else if (taps.length >= 3) { taps = []; this.giggle(); handled = true; }
      squeezeTimer = setTimeout(() => {
        if (moved < 15 && !this.sulking) { squeezed = true; this.animate({ offset: this.s.offset, scale: { x: 1.14, y: 0.82 }, rotation: 0 }, 250); }
      }, 350);
    });
    this.el.addEventListener("pointermove", e => {
      if (!down) return;
      const dx = e.clientX - down.x, dy = e.clientY - down.y;
      moved = Math.hypot(dx, dy);
      if (moved > 15) {
        clearTimeout(squeezeTimer);
        if (squeezed) { squeezed = false; this.rest(300); }
      }
      this.anim = null;
      this.s.offset = { x: rubber(dx) / 1.4, y: rubber(dy) / 1.4 };
      const now = performance.now();
      if (rub.last) {
        const ddx = e.clientX - rub.last.x;
        rub.travelled += Math.abs(ddx) + Math.abs(e.clientY - rub.last.y);
        const dir = Math.sign(ddx);
        if (dir && rub.dir && dir !== rub.dir && Math.abs(ddx) > 1) rub.reversals.push(now);
        if (dir) rub.dir = dir;
      }
      rub.last = { x: e.clientX, y: e.clientY };
      rub.reversals = rub.reversals.filter(t => now - t < 900);
      if (!this.sulking && rub.reversals.length >= 4) { rub.reversals = []; this.giggle(); handled = true; }
      else if (!this.sulking && !rub.petted && now - rub.start > 600 && rub.travelled > 45 && rub.reversals.length <= 1) { rub.petted = true; this.pet(); handled = true; }
    });
    const up = () => {
      if (!down) return;
      clearTimeout(squeezeTimer);
      if (squeezed && !this.sulking) { this.show("surprised", 0.9); this.say(this.line("boing")); this.jump(40); handled = true; }
      else if (moved > 6) { this.rest(450); handled = true; }
      if (!handled) this.tapped();
      down = null;
    };
    this.el.addEventListener("pointerup", up);
    this.el.addEventListener("pointercancel", up);
    this.el.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); this.tapped(); } });
  }

  draw(t) {
    const now = performance.now();
    if (this.anim) {
      const p = Math.min(1, (now - this.anim.start) / this.anim.ms), k = 1 - Math.pow(1 - p, 3) * (1 - 1.6 * p * (1 - p));
      const lerp = (a, b) => a + (b - a) * k, { from, to } = this.anim;
      this.s.offset = { x: lerp(from.offset.x, to.offset.x), y: lerp(from.offset.y, to.offset.y) };
      this.s.scale = { x: lerp(from.scale.x, to.scale.x), y: lerp(from.scale.y, to.scale.y) };
      this.s.rotation = lerp(from.rotation, to.rotation);
      if (p >= 1) this.anim = null;
    }
    // Eyes on the pointer, wherever it is on the page.
    if (pointer) {
      const r = this.el.getBoundingClientRect();
      const dx = pointer.x - (r.left + r.width / 2), dy = pointer.y - (r.top + r.height / 2);
      const far = Math.max(1, Math.hypot(dx, dy));
      const target = { x: dx / Math.max(far, 220), y: dy / Math.max(far, 220) };
      this.s.gaze = { x: this.s.gaze.x + (target.x - this.s.gaze.x) * 0.2, y: this.s.gaze.y + (target.y - this.s.gaze.y) * 0.2 };
    } else {
      this.s.gaze = { x: this.s.gaze.x * 0.9, y: this.s.gaze.y * 0.9 };
    }
    this.s.mood = this.reaction ?? this.base;
    this.svg.innerHTML = renderBip(this.c, this.s, t);
  }
}

const t0 = performance.now();
(function loop(now) {
  const t = REDUCED ? 0 : (now - t0) / 1000;
  for (const bip of living) if (bip.visible) bip.draw(t);
  requestAnimationFrame(loop);
})(t0);
