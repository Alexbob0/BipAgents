// The Bips, drawn as in docs/play (same geometry as App/Design/MascotView.swift, 120×120 design space). Only
// constants and numbers go into the SVG: no text from an agent or a server ever reaches this markup.
const INK = "#16181D";
export const CATEGORIES = [
  { id: "wellness", label: "Bien-être", name: "Wellness", main: "#3CC37A", deep: "#1D8A4F", tint: "#E2F5EA", body: "blob", acc: "sprout", pitch: 520 },
  { id: "finance", label: "Finance", name: "Budget", main: "#F6B826", deep: "#9A6A00", tint: "#FFF3D1", body: "squircle", acc: "coin", pitch: 410 },
  { id: "daily", label: "Quotidien", name: "Vie", main: "#FF7B5C", deep: "#C9472A", tint: "#FFE8E0", body: "egg", acc: "ears", pitch: 600 },
  { id: "work", label: "Travail", name: "Boulot", main: "#4F7CFF", deep: "#2B55D4", tint: "#E4EBFF", body: "squircle", acc: "antenna", pitch: 450 },
  { id: "learning", label: "Apprendre", name: "Prof", main: "#8E6CF5", deep: "#6142D2", tint: "#EDE7FF", body: "cloud", acc: "tuft", pitch: 560 },
  { id: "home", label: "Maison", name: "Maison", main: "#1FB2C1", deep: "#0D7F8B", tint: "#DDF4F6", body: "egg", acc: "roof", pitch: 490 },
  { id: "creative", label: "Créatif", name: "Atelier", main: "#F2649E", deep: "#C23774", tint: "#FDE4EE", body: "blob", acc: "star", pitch: 640 },
  { id: "tech", label: "Tech", name: "Serveur", main: "#6B7489", deep: "#3F4659", tint: "#E7E9EE", body: "squircle", acc: "antenna", pitch: 470 },
];
const REDUCED = matchMedia("(prefers-reduced-motion: reduce)").matches;
const ell = (cx, cy, rx, ry, attrs) => `<ellipse cx="${cx}" cy="${cy}" rx="${rx}" ry="${Math.max(0, ry)}" ${attrs}/>`;
const path = (d, attrs) => `<path d="${d}" ${attrs}/>`;
const strokeLine = (w = 3) => `fill="none" stroke="${INK}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"`;

function bodyShape(c, fill) {
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
      case "ears":
        return [35, 85].map(x => ell(x, 36, 12, 12, `fill="${main}"`) + ell(x, 36, 5.5, 5.5, `fill="#FFB3C1"`)).join("");
      case "antenna": {
        const glow = 0.775 + 0.225 * Math.cos(t * 2 * Math.PI / 1.6);
        return path("M60 30V13", round(3.5)) + ell(60, 10, 6.5, 6.5, `fill="${main}" fill-opacity="${glow}" stroke="${deep}" stroke-width="3"`);
      }
      case "tuft":
        return path("M50 28c-2-8 2-14 8-17-1 6 2 9 4 12 1-6 5-10 11-11-3 5-3 10-2 16z", `fill="${deep}"`);
    }
  } else {
    if (c.acc === "star")
      return path("M60 3l3.6 7.4 8.1 1.1-5.9 5.6 1.5 8-7.3-3.9-7.3 3.9 1.5-8-5.9-5.6 8.1-1.1z", `fill="${gold}" stroke="${deep}" stroke-width="2.2" stroke-linejoin="round"`);
    if (c.acc === "roof")
      return path("M33 35L60 11l27 24", round(6));
  }
  return "";
}

function blinkScale(c, t) {
  const offset = CATEGORIES.indexOf(c) * 1.37;
  const phase = ((t + offset) % 4.6) / 4.6;
  if (phase <= 0.93) return 1;
  const local = (phase - 0.93) / 0.07;
  return Math.max(0.1, Math.abs(local * 2 - 1));
}

function eyes(c, t, gaze, rx, ry, y, xs, glint, blink = true) {
  const s = blink ? blinkScale(c, t) : 1;
  const lx = 3 * Math.max(-1, Math.min(1, gaze.x)), ly = 2.5 * Math.max(-1, Math.min(1, gaze.y));
  return xs.map(x0 => {
    const x = x0 + lx, yy = y + ly;
    return ell(x, yy, rx, ry * s, `fill="${INK}"`) + (s > 0.6 ? ell(x + rx * 0.38, yy - ry * 0.4, glint, glint, `fill="#fff"`) : "");
  }).join("");
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
  const L = strokeLine();
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
      return [47, 73].map(x => `<g transform="translate(${x} 64) rotate(${(t * 6 * 180 / Math.PI) % 360})">` + path(spiral(2.2, 7), strokeLine(2)) + `</g>`).join("")
        + path("M51 81q3-3 6 0t6 0t6 0", L);
    case "grumpy": return eyes(c, t, gaze, 5.5, 4.5, 66, [47, 73], 1.5, false) + path("M40 55l12 5M80 55l-12 5", strokeLine(3.5)) + path("M53 83q7-6 14 0", L);
    case "winking":
      return path("M41 65q6-6 12 0", L) + ell(73, 64, 5.5, 7.5, `fill="${INK}"`) + ell(75, 61, 2, 2, `fill="#fff"`)
        + path("M52 77q8 8 16 0", L) + ell(63, 82, 3, 2.4, `fill="#FF7A93"`);
    default: return eyes(c, t, gaze, 5.5, 7.5, 64, [47, 73], 2) + path("M53 78q7 6 14 0", L);
  }
}

export function renderBip(c, s, t) {
  const period = s.mood === "speaking" || s.mood === "listening" ? 0.9 : 3;
  const bob = REDUCED ? 0 : -2 - 2 * Math.sin(t * 2 * Math.PI / period);
  const cheek = 0.5 + 0.4 * Math.min(1, Math.max(0, s.blush));
  return `<svg viewBox="0 0 120 120" aria-hidden="true">`
    + `<g transform="translate(${s.offset.x} ${s.offset.y}) translate(60 106) rotate(${s.rotation}) scale(${s.scale.x} ${s.scale.y}) translate(-60 -106)">`
    + ell(60, 113, 32, 4.5, `fill="${INK}" fill-opacity="0.1"`)
    + [45, 75].map(x => ell(x, 106, 10.5, 6.5, `fill="${c.deep}"`) + ell(x - 3, 104.5, 3.5, 1.8, `fill="#fff" fill-opacity="0.25"`)).join("")
    + `<g transform="translate(0 ${bob})">`
    + accessory(c, false, t) + bodyShape(c, c.main) + accessory(c, true, t)
    + `<g transform="translate(43 44) rotate(-30)">` + ell(0, 0, 12, 7, `fill="#fff" fill-opacity="0.32"`) + `</g>`
    + ell(37, 76, 6.5, 4, `fill="#FF6F91" fill-opacity="${cheek}"`) + ell(83, 76, 6.5, 4, `fill="#FF6F91" fill-opacity="${cheek}"`)
    + face(c, s.mood, t, s.gaze, s.level)
    + `</g></g></svg>`;
}

// ----------------------------------------------------------------------------------------------------------------
// English by default, French with ?lang=fr or the FR button (remembered), or a French browser.

// The app's guess from a name (AgentCategory.suggest): the category the app picked is not on the server yet.
const KEYWORDS = {
  wellness: ["well", "santé", "sante", "health", "sommeil", "sleep", "sport", "fitness", "bien-être", "bien etre", "médit", "nutrition", "coach", "salud", "bienestar", "sueño", "gesund", "schlaf"],
  finance: ["financ", "budget", "banque", "bank", "argent", "money", "invest", "compta", "impôt", "impot", "tax", "dinero", "banco", "impuesto", "geld", "finanz", "steuer"],
  daily: ["vie", "life", "perso", "agenda", "quotidien", "organis", "assistant", "planning", "famille", "family", "daily", "vida", "familia", "alltag", "leben", "familie"],
  work: ["travail", "work", "pro", "bureau", "client", "projet", "business", "mail", "job", "trabajo", "oficina", "arbeit", "büro"],
  learning: ["appren", "learn", "étude", "etude", "langue", "cours", "school", "prof", "tutor", "aprend", "estudi", "idioma", "escuela", "lern", "schule", "sprach"],
  home: ["maison", "home", "domot", "jardin", "cuisine", "courses", "bricol", "garden", "kitchen", "casa", "hogar", "cocina", "haus", "garten", "küche"],
  creative: ["créa", "crea", "écri", "ecri", "write", "musique", "music", "design", "photo", "art", "escrit", "música", "creativ", "schreib", "musik", "kunst", "kreativ"],
  tech: ["tech", "dev", "code", "serveur", "server", "infra", "ops", "admin", "linux", "servidor", "technik"],
};

export function category(agent) {
  const wanted = agent.category && CATEGORIES.find(c => c.id === agent.category);
  if (wanted) return wanted;
  const text = `${agent.name || ""} ${agent.id || ""}`.toLowerCase();
  let best = null, score = 0;
  for (const c of CATEGORIES) {
    const n = KEYWORDS[c.id].filter(k => text.includes(k)).length;
    if (n > score) { best = c; score = n; }
  }
  return best || CATEGORIES.find(c => c.id === "daily");
}

// One animation loop for every Bip on the page; each draws itself into its element (blink, bob, thinking bubbles).
const drawn = new Set();
let started = 0;

function frame(now) {
  const t = (now - started) / 1000;
  for (const bip of drawn) bip.el.innerHTML = renderBip(bip.c, bip.state, REDUCED ? 0.5 : t);
  if (drawn.size) requestAnimationFrame(frame);
}

export function attach(el, c, mood = "idle") {
  const bip = { el, c, state: { mood, offset: { x: 0, y: 0 }, rotation: 0, scale: { x: 1, y: 1 }, blush: 0, gaze: { x: 0, y: 0 } } };
  const first = drawn.size === 0;
  drawn.add(bip);
  if (first) { started = started || performance.now(); requestAnimationFrame(frame); }
  return {
    set(c2, mood2) { bip.c = c2; bip.state.mood = mood2; },
    detach() { drawn.delete(bip); },
  };
}
