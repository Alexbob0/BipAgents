// Not under the MIT license: the Bips' drawings and sounds are reserved (LICENSE-ASSETS).
// The Bips for the websites, drawn like the apps draw them (PixelEngine.swift / PixelEngine.kt, the design page
// « Les Bips se déguisent »): every number comes from scripts/pixel/pixel.json, which the build writes next to this
// file as pixel-data.js. Body cells of P, face pixels of U = P/3, the mouth in the eyes' colour, each open eye one
// solid rounded square; states with key times (idle, think, talk, listen, work, happy, sleep, surprised, error), the
// tap reactions, and the costumes with their seasons. Same « Animalese » as BipBabble.swift.
import D from "./pixel-data.js";

const { P, U, GAP, BOTTOM, CX, INK, LIGHT, COLORS, BODIES, EYE_STYLES, MOUTHS, GLYPHS, SPARK_FRAMES, STATES, REACTIONS,
  REACTION_ORDER, MOUTH_MIN, MOUTH_MAX, COSTUMES, SEASONS } = D;
export { INK, COSTUMES, SEASONS };

const W = typeof window === "object" ? window : null;
export const REDUCED = !!(W && W.matchMedia && W.matchMedia("(prefers-reduced-motion: reduce)").matches);

// Each kind of agent's colour, as in the apps (AgentCategory.pixelColor); tint is the site's pale card ground.
// main/deep come from pixel.json (deep is for drawing only: as text use site.css's --<tone>-deep).
export const CATEGORIES = {
  wellness: { color: "menthe", tint: "#EEF6EA", pitch: 520, voice: "mousse" },
  finance: { color: "miel", tint: "#FAF3E3", pitch: 410, voice: "ours" },
  daily: { color: "peche", tint: "#FBEFEA", pitch: 600, voice: "colibri" },
  work: { color: "bleuet", tint: "#ECF0FD", pitch: 450, voice: "galet" },
  learning: { color: "lavande", tint: "#F2EFFB", pitch: 560, voice: "lumen" },
  home: { color: "lagon", tint: "#E9F5F4", pitch: 490, voice: "ours" },
  creative: { color: "rose", tint: "#FBEDF2", pitch: 640, voice: "lumen" },
  tech: { color: "perle", tint: "#F1F2F5", pitch: 470, voice: "galet" },
};
Object.entries(CATEGORIES).forEach(([id, c], i) => {
  Object.assign(c, { id, index: i, main: COLORS[c.color].body, deep: COLORS[c.color].deep });
});

// The site's words for a Bip's mood → the apps' states.
const MOODS = {
  happy: "idle", idle: "idle", thinking: "think", think: "think", speaking: "talk", talk: "talk", listening: "listen",
  listen: "listen", working: "work", work: "work", sleeping: "sleep", sleep: "sleep", content: "happy",
  giggling: "happy", surprised: "surprised", error: "error",
};
const stateOf = mood => STATES[MOODS[mood] || mood] || STATES.idle;

// -- Engine (the page's evalState / drawPixel / viewBoxOf) ------------------------------------------------------------

const FX = ["dots", "sound", "sparks", "scan", "heart", "z", "bang"];
function stepAt(keys, t) { let v = keys[0][1]; for (const [k, val] of keys) { if (k <= t) v = val; else break; } return v; }
function smoothAt(keys, t) {
  if (t <= keys[0][0]) return keys[0][1];
  for (let i = 0; i < keys.length - 1; i++) {
    const [a, va] = keys[i], [b, vb] = keys[i + 1];
    if (t < b) { let u = (t - a) / (b - a); u = u * u * (3 - 2 * u); return va + (vb - va) * u; }
  }
  return keys[keys.length - 1][1];
}
function evalState(st, t) {
  const tr = st.tracks, d = {}, m = { x: 0, y: 0, rot: 0, sx: 1, sy: 1 };
  const eyes = tr.eyes ? stepAt(tr.eyes, t) : "open";
  d.eyeL = tr.eyeL ? stepAt(tr.eyeL, t) : eyes; d.eyeR = tr.eyeR ? stepAt(tr.eyeR, t) : eyes;
  d.gaze = tr.gaze ? stepAt(tr.gaze, t) : [0, 0];
  const both = tr.brows ? stepAt(tr.brows, t) : [0, 0];
  d.browL = tr.browL ? stepAt(tr.browL, t) : both; d.browR = tr.browR ? stepAt(tr.browR, t) : both;
  d.mouth = tr.mouth ? stepAt(tr.mouth, t) : "smile";
  d.glitch = tr.glitch ? stepAt(tr.glitch, t) : null;
  d.fx = {};
  for (const k of FX) if (tr[k] && tr[k].length) { const v = stepAt(tr[k], t); if (v !== null && v !== -1) d.fx[k] = v; }
  for (const k of ["x", "y", "rot", "sx", "sy"]) if (tr[k]) m[k] = smoothAt(tr[k], t);
  if (st.wave) for (const [k, [a, p]] of Object.entries(st.wave)) m[k] += a * Math.sin(2 * Math.PI * t / p);
  return { d, m };
}

const fmt = n => Math.round(n * 100) / 100;
const glyphCache = new Map();
function glyph(rows) {
  const key = rows.join("/");
  if (glyphCache.has(key)) return glyphCache.get(key);
  const h = rows.length, w = rows[0].length, pts = [];
  rows.forEach((row, y) => [...row].forEach((ch, x) => { if (ch === "#") pts.push([x - (w - 1) / 2, y - (h - 1) / 2]); }));
  const g = { pts, w, h };
  glyphCache.set(key, g);
  return g;
}
const EYE = EYE_STYLES.petit;
function palette(key) {
  const c = COLORS[key] || COLORS.peche;
  if (key === "charbon") return { body: c.body, deep: "#555963", eye: LIGHT, brow: "#D9DCE3", mouth: LIGHT, hl: null, outline: "#3A3D45", scan: "#FFFFFF", scanA: 0.14, fx: "#8E94A1" };
  if (key === "craie") return { body: "#FFFFFF", deep: c.deep, eye: INK, brow: INK, mouth: INK, hl: "#FFFFFF", outline: "#C9CDD5", scan: "#BFC3CC", scanA: 0.4, fx: "#8C929F" };
  return { body: c.body, deep: c.deep, eye: INK, brow: INK, mouth: INK, hl: "#FFFFFF", outline: null, scan: "#FFFFFF", scanA: 0.55, fx: c.deep };
}
function sq(x, y, s, fill, extra, r = 0.3) {
  return `<rect x="${fmt(x - s / 2)}" y="${fmt(y - s / 2)}" width="${fmt(s)}" height="${fmt(s)}" rx="${fmt(s * r)}" fill="${fill}"${extra || ""}/>`;
}
// Glyph squares that touch (« plein », 2026-10-29: u + 0.06), as the apps draw them.
function drawGlyph(rows, x, y, pitch, fill, extra) {
  const s = pitch + 0.06;
  return glyph(rows).pts.map(([gx, gy]) => sq(x + gx * pitch, y + gy * pitch, s, fill, extra, 0.12)).join("");
}
// The page's `mix` (each channel rounded).
function mix(a, b, t) {
  const pa = [1, 3, 5].map(i => parseInt(a.slice(i, i + 2), 16)), pb = [1, 3, 5].map(i => parseInt(b.slice(i, i + 2), 16));
  return "#" + pa.map((x, k) => Math.round(x + (pb[k] - x) * t).toString(16).padStart(2, "0")).join("");
}
// A body block « plein » (« comme sur notre icône »): a square of pitch `s` touching its neighbours, its seam a deeper
// tone of its own colour (the palette's outline when it has one), never the ground showing through.
function block(x, y, s, fill, deep, outline, extra = "", rot = 0) {
  const seam = outline || mix(fill, deep, 0.28), inner = s * 0.86;
  const turn = rot ? ` transform="rotate(${fmt(rot)} ${fmt(x)} ${fmt(y)})"` : "";
  return sq(x, y, s + 0.3, seam, extra + turn, 0.08 * s / (s + 0.3)) + sq(x, y, inner, fill, extra + turn, 0.12 / 0.86);
}
// One eye centred at (x, y): open and wide eyes one solid rounded square, closed a bar, the others pixel lines.
function eyeShape(shape, x, y, gx, gy, fill, extra = "") {
  const ax = x + gx * U;
  switch (shape) {
    case "wide": return sq(ax, y + gy * U, 2.6 * U, fill, extra, 0.25);
    case "closed": return `<rect x="${fmt(ax - U)}" y="${fmt(y)}" width="${fmt(2 * U)}" height="${fmt(0.6 * U)}" rx="${fmt(0.3 * U)}" fill="${fill}"${extra}/>`;
    case "happy": return drawGlyph([".#.", "#.#"], ax, y, U * 0.8, fill, extra);
    case "sleep": return drawGlyph(["#.#", ".#."], ax, y + 0.5 * U, U * 0.8, fill, extra);
    case "dash": return drawGlyph(["###"], ax, y + gy * U, U, fill, extra);
    default: return sq(ax, y + gy * U, glyph(EYE.open).w * U * 0.96, fill, extra, 0.22);
  }
}
const costumeOf = look => (look.costume && COSTUMES[look.costume]) || null;
function bodyOf(CO, bf) {
  const ob = CO && CO.body;
  let B = BODIES.standard;
  if (ob && (ob.rows || ob.frames)) {
    B = Object.assign({}, B, { rows: ob.frames ? ob.frames[bf || 0] : ob.rows });
    for (const k of ["eyeRow", "eyeCols", "mouthRow"]) if (ob[k] !== undefined) B[k] = ob[k];
  }
  return B;
}
function paletteOf(look, CO) {
  const ob = CO && CO.body;
  const base = palette((ob && ob.color) || look.color);
  return ob && ob.colors ? Object.assign({}, base, ob.colors) : base;
}
function geom(B) {
  const cols = B.rows[0].length, rows = B.rows.length;
  const left = CX - cols * P / 2, top = BOTTOM - rows * P, right = CX + cols * P / 2;
  const cx = c => left + c * P + P / 2, cy = r => top + r * P + P / 2;
  return { cols, rows, left, top, right, cx, cy, eyeY: cy(B.eyeRow), mouthY: cy(B.mouthRow) };
}
// The mouth never thinner than MOUTH_MIN screen px per face pixel when drawn small (as in the apps).
const mouthScale = ppu => ppu ? Math.min(MOUTH_MAX, Math.max(1, MOUTH_MIN / (U * ppu))) : 1;
function anchorPt(at, G, mouth, mk) {
  switch (at) {
    case "bottom": return [CX, BOTTOM];
    case "eyes": return [CX, G.eyeY];
    case "mouth": return [CX, G.mouthY];
    case "mouthBottom": {
      const M = MOUTHS[mouth] || MOUTHS.smile, mu = U * (mk || 1);
      return [CX + (M.dx || 0) * mu, G.mouthY + (M.dy || 0) * mu + M.g.length * mu / 2];
    }
    default: return [CX, G.top];
  }
}
function frameState(CO, clock) {
  if (!CO) return null;
  const f = i => Math.floor(clock / i.period) % i.frames.length;
  return { b: CO.body && CO.body.frames ? f(CO.body) : 0, i: CO.items.map(it => it.frames ? f(it) : 0) };
}
function drawItem(it, idx, G, d, CO, fr, mk) {
  let rows = it.frames ? it.frames[fr.i[idx]] : it.rows, rot = it.rot || 0, dx = it.dx, dy = it.dy;
  const w = it.when && (it.when[d.eyeL] || it.when[d.eyeR]);
  if (w) { rows = w.rows || rows; if (w.rot !== undefined) rot = w.rot; if (w.dx !== undefined) dx = w.dx; if (w.dy !== undefined) dy = w.dy; }
  const pitch = U * (it.scale || 1), [ax, ay] = anchorPt(it.at, G, d.mouth, mk);
  const ox = ax + dx * U, oy = ay + dy * U;
  const pv = it.pivot || [rows[0].length / 2, rows.length];
  const pvx = ox + pv[0] * pitch, pvy = oy + pv[1] * pitch;
  let s = "";
  rows.forEach((row, j) => [...row].forEach((ch, i) => {
    if (ch === "." || ch === " ") return;
    const p = CO.palette[ch];
    if (!p) return;
    const [fill, stroke] = Array.isArray(p) ? p : [p];
    s += sq(ox + (i + 0.5) * pitch, oy + (j + 0.5) * pitch, pitch * 0.94, fill, stroke ? ` stroke="${stroke}" stroke-width="${fmt(pitch * 0.14)}"` : "");
  }));
  if (rot) s = `<g transform="rotate(${rot} ${fmt(pvx)} ${fmt(pvy)})">${s}</g>`;
  return `<g data-it="${idx}" data-pv="${fmt(pvx)} ${fmt(pvy)}"${it.alpha ? ` opacity="${it.alpha}"` : ""}>${s}</g>`;
}
function itemDyn(it, clock, m) {
  let x = 0, y = 0, rot = 0, op = 1;
  if (it.wave) for (const [k, [a, p, ph]] of Object.entries(it.wave)) {
    const v = a * Math.sin(2 * Math.PI * (clock / p + (ph || 0)));
    if (k === "x") x += v; else if (k === "y") y += v; else rot += v;
  }
  if (it.lift) y += it.lift * m.y;
  if (it.blink) { const [p, on, ph] = it.blink; op = ((clock / p + (ph || 0)) % 1) < on ? 1 : 0; }
  return { x, y, rot, op };
}

/** The figure's squares for a look ({color, costume, noHL}) and a pose, as SVG markup (no <svg>). */
function drawPixel(look, pose, fr, ppu) {
  const CO = costumeOf(look), B = bodyOf(CO, fr ? fr.b : 0), C = paletteOf(look, CO), d = pose.d, fx = d.fx;
  const G = geom(B), { left, top, right, cx, cy } = G;
  const o = [], mk = mouthScale(ppu);
  const stroke = "";
  const cell = (x, y, fill, extra) => block(x, y, P, fill, C.deep, C.outline, extra || "");
  const layer = name => { if (CO) CO.items.forEach((it, i) => { if ((it.layer || "top") === name) o.push(drawItem(it, i, G, d, CO, fr, mk)); }); };
  const g = d.glitch;
  layer("back");
  const bodyCells = [];
  B.rows.forEach((row, r) => [...row].forEach((ch, c) => {
    if (ch === ".") return;
    const p = ch === "#" ? C.body : (CO.palette[ch] || C.body), fill = Array.isArray(p) ? p[0] : p;
    if (g && g.r === r && g.c === c) { bodyCells.push(cell(cx(c), cy(r), fill, ` fill-opacity="0.35"`)); return; }
    bodyCells.push(cell(cx(c), cy(r), fill, stroke));
    if (fx.scan === r) bodyCells.push(sq(cx(c), cy(r), P * 0.86, C.scan, ` fill-opacity="${C.scanA}"`, 0.12 / 0.86));
  }));
  const op = CO && CO.body && CO.body.opacity;
  o.push(op ? `<g opacity="${op}">${bodyCells.join("")}</g>` : bodyCells.join(""));
  if (g) o.push(cell(cx(g.c) + g.dx * U, cy(g.r) + g.dy * U, C.deep));
  layer("body");
  // Eyes: an open (or wide) eye is one solid rounded square, a closed one a bar; happy, sleeping and busy eyes are lines.
  const eyeY = G.eyeY, openG = glyph(EYE.open), face = U + 0.06;
  [0, 1].forEach(side => {
    const shape = side ? d.eyeR : d.eyeL, flat = shape === "closed" || shape === "happy" || shape === "sleep";
    o.push(eyeShape(shape, cx(B.eyeCols[side]), eyeY, d.gaze[0], flat ? 0 : d.gaze[1], C.eye));
  });
  layer("face");
  // Brows show when they move.
  const bl = d.browL, br = d.browR;
  if (bl[0] || bl[1] || br[0] || br[1]) {
    const base = -(openG.h - 1) / 2 - 2 + Math.min(0, d.gaze[1]);
    [[0, bl], [1, br]].forEach(([side, [dy, tilt]]) => {
      const ax = cx(B.eyeCols[side]);
      [-1, 0, 1].forEach(i => {
        const outer = side === 0 ? i === -1 : i === 1, inner = side === 0 ? i === 1 : i === -1;
        const lift = tilt > 0 && outer ? tilt : tilt < 0 && inner ? -tilt : 0;
        o.push(sq(ax + i * U, eyeY + (base + dy - lift) * U, face, C.brow));
      });
    });
  }
  const M = MOUTHS[d.mouth] || MOUTHS.smile;
  o.push(drawGlyph(M.g, CX + (M.dx || 0) * U * mk, G.mouthY + (M.dy || 0) * U * mk, U * mk, C.mouth));
  layer("top");
  // Effects
  if (fx.dots) { const spots = [[-0.5, -1.6, 0.85], [1.4, -3.6, 1.1], [3.7, -5.8, 1.4]]; for (let i = 0; i < fx.dots; i++) { const [x, y, s] = spots[i]; o.push(sq(right + x * U, top + y * U, s * U, C.fx)); } }
  if (fx.sound) for (let j = 0; j < fx.sound; j++) { const n = 2 + j, x = left - (1.8 + 1.5 * j) * U; for (let k = 0; k < n; k++) o.push(sq(x, eyeY + (k - (n - 1) / 2) * U * 0.95, U * 0.8, C.fx)); }
  if (fx.sparks !== undefined) for (const [x, y, big] of SPARK_FRAMES[fx.sparks % SPARK_FRAMES.length]) {
    const px = CX + x * U, py = top + y * U, s = U * 0.8;
    o.push(sq(px, py, s, "#E7B43C"));
    if (big) for (const [ox, oy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) o.push(sq(px + ox * s, py + oy * s, s * 0.8, "#E7B43C", ` fill-opacity="0.75"`));
  }
  if (fx.heart !== undefined && fx.heart >= 0) o.push(drawGlyph(GLYPHS.heart, right + 1.6 * U, top - (2 + fx.heart) * U, U * 0.62, "#EC6F8F", fx.heart >= 4 ? ` fill-opacity="0.5"` : ""));
  if (fx.z !== undefined) for (let i = 0; i < 3; i++) { const k = (fx.z + i * 3) % 8; if (k >= 7) continue; o.push(drawGlyph(GLYPHS.z, right - 0.5 * U + k * 0.7 * U, top - (0.5 + k * 1.3) * U, U * (0.5 + k * 0.07), C.fx, k >= 5 ? ` fill-opacity="0.5"` : "")); }
  if (fx.bang) o.push(drawGlyph(GLYPHS.bang, right + 1.4 * U, top - 2.6 * U, U * 0.95, C.fx));
  return o.join("");
}

// -- The animations of 2026-10-29 (design page « Animations des Bips », internal/design/bip-animations-2026-10-29.html)
// Each a pure function of time, from the page's formulas (its ANIMS and FUN), the same as the apps' BipMotions.swift and
// BipMotions.kt. A frame lists what to draw: blocks {k:"cell", i, j, dx, dy, rot, s, op, fill?}, the face {k:"face", dx,
// dy, op, gaze, eyes, mouth}, glyphs, rounded rects and polygons. site/check-motions.mjs compares the frames to
// scripts/pixel/motion-frames.json (the page's own, at 0.4, 1.2 and 2.0 s).

/** The thinking animations, alternating (« today » is the think state's own); the small ones read at 22–56 px. */
export const THINKING = ["today", "chantier", "vague", "construit", "bulle", "puzzle", "jongle", "scanner", "relief", "empile"];
export const THINKING_SMALL = ["vague", "chantier", "jongle", "scanner", "construit"];
/** The moments played once: their length (s). */
export const MOMENTS = { done: 2.2, oops: 3.6, wave: 1.8, born: 3.8, sent: 2.4 };
const MOTION_STILL = { construit: 2.4, empile: 2.6, done: 0.4, oops: 0.3, born: 3.0, sent: 1.0 };
const M_LEFT = 60 - 3 * P, M_TOP = 106 - 6 * P, CS = P - GAP, GOLD = "#FFC93C", CHEEK = "#F58BA6";
const CELLS = [];
BODIES.standard.rows.forEach((r, j) => [...r].forEach((c, i) => { if (c === "#") CELLS.push([i, j]); }));
const CELLSET = new Set(CELLS.map(c => c.join()));
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const lerp = (a, b, t) => a + (b - a) * t;
const easeOut = t => 1 - Math.pow(1 - clamp(t, 0, 1), 3);
const easeInOut = t => { t = clamp(t, 0, 1); return t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2; };
const back = t => { t = clamp(t, 0, 1); const c1 = 1.7, c3 = c1 + 1; return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2); };
function rng(seed) { let s = seed >>> 0; return () => ((s = (s * 1664525 + 1013904223) >>> 0) / 4294967296); }
function plan(seed) {
  const r = rng(seed), order = CELLS.map((c, n) => [r(), n]).sort((a, b) => a[0] - b[0]).map(x => x[1]);
  const dirs = CELLS.map(() => { const a = r() * Math.PI * 2, d = 46 + r() * 26; return [Math.cos(a) * d, Math.sin(a) * d - 10, (r() - .5) * 260]; });
  return { dirs, rank: order.reduce((m, n, k) => (m[n] = k, m), {}) };
}
const cellX = i => M_LEFT + i * P + GAP / 2, cellY = j => M_TOP + j * P + GAP / 2;
const P_BUILD = plan(7), P_NEW = plan(21);
const SWAPS = [[[1, 1], [4, 3]], [[2, 4], [5, 2]], [[0, 3], [3, 1]], [[4, 0], [1, 5]], [[5, 4], [2, 1]]];
const cellM = (out, i, j, o = {}) => { const it = { k: "cell", i, j, dx: o.dx || 0, dy: o.dy || 0, rot: o.rot || 0, s: o.s ?? 1, op: o.op ?? 1 }; if (o.fill) it.fill = o.fill; out.push(it); };
const bodyM = (out, o = {}) => CELLS.forEach(([i, j], n) => cellM(out, i, j, o.each ? o.each(i, j, n) : o));
const faceM = (out, o = {}) => out.push({ k: "face", dx: o.dx || 0, dy: o.dy || 0, op: o.op ?? 1, gaze: o.gaze || [0, 0], eyes: o.eyes || "open", mouth: o.mouth || "smile" });
const glyphM = (out, rows, cx, cy, u, fill, op = 1) => out.push({ k: "glyph", rows, cx, cy, u, fill, op });
const rectM = (out, x, y, w, h, rx, fill, o = {}) => { const it = { k: "rect", x, y, w, h, rx, fill, op: o.op ?? 1, rot: o.rot || 0 }; if (o.ox !== undefined) { it.ox = o.ox; it.oy = o.oy; } if (o.stroke) it.stroke = o.stroke; out.push(it); };
const MOTIONS = {
  chantier(t, out) {
    const fly = [[4, 0, 0], [5, 1, 1.1]], D = 2.2;
    CELLS.forEach(([i, j]) => { const fl = fly.find(f => f[0] === i && f[1] === j);
      if (!fl) return cellM(out, i, j);
      const k = ((t + fl[2]) % D) / D, up = k < .5 ? easeOut(k / .5) : 1 - easeInOut((k - .5) / .5);
      cellM(out, i, j, { dx: up * (10 + 4 * fl[2]), dy: -up * (16 + 6 * fl[2]), rot: up * (fl[2] ? -40 : 35), s: 1 - up * .18 }); });
    faceM(out, { gaze: [1, -1], mouth: "hmm" });
  },
  vague(t, out) {
    const lift = i => Math.max(0, Math.sin(2 * Math.PI * (t / 1.3 - i / 7))) * 3.4;
    bodyM(out, { each: i => ({ dy: -lift(i) }) }); faceM(out, { gaze: [Math.sin(t * 2.4) > 0 ? 1 : -1, -1], mouth: "hmm", dy: -(lift(1) + lift(4)) / 2 });
  },
  construit(t, out) {
    t %= 4.2;
    CELLS.forEach(([i, j], n) => { const r = P_BUILD.rank[n], [ddx, ddy, rr] = P_BUILD.dirs[n];
      const tin = clamp((t - r * 0.045) / .5, 0, 1), tout = clamp((t - 3.0 - (33 - r) * 0.025) / .45, 0, 1);
      const a = back(tin) * (1 - easeInOut(tout)); if (a <= 0.001) return;
      cellM(out, i, j, { dx: ddx * (1 - a), dy: ddy * (1 - a), rot: rr * (1 - a), s: .55 + .45 * a, op: clamp(a * 1.4, 0, 1) }); });
    const fop = clamp((t - 1.7) / .3, 0, 1) * (1 - clamp((t - 2.95) / .2, 0, 1));
    if (fop > 0) faceM(out, { op: fop, gaze: [0, -1], mouth: "smile" });
  },
  bulle(t, out) {
    bodyM(out); faceM(out, { gaze: [1, -1], mouth: "hmm" });
    [[90, 27, 3.2], [96, 20, 4.2]].forEach(([x, y, s]) => rectM(out, x, y, s, s, s * .3, "#C9CED9"));
    rectM(out, 94, 0, 26, 14, 4, "#FFFFFF", { op: .96 }); rectM(out, 94, 0, 26, 14, 4, "none", { stroke: "#C9CED9" });
    for (let k = 0; k < 3; k++) { const on = Math.floor((t * 2.6) % 4) > k; rectM(out, 94 + 5 + k * 6.5, 5, 3.6, 3.6, 1, on ? "#6B7180" : "#D9DCE3"); }
  },
  puzzle(t, out, body) {
    const D = .9, n = Math.floor(t / D) % SWAPS.length, k = (t % D) / D, e = easeInOut(clamp(k / .7, 0, 1));
    const [[ai, aj], [bi, bj]] = SWAPS[n];
    CELLS.forEach(([i, j]) => { if (!(i === ai && j === aj) && !(i === bi && j === bj)) cellM(out, i, j); });
    const hop = Math.sin(Math.PI * e) * 14;
    cellM(out, ai, aj, { dx: (bi - ai) * P * e, dy: (bj - aj) * P * e - hop, rot: e * 180, fill: mix(body, "#FFFFFF", .25) });
    cellM(out, bi, bj, { dx: (ai - bi) * P * e, dy: (aj - bj) * P * e - hop * .6, rot: -e * 180 });
    faceM(out, { gaze: [((ai + bi) / 2 - 2.5) > 0 ? 1 : -1, -1], mouth: "hmm" });
  },
  jongle(t, out) {
    const D = 1.1, k = (t % D) / D, h = 4 * k * (1 - k), y = 22 - h * 30, x = 60 + Math.sin(t * 2.9) * 10, land = k < .08 || k > .92 ? 1.2 : 0;
    bodyM(out, { each: (i, j) => ({ dy: j <= 1 && Math.abs(cellX(i) + CS / 2 - x) < 13 ? land : 0 }) });
    faceM(out, { gaze: [x > 60 ? 1 : -1, h > .5 ? -1 : 0], mouth: "smile" });
    rectM(out, x - 4, y - 4, 8, 8, 2, GOLD, { rot: t * 300 });
  },
  scanner(t, out, body) {
    const D = 1.8, bx = lerp(-12, 132, (t % D) / D);
    bodyM(out, { each: (i, j) => { const cx = cellX(i) + CS / 2 + (j - 2.5) * 4; return { fill: mix(body, "#FFFFFF", clamp(1 - Math.abs(cx - bx) / 16, 0, 1) * .7) }; } });
    faceM(out, { gaze: [bx < 60 ? -1 : 1, 0], mouth: "hmm" });
  },
  relief(t, out, body, deep) {
    const top = mix(body, "#FFFFFF", .4), side = mix(body, deep, .78);
    const k = (i, j) => 2 + 3.2 * (0.5 + 0.5 * Math.sin(2 * Math.PI * (t / 1.6 - (i + j) / 10)));
    CELLS.slice().sort((a, b) => (a[0] - b[0]) || (b[1] - a[1])).forEach(([i, j]) => { const x = cellX(i), y = cellY(j), kk = k(i, j);
      if (!CELLSET.has([i + 1, j].join())) out.push({ k: "poly", pts: [[x + CS, y], [x + CS + kk, y - kk], [x + CS + kk, y + CS - kk], [x + CS, y + CS]], fill: side, op: 1 });
      if (!CELLSET.has([i, j - 1].join())) out.push({ k: "poly", pts: [[x, y], [x + kk, y - kk], [x + CS + kk, y - kk], [x + CS, y]], fill: top, op: 1 }); });
    CELLS.forEach(([i, j]) => rectM(out, cellX(i) - .2, cellY(j) - .2, CS + .4, CS + .4, 1.2, body));
    faceM(out, { gaze: [1, -1], mouth: "hmm" });
  },
  empile(t, out) {
    t %= 3.6;
    CELLS.forEach(([i, j]) => { const start = (5 - j) * .22 + (i % 3) * .05, k = clamp((t - start) / .45, 0, 1), fall = 1 - k * k;
      const bounce = k >= 1 ? Math.max(0, Math.sin((t - start - .45) * 14) * 1.2 * Math.exp(-(t - start - .45) * 6)) : 0;
      if (k <= 0) return; cellM(out, i, j, { dy: -fall * 90 - bounce, op: 1 - clamp((t - 3.05) / .4, 0, 1) }); });
    if (t > 1.6) faceM(out, { op: clamp((t - 1.6) / .3, 0, 1) * (1 - clamp((t - 3.05) / .4, 0, 1)), gaze: [0, -1], mouth: "smile" });
  },
  done(t, out) {
    t %= 2.2; const jump = t < .55 ? Math.sin(Math.PI * t / .55) * 14 : 0;
    bodyM(out, { dy: -jump }); faceM(out, { dy: -jump, eyes: "happy", mouth: "grin" });
    const r = rng(5), cols = [GOLD, "#FF7A59", "#8FA8FF", "#5BD46A", CHEEK];
    for (let k = 0; k < 16; k++) { const a = r() * Math.PI * 2, v = 30 + r() * 40, tt = clamp(t - .2, 0, 2), x = 60 + Math.cos(a) * v * tt, y = 50 + Math.sin(a) * v * tt - 40 * tt + 45 * tt * tt;
      if (t > .2) rectM(out, x, y, 4, 4, 1, cols[k % 5], { op: 1 - clamp((t - 1.4) / .6, 0, 1), rot: tt * 400 }); }
  },
  oops(t, out) {
    t %= 3.6; const r = rng(9);
    CELLS.forEach(([i, j], n) => { const jitter = (r() - .5) * 30, floor = 108 - (j % 2) * 9 - (i % 3) * 3, y0 = cellY(j);
      let dy = 0, dx = 0, rot = 0;
      if (t > .5 && t < 1.6) { const k = clamp((t - .5 - n * .008) / .5, 0, 1); dy = (floor - CS - y0) * k * k; dx = jitter * k; rot = jitter * 3 * k; }
      else if (t >= 1.6 && t < 2.4) { dy = floor - CS - y0; dx = jitter; rot = jitter * 3; }
      else if (t >= 2.4) { const k = 1 - back(clamp((t - 2.4 - n * .01) / .6, 0, 1)); dy = (floor - CS - y0) * k; dx = jitter * k; rot = jitter * 3 * k; }
      cellM(out, i, j, { dx, dy, rot }); });
    if (t < .5) faceM(out, { eyes: "wide", mouth: "o" }); else if (t > 3) faceM(out, { mouth: "smile", op: clamp((t - 3) / .2, 0, 1) });
  },
  sleep(t, out) {
    const br = Math.sin(t * 2) * .8; bodyM(out, { dy: br }); faceM(out, { eyes: "closed", mouth: "sleep", dy: br });
    for (let k = 0; k < 2; k++) { const kk = ((t / 2.4) + k * .5) % 1; glyphM(out, ["####", "..#.", ".#..", "####"], 94 + kk * 12, 26 - kk * 26, 1.6 + kk * 1.4, "#8C93A3", 1 - kk); }
  },
  wave(t, out, body) {
    bodyM(out); faceM(out, { eyes: "happy", mouth: "smile" });
    const a = Math.sin(t * 7) * 22 - 10, sx = M_LEFT + 6 * P - 1, sy = M_TOP + 3.2 * P;
    [[0, 0], [1, 0], [1, -1], [1, -2], [1, -3]].forEach(([cx, cy]) => rectM(out, sx + cx * 4.6, sy + cy * 4.6, 4.6, 4.6, 1.2, body, { rot: a, ox: sx, oy: sy + 2.3 }));
  },
  listen(t, out, body, deep) {
    bodyM(out); faceM(out, { eyes: "wide", mouth: "o", gaze: [1, 0] });
    for (let k = 0; k < 4; k++) { const h = 4 + Math.abs(Math.sin(t * (5 + k * 1.7) + k)) * 18; rectM(out, 104 + k * 5.2, 60 - h / 2, 3.6, h, 1.4, deep, { op: .85 }); }
  },
  born(t, out) {
    t %= 3.8; const last = [[4, 0], [5, 1]];
    CELLS.forEach(([i, j], n) => { const isLast = last.findIndex(l => l[0] === i && l[1] === j), r = P_NEW.rank[n], [ddx, ddy, rr] = P_NEW.dirs[n];
      const start = isLast < 0 ? r * .035 : 1.35 + isLast * .3, tin = clamp((t - start) / (isLast < 0 ? .45 : .55), 0, 1), a = back(tin);
      if (tin <= 0) return; const fx = isLast < 0 ? ddx : 26, fy = isLast < 0 ? ddy : -34;
      cellM(out, i, j, { dx: fx * (1 - a), dy: fy * (1 - a), rot: (isLast < 0 ? rr : 200) * (1 - a), s: .55 + .45 * a, op: clamp(tin * 1.6, 0, 1) }); });
    if (t > 1.0) faceM(out, { op: clamp((t - 1.0) / .3, 0, 1), gaze: t < 2.2 ? [1, -1] : [0, 0], mouth: t > 2.2 ? "grin" : "smile", eyes: t > 2.3 && t < 3.2 ? "happy" : "open" });
    if (t > 2.15 && t < 2.9) { const k = (t - 2.15) / .75; for (let q = 0; q < 4; q++) { const a = q * Math.PI / 2 + .6, d = 8 + k * 12; rectM(out, 104 + Math.cos(a) * d - 2, 22 + Math.sin(a) * d - 2, 4, 4, 1, GOLD, { op: 1 - k }); } }
  },
  sent(t, out) {
    t %= 2.4; bodyM(out); const k = clamp((t - .3) / 1.4, 0, 1);
    faceM(out, { gaze: k > 0 ? [1, -1] : [1, 0], mouth: k > 0 ? "grin" : "smile" });
    const x = 98 + k * 40, y = 50 - k * 70, op = 1 - clamp((k - .7) / .3, 0, 1), plane = ["#...", "###.", "####", "###.", "#..."];
    glyphM(out, plane, x, y, 2.4, "#FFFFFF", op); glyphM(out, plane, x + .4, y + .4, 2.4, "#8C93A3", op * .35);
  },
};
/** A motion's frame `t` seconds in, for a body colour and its deep shade (null for an unknown id, « today »). */
export function motionFrame(id, t, body, deep) {
  const f = MOTIONS[id];
  if (!f) return null;
  const out = [];
  f(Math.max(0, t), out, body, deep);
  return out;
}
/** A thinking episode's animation: the `episode`-th of the rotation (small Bips: the ones readable small). */
export const thinkingVariant = (episode, small) => { const l = small ? THINKING_SMALL : THINKING; return l[((episode % l.length) + l.length) % l.length]; };
/** How long a variant plays before the next one takes over within the same episode (Alex, 2026-10-29): whole loops of
 *  its own, about 7–9 s, so it changes where its loop starts again. */
export const THINKING_TURN = { today: 7, chantier: 2.2 * 4, vague: 1.3 * 6, construit: 4.2 * 2, bulle: 4 / 2.6 * 5, puzzle: 0.9 * 8,
  jongle: 1.1 * 7, scanner: 1.8 * 4, relief: 1.6 * 5, empile: 3.6 * 2 };
/** What an episode plays `elapsed` seconds in: its variant, then the next ones in turn, and the time within it. */
export function thinkingNow(episode, elapsed, small) {
  const l = small ? THINKING_SMALL : THINKING, round = l.reduce((a, v) => a + THINKING_TURN[v], 0);
  let t = Math.max(0, elapsed) % round;
  for (let k = episode; ; k++) {
    const v = thinkingVariant(k, small);
    if (t < THINKING_TURN[v]) return [v, t];
    t -= THINKING_TURN[v];
  }
}

/** A motion frame as SVG markup, with the look's colours and costume (only on a whole Bip). */
function drawMotion(look, items, fr, ppu, pose) {
  const CO = costumeOf(look), B = bodyOf(CO, fr ? fr.b : 0), C = paletteOf(look, CO), G = geom(B), mk = mouthScale(ppu);
  const cells = items.filter(it => it.k === "cell");
  const dressed = CO && cells.length >= CELLS.length && Math.min(...cells.map(c => c.op)) >= 0.6;
  const layer = name => dressed ? CO.items.map((it, i) => (it.layer || "top") === name ? drawItem(it, i, G, pose.d, CO, fr, mk) : "").join("") : "";
  const op = v => v < 1 ? ` opacity="${fmt(v)}"` : "";
  let s = items.filter(it => it.k === "poly").map(p => `<polygon points="${p.pts.map(q => fmt(q[0]) + "," + fmt(q[1])).join(" ")}" fill="${p.fill}"/>`).join("");
  s += layer("back");
  for (const c of cells) s += block(G.cx(c.i) + c.dx, G.cy(c.j) + c.dy, P * c.s, c.fill || C.body, C.deep, C.outline, op(c.op), c.rot);
  s += layer("body");
  for (const f of items) if (f.k === "face") {
    const extra = op(f.op), ey = G.eyeY + f.dy;
    [0, 1].forEach(side => { const flat = f.eyes === "closed" || f.eyes === "happy";
      s += eyeShape(f.eyes, G.cx(B.eyeCols[side]) + f.dx, ey, f.gaze[0] * 0.8, flat ? 0 : f.gaze[1] * 0.8, C.eye, extra); });
    const M = MOUTHS[f.mouth === "sleep" ? "sleepm" : f.mouth] || MOUTHS.smile;
    s += drawGlyph(M.g, CX + f.dx + (M.dx || 0) * U * mk, G.mouthY + f.dy + (M.dy || 0) * U * mk, U * mk, C.mouth, extra);
  }
  s += layer("face") + layer("top");
  for (const it of items) {
    if (it.k === "rect") {
      const ox = it.ox ?? it.x + it.w / 2, oy = it.oy ?? it.y + it.h / 2;
      s += `<rect x="${fmt(it.x)}" y="${fmt(it.y)}" width="${fmt(it.w)}" height="${fmt(it.h)}" rx="${fmt(it.rx)}" fill="${it.fill}"${op(it.op)}`
        + (it.stroke ? ` stroke="${it.stroke}" stroke-width="1"` : "") + (it.rot ? ` transform="rotate(${fmt(it.rot)} ${fmt(ox)} ${fmt(oy)})"` : "") + "/>";
    } else if (it.k === "glyph") s += drawGlyph(it.rows, it.cx, it.cy, it.u, it.fill, op(it.op));
  }
  return s;
}
const memo = {};
/** The next thinking episode of `key` (a category): counted and kept in this browser when it can, so the animations
 *  vary from one visit to the next. */
export function nextEpisode(key) {
  let n = (memo[key] || 0) + 1;
  try { n = (+localStorage.getItem("bips.thinking." + key) || 0) + 1; localStorage.setItem("bips.thinking." + key, String(n)); } catch { /* none */ }
  memo[key] = n;
  return n;
}
// The costumes that reshape the body (the pumpkin, the vampire, the ghost) keep the states' own animations.
const reshapes = costume => { const ob = costume && COSTUMES[costume] && COSTUMES[costume].body; return !!(ob && (ob.rows || ob.frames)); };

// The frame grows upward and sideways to hold a costume (at most 152), the ground stays at 120.
const MAXBOX = 152, boxCache = new Map();
function frameOf(costume) {
  const CO = costume && COSTUMES[costume], k = costume || "";
  if (boxCache.has(k)) return boxCache.get(k);
  let size = 120;
  if (CO) {
    const G = geom(bodyOf(CO, 0));
    let minX = G.left, maxX = G.right, minY = G.top;
    CO.items.forEach(it => {
      const rows = it.rows || it.frames[0], pitch = U * (it.scale || 1), [ax, ay] = anchorPt(it.at, G, "smile");
      minX = Math.min(minX, ax + it.dx * U); maxX = Math.max(maxX, ax + it.dx * U + rows[0].length * pitch); minY = Math.min(minY, ay + it.dy * U);
    });
    const half = Math.max(60, 60 - minX + 3, maxX + 3 - 60);
    size = Math.min(MAXBOX, Math.max(120, 120 - (minY - 4), 2 * half));
  }
  boxCache.set(k, size);
  return size;
}
const viewBox = size => `${fmt(60 - size / 2)} ${fmt(120 - size)} ${fmt(size)} ${fmt(size)}`;

/** A pose: the state (or reaction) at its time, the costume's own motion, its frames. */
function poseOf(st, t, costume, clock, still) {
  const tt = still ? st.still : t;
  const pose = evalState(st, tt);
  const CO = costume && COSTUMES[costume];
  if (CO && CO.wave) for (const [k, [a, p]] of Object.entries(CO.wave)) pose.m[k] += a * Math.sin(2 * Math.PI * clock / p);
  return pose;
}
const figTransform = m => `translate(${fmt(m.x)} ${fmt(m.y)}) translate(60 ${BOTTOM}) rotate(${fmt(m.rot)}) scale(${fmt(m.sx)} ${fmt(m.sy)}) translate(-60 ${-BOTTOM})`;
const SHADOW = `<ellipse cx="60" cy="${BOTTOM + 3}" rx="30" ry="4" fill="${INK}" fill-opacity="0.1"/>`;

/** A whole Bip as SVG markup: { color | category, mood or state, t (seconds), costume, size (px, for the mouth) }. */
/** `motion`: one of the animations of 2026-10-29 (a thinking variant from `thinkingVariant`, a moment of MOMENTS),
 *  `mt` seconds in; asleep and listening Bips take the page's « Dodo » and « Écoute » by themselves. */
export function pixelSVG({ category = "daily", color, mood = "idle", t = 0, costume = null, size = 48, still = REDUCED, shadow = true,
  motion = null, mt = t } = {}) {
  const c = CATEGORIES[category] || CATEGORIES.daily, st = stateOf(mood);
  const clock = still ? 0.6 : t, loop = ((t % st.dur) + st.dur) % st.dur;
  const CO = costume && COSTUMES[costume];
  const box = frameOf(costume), look = { color: color || c.color, costume };
  const fr = frameState(CO, clock);
  const m = reshapes(costume) ? null : motion && MOTIONS[motion] ? motion : st === STATES.sleep ? "sleep" : st === STATES.listen ? "listen" : null;
  if (m) {
    const C = paletteOf(look, CO), pose = poseOf(STATES.idle, 0, costume, clock, true);
    pose.m = { x: 0, y: 0, rot: 0, sx: 1, sy: 1 };
    const inner = drawMotion(look, motionFrame(m, still ? (MOTION_STILL[m] ?? 1.2) : mt, C.body, C.deep), fr, size / box, pose);
    return `<svg viewBox="${viewBox(box)}" aria-hidden="true" focusable="false">${shadow ? SHADOW : ""}<g>${inner}</g></svg>`;
  }
  const pose = poseOf(st, loop, costume, clock, still);
  let inner = drawPixel(look, pose, fr, size / box);
  if (CO) inner = inner.replace(/<g data-it="(\d+)" data-pv="([\d.\- ]+)"/g, (all, i, pv) => {
    const it = CO.items[+i];
    if (!it.wave && !it.lift && !it.blink) return all;
    const q = itemDyn(it, clock, pose.m);
    return `${all} transform="translate(${fmt(q.x)} ${fmt(q.y)}) rotate(${fmt(q.rot)} ${pv})"${q.op ? "" : ` opacity="0"`}`;
  });
  return `<svg viewBox="${viewBox(box)}" aria-hidden="true" focusable="false">${shadow ? SHADOW : ""}`
    + `<g transform="${figTransform(pose.m)}">${inner}</g></svg>`;
}

/** A still Bip (avatars, small marks): the state's still moment. */
export function staticBip(category, mood = "happy", costume = null, size = 48) {
  return pixelSVG({ category, mood, costume, size, still: true });
}

/** The old call (site.js' working line): a category's Bip in a mood at time t. */
export function renderBip(c, s, t) {
  return pixelSVG({ category: c.id, mood: s.mood, t, still: REDUCED });
}

// -- The app icon « Relief classique, rose » (scripts/pixel/icon.py) ---------------------------------------------------

// <app-icon> written by scripts/pixel/icon.py: do not edit by hand
const APP_ICON = "<defs><radialGradient id=\"ICg\" cx=\"60\" cy=\"50.4\" r=\"90\" gradientUnits=\"userSpaceOnUse\"><stop offset=\"0\" stop-color=\"#25262D\"/><stop offset=\"1\" stop-color=\"#0C0C0F\"/></radialGradient><filter id=\"ICsoft\" x=\"-40%\" y=\"-40%\" width=\"180%\" height=\"180%\"><feGaussianBlur stdDeviation=\"2.2\"/></filter><clipPath id=\"ICc\"><rect width=\"120\" height=\"120\" rx=\"ROUND\"/></clipPath></defs><g clip-path=\"url(#ICc)\"><rect width=\"120\" height=\"120\" fill=\"url(#ICg)\"/><ellipse cx=\"60.30\" cy=\"113.50\" rx=\"39.00\" ry=\"4.29\" fill=\"#000\" opacity=\".7\" filter=\"url(#ICsoft)\"/><polygon points=\"22.00,48.10 26.60,43.40 38.60,43.40 34.00,48.10\" fill=\"#F7D3E2\"/><polygon points=\"34.00,36.10 38.60,31.40 50.60,31.40 46.00,36.10\" fill=\"#F7D3E2\"/><polygon points=\"46.00,36.10 50.60,31.40 62.60,31.40 58.00,36.10\" fill=\"#F7D3E2\"/><polygon points=\"69.90,36.00 74.60,31.40 74.60,43.40 69.90,48.00\" fill=\"#CE82A2\"/><polygon points=\"58.00,36.10 62.60,31.40 74.60,31.40 70.00,36.10\" fill=\"#F7D3E2\"/><polygon points=\"81.90,96.00 86.60,91.40 86.60,103.40 81.90,108.00\" fill=\"#CE82A2\"/><polygon points=\"81.90,48.00 86.60,43.40 86.60,55.40 81.90,60.00\" fill=\"#CE82A2\"/><polygon points=\"70.00,48.10 74.60,43.40 86.60,43.40 82.00,48.10\" fill=\"#F7D3E2\"/><polygon points=\"93.90,84.00 98.60,79.40 98.60,91.40 93.90,96.00\" fill=\"#CE82A2\"/><polygon points=\"93.90,72.00 98.60,67.40 98.60,79.40 93.90,84.00\" fill=\"#CE82A2\"/><polygon points=\"93.90,60.00 98.60,55.40 98.60,67.40 93.90,72.00\" fill=\"#CE82A2\"/><polygon points=\"82.00,60.10 86.60,55.40 98.60,55.40 94.00,60.10\" fill=\"#F7D3E2\"/><rect x=\"21.80\" y=\"47.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"22.00,48.00 34.00,48.00 32.92,49.08 23.08,49.08 23.08,58.92 22.00,60.00\" fill=\"#F6CBDD\"/><polygon points=\"34.00,48.00 34.00,60.00 22.00,60.00 23.08,58.92 32.92,58.92 32.92,49.08\" fill=\"#E29EBA\"/><rect x=\"21.80\" y=\"59.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"22.00,60.00 34.00,60.00 32.92,61.08 23.08,61.08 23.08,70.92 22.00,72.00\" fill=\"#F6CBDD\"/><polygon points=\"34.00,60.00 34.00,72.00 22.00,72.00 23.08,70.92 32.92,70.92 32.92,61.08\" fill=\"#E29EBA\"/><rect x=\"21.80\" y=\"71.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"22.00,72.00 34.00,72.00 32.92,73.08 23.08,73.08 23.08,82.92 22.00,84.00\" fill=\"#F6CBDD\"/><polygon points=\"34.00,72.00 34.00,84.00 22.00,84.00 23.08,82.92 32.92,82.92 32.92,73.08\" fill=\"#E29EBA\"/><rect x=\"21.80\" y=\"83.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"22.00,84.00 34.00,84.00 32.92,85.08 23.08,85.08 23.08,94.92 22.00,96.00\" fill=\"#F6CBDD\"/><polygon points=\"34.00,84.00 34.00,96.00 22.00,96.00 23.08,94.92 32.92,94.92 32.92,85.08\" fill=\"#E29EBA\"/><rect x=\"33.80\" y=\"35.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"34.00,36.00 46.00,36.00 44.92,37.08 35.08,37.08 35.08,46.92 34.00,48.00\" fill=\"#F6CBDD\"/><polygon points=\"46.00,36.00 46.00,48.00 34.00,48.00 35.08,46.92 44.92,46.92 44.92,37.08\" fill=\"#E29EBA\"/><rect x=\"33.80\" y=\"47.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"34.00,48.00 46.00,48.00 44.92,49.08 35.08,49.08 35.08,58.92 34.00,60.00\" fill=\"#F6CBDD\"/><polygon points=\"46.00,48.00 46.00,60.00 34.00,60.00 35.08,58.92 44.92,58.92 44.92,49.08\" fill=\"#E29EBA\"/><rect x=\"33.80\" y=\"59.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"34.00,60.00 46.00,60.00 44.92,61.08 35.08,61.08 35.08,70.92 34.00,72.00\" fill=\"#F6CBDD\"/><polygon points=\"46.00,60.00 46.00,72.00 34.00,72.00 35.08,70.92 44.92,70.92 44.92,61.08\" fill=\"#E29EBA\"/><rect x=\"33.80\" y=\"71.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"34.00,72.00 46.00,72.00 44.92,73.08 35.08,73.08 35.08,82.92 34.00,84.00\" fill=\"#F6CBDD\"/><polygon points=\"46.00,72.00 46.00,84.00 34.00,84.00 35.08,82.92 44.92,82.92 44.92,73.08\" fill=\"#E29EBA\"/><rect x=\"33.80\" y=\"83.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"34.00,84.00 46.00,84.00 44.92,85.08 35.08,85.08 35.08,94.92 34.00,96.00\" fill=\"#F6CBDD\"/><polygon points=\"46.00,84.00 46.00,96.00 34.00,96.00 35.08,94.92 44.92,94.92 44.92,85.08\" fill=\"#E29EBA\"/><rect x=\"33.80\" y=\"95.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"34.00,96.00 46.00,96.00 44.92,97.08 35.08,97.08 35.08,106.92 34.00,108.00\" fill=\"#F6CBDD\"/><polygon points=\"46.00,96.00 46.00,108.00 34.00,108.00 35.08,106.92 44.92,106.92 44.92,97.08\" fill=\"#E29EBA\"/><rect x=\"45.80\" y=\"35.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"46.00,36.00 58.00,36.00 56.92,37.08 47.08,37.08 47.08,46.92 46.00,48.00\" fill=\"#F6CBDD\"/><polygon points=\"58.00,36.00 58.00,48.00 46.00,48.00 47.08,46.92 56.92,46.92 56.92,37.08\" fill=\"#E29EBA\"/><rect x=\"45.80\" y=\"47.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"46.00,48.00 58.00,48.00 56.92,49.08 47.08,49.08 47.08,58.92 46.00,60.00\" fill=\"#F6CBDD\"/><polygon points=\"58.00,48.00 58.00,60.00 46.00,60.00 47.08,58.92 56.92,58.92 56.92,49.08\" fill=\"#E29EBA\"/><rect x=\"45.80\" y=\"59.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"46.00,60.00 58.00,60.00 56.92,61.08 47.08,61.08 47.08,70.92 46.00,72.00\" fill=\"#F6CBDD\"/><polygon points=\"58.00,60.00 58.00,72.00 46.00,72.00 47.08,70.92 56.92,70.92 56.92,61.08\" fill=\"#E29EBA\"/><rect x=\"45.80\" y=\"71.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"46.00,72.00 58.00,72.00 56.92,73.08 47.08,73.08 47.08,82.92 46.00,84.00\" fill=\"#F6CBDD\"/><polygon points=\"58.00,72.00 58.00,84.00 46.00,84.00 47.08,82.92 56.92,82.92 56.92,73.08\" fill=\"#E29EBA\"/><rect x=\"45.80\" y=\"83.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"46.00,84.00 58.00,84.00 56.92,85.08 47.08,85.08 47.08,94.92 46.00,96.00\" fill=\"#F6CBDD\"/><polygon points=\"58.00,84.00 58.00,96.00 46.00,96.00 47.08,94.92 56.92,94.92 56.92,85.08\" fill=\"#E29EBA\"/><rect x=\"45.80\" y=\"95.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"46.00,96.00 58.00,96.00 56.92,97.08 47.08,97.08 47.08,106.92 46.00,108.00\" fill=\"#F6CBDD\"/><polygon points=\"58.00,96.00 58.00,108.00 46.00,108.00 47.08,106.92 56.92,106.92 56.92,97.08\" fill=\"#E29EBA\"/><rect x=\"57.80\" y=\"35.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"58.00,36.00 70.00,36.00 68.92,37.08 59.08,37.08 59.08,46.92 58.00,48.00\" fill=\"#F6CBDD\"/><polygon points=\"70.00,36.00 70.00,48.00 58.00,48.00 59.08,46.92 68.92,46.92 68.92,37.08\" fill=\"#E29EBA\"/><rect x=\"57.80\" y=\"47.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"58.00,48.00 70.00,48.00 68.92,49.08 59.08,49.08 59.08,58.92 58.00,60.00\" fill=\"#F6CBDD\"/><polygon points=\"70.00,48.00 70.00,60.00 58.00,60.00 59.08,58.92 68.92,58.92 68.92,49.08\" fill=\"#E29EBA\"/><rect x=\"57.80\" y=\"59.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"58.00,60.00 70.00,60.00 68.92,61.08 59.08,61.08 59.08,70.92 58.00,72.00\" fill=\"#F6CBDD\"/><polygon points=\"70.00,60.00 70.00,72.00 58.00,72.00 59.08,70.92 68.92,70.92 68.92,61.08\" fill=\"#E29EBA\"/><rect x=\"57.80\" y=\"71.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"58.00,72.00 70.00,72.00 68.92,73.08 59.08,73.08 59.08,82.92 58.00,84.00\" fill=\"#F6CBDD\"/><polygon points=\"70.00,72.00 70.00,84.00 58.00,84.00 59.08,82.92 68.92,82.92 68.92,73.08\" fill=\"#E29EBA\"/><rect x=\"57.80\" y=\"83.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"58.00,84.00 70.00,84.00 68.92,85.08 59.08,85.08 59.08,94.92 58.00,96.00\" fill=\"#F6CBDD\"/><polygon points=\"70.00,84.00 70.00,96.00 58.00,96.00 59.08,94.92 68.92,94.92 68.92,85.08\" fill=\"#E29EBA\"/><rect x=\"57.80\" y=\"95.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"58.00,96.00 70.00,96.00 68.92,97.08 59.08,97.08 59.08,106.92 58.00,108.00\" fill=\"#F6CBDD\"/><polygon points=\"70.00,96.00 70.00,108.00 58.00,108.00 59.08,106.92 68.92,106.92 68.92,97.08\" fill=\"#E29EBA\"/><rect x=\"69.80\" y=\"47.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"70.00,48.00 82.00,48.00 80.92,49.08 71.08,49.08 71.08,58.92 70.00,60.00\" fill=\"#F6CBDD\"/><polygon points=\"82.00,48.00 82.00,60.00 70.00,60.00 71.08,58.92 80.92,58.92 80.92,49.08\" fill=\"#E29EBA\"/><rect x=\"69.80\" y=\"59.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"70.00,60.00 82.00,60.00 80.92,61.08 71.08,61.08 71.08,70.92 70.00,72.00\" fill=\"#F6CBDD\"/><polygon points=\"82.00,60.00 82.00,72.00 70.00,72.00 71.08,70.92 80.92,70.92 80.92,61.08\" fill=\"#E29EBA\"/><rect x=\"69.80\" y=\"71.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"70.00,72.00 82.00,72.00 80.92,73.08 71.08,73.08 71.08,82.92 70.00,84.00\" fill=\"#F6CBDD\"/><polygon points=\"82.00,72.00 82.00,84.00 70.00,84.00 71.08,82.92 80.92,82.92 80.92,73.08\" fill=\"#E29EBA\"/><rect x=\"69.80\" y=\"83.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"70.00,84.00 82.00,84.00 80.92,85.08 71.08,85.08 71.08,94.92 70.00,96.00\" fill=\"#F6CBDD\"/><polygon points=\"82.00,84.00 82.00,96.00 70.00,96.00 71.08,94.92 80.92,94.92 80.92,85.08\" fill=\"#E29EBA\"/><rect x=\"69.80\" y=\"95.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"70.00,96.00 82.00,96.00 80.92,97.08 71.08,97.08 71.08,106.92 70.00,108.00\" fill=\"#F6CBDD\"/><polygon points=\"82.00,96.00 82.00,108.00 70.00,108.00 71.08,106.92 80.92,106.92 80.92,97.08\" fill=\"#E29EBA\"/><rect x=\"81.80\" y=\"59.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"82.00,60.00 94.00,60.00 92.92,61.08 83.08,61.08 83.08,70.92 82.00,72.00\" fill=\"#F6CBDD\"/><polygon points=\"94.00,60.00 94.00,72.00 82.00,72.00 83.08,70.92 92.92,70.92 92.92,61.08\" fill=\"#E29EBA\"/><rect x=\"81.80\" y=\"71.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"82.00,72.00 94.00,72.00 92.92,73.08 83.08,73.08 83.08,82.92 82.00,84.00\" fill=\"#F6CBDD\"/><polygon points=\"94.00,72.00 94.00,84.00 82.00,84.00 83.08,82.92 92.92,82.92 92.92,73.08\" fill=\"#E29EBA\"/><rect x=\"81.80\" y=\"83.80\" width=\"12.40\" height=\"12.40\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"82.00,84.00 94.00,84.00 92.92,85.08 83.08,85.08 83.08,94.92 82.00,96.00\" fill=\"#F6CBDD\"/><polygon points=\"94.00,84.00 94.00,96.00 82.00,96.00 83.08,94.92 92.92,94.92 92.92,85.08\" fill=\"#E29EBA\"/><rect x=\"40.00\" y=\"58.00\" width=\"8.00\" height=\"8.00\" rx=\"1.00\" fill=\"#16181D\"/><rect x=\"35.20\" y=\"70.40\" width=\"9.60\" height=\"4.40\" rx=\"1.60\" fill=\"#F58BA6\" opacity=\".9\"/><rect x=\"76.00\" y=\"58.00\" width=\"8.00\" height=\"8.00\" rx=\"1.00\" fill=\"#16181D\"/><rect x=\"79.20\" y=\"70.40\" width=\"9.60\" height=\"4.40\" rx=\"1.60\" fill=\"#F58BA6\" opacity=\".9\"/><rect x=\"47.97\" y=\"85.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><rect x=\"51.97\" y=\"85.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><rect x=\"55.97\" y=\"85.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><rect x=\"59.97\" y=\"85.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><rect x=\"63.97\" y=\"85.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><rect x=\"51.97\" y=\"89.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><rect x=\"55.97\" y=\"89.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><rect x=\"59.97\" y=\"89.97\" width=\"4.06\" height=\"4.06\" rx=\"0.00\" fill=\"#16181D\"/><g transform=\"rotate(14 88.00 23.00)\"><polygon points=\"93.16,17.84 97.12,13.88 97.12,24.20 93.16,28.16\" fill=\"#CE82A2\"/><polygon points=\"82.84,17.84 86.80,13.88 97.12,13.88 93.16,17.84\" fill=\"#F7D3E2\"/><rect x=\"82.84\" y=\"17.84\" width=\"10.32\" height=\"10.32\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"82.84,17.84 93.16,17.84 92.23,18.77 83.77,18.77 83.77,27.23 82.84,28.16\" fill=\"#F6CBDD\"/><polygon points=\"93.16,17.84 93.16,28.16 82.84,28.16 83.77,27.23 92.23,27.23 92.23,18.77\" fill=\"#E29EBA\"/></g><g transform=\"rotate(-18 104.00 45.00)\"><polygon points=\"107.96,41.04 111.00,38.00 111.00,45.92 107.96,48.96\" fill=\"#CE82A2\"/><polygon points=\"100.04,41.04 103.08,38.00 111.00,38.00 107.96,41.04\" fill=\"#F7D3E2\"/><rect x=\"100.04\" y=\"41.04\" width=\"7.92\" height=\"7.92\" rx=\"0.00\" fill=\"#F2B5CE\"/><polygon points=\"100.04,41.04 107.96,41.04 107.25,41.75 100.75,41.75 100.75,48.25 100.04,48.96\" fill=\"#F6CBDD\"/><polygon points=\"107.96,41.04 107.96,48.96 100.04,48.96 100.75,48.25 107.25,48.25 107.25,41.75\" fill=\"#E29EBA\"/></g></g>";
// </app-icon>

let iconId = 0;
/** The app's icon as SVG, the same drawing as the apps' (scripts/pixel/icon.py writes APP_ICON): the « rose » Bip of
 *  pixel blocks, two last blocks flying in, on a near-black ground. `round` = the corner radius in icon units (0: a
 *  full square, as the stores mask it). */
export function iconSVG(round = 27) {
  const id = "ic" + (iconId++);
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120" aria-hidden="true" focusable="false">`
    + APP_ICON.replace(/ICg|ICsoft|ICc/g, m => id + m.slice(2)).replace('rx="ROUND"', `rx="${round}"`) + `</svg>`;
}

// -- Seasons: the costume of the day (PixelCostumes.season in the apps) ------------------------------------------------

function easter(y) {
  const a = y % 19, b = Math.floor(y / 100), c = y % 100, d = Math.floor(b / 4), e = b % 4, f = Math.floor((b + 8) / 25);
  const g = Math.floor((b - f + 1) / 3), h = (19 * a + b - d - g + 15) % 30, i = Math.floor(c / 4), k = c % 4;
  const l = (32 + 2 * e + 2 * i - h - k) % 7, m = Math.floor((a + 11 * h + 22 * l) / 451);
  return new Date(y, Math.floor((h + l - 7 * m + 114) / 31) - 1, (h + l - 7 * m + 114) % 31 + 1);
}
/** Today's event (Halloween, Christmas…), or null. `?costumes=<event id>` or `?costumes=none` overrides it. */
export function seasonOn(date = new Date()) {
  const forced = W && new URLSearchParams(W.location.search).get("costumes");
  if (forced) return SEASONS.find(s => s.id === forced) || null;
  const md = String(date.getMonth() + 1).padStart(2, "0") + "-" + String(date.getDate()).padStart(2, "0");
  for (const s of SEASONS) {
    if (s.birthday) continue;
    if (s.easter) {
      const e = easter(date.getFullYear()), day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
      const days = Math.round((day - e) / 86400000);
      if (days >= s.easter[0] && days <= s.easter[1]) return s;
    } else if (s.from <= s.to ? md >= s.from && md <= s.to : md >= s.from || md <= s.to) return s;
  }
  return null;
}
let dressed = 0;
/** The next costume of today's event, so Bips side by side wear different ones; null outside events. */
export function nextCostume() {
  const s = seasonOn();
  return s && s.costumes.length ? s.costumes[dressed++ % s.costumes.length] : null;
}

// -- Animalese ----------------------------------------------------------------------------------------------------

let audio;
export const sound = { on: true };
const VOWELS = { a: [800, 1250], e: [480, 1850], i: [300, 2300], o: [520, 900], u: [330, 850], y: [300, 2100] };
const PLOSIVES = new Set("bdgkpqt"), FRICATIVES = new Set("cfhjsvxz");

export function audioContext() {
  audio ??= new (W.AudioContext || W.webkitAudioContext)();
  if (audio.state === "suspended") audio.resume();
  return audio;
}

function voiceFor(c, mood) {
  const v = { base: c.pitch, syl: 0.055, spread: 2.5, drift: 0, bounce: 0, vol: 0.28 };
  if (mood === "rire") { v.base *= 1.25; v.syl = 0.045; v.bounce = 3; }
  if (mood === "surpris" || mood === "boing") { v.base *= 1.15; v.drift = 5; }
  if (mood === "happy") { v.base *= 0.92; v.syl = 0.075; v.spread = 1.2; v.vol = 0.22; }
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
  if (!sound.on || !W) return;
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
let pointer = null, pointerAt = 0;   // the page's pointer: idle Bips glance at it
if (W) {
  W.addEventListener("pointermove", e => { pointer = { x: e.clientX, y: e.clientY }; pointerAt = performance.now(); }, { passive: true });
  W.addEventListener("pointerleave", () => { pointer = null; });
}
// What a reaction says (the site's lines) and how it sounds.
const SAYS = { boing: "boing", clin: "wink", rire: "giggle", regard: "hop", surpris: "surprised" };

/**
 * `new Bip(element, "daily", { interactive, lines, mood, costume })`: draws a pixel Bip in `element` (any size: the
 * SVG fills it), breathing and blinking in its state; when interactive, a tap plays one of the apps' reactions (three
 * quick taps: a happy jump with a heart). `costume`: a costume id, or true for today's event (Halloween…).
 * `bip.mood` changes its state from outside.
 */
export class Bip {
  constructor(element, category, options = {}) {
    this.c = CATEGORIES[category] || CATEGORIES.daily;
    this.el = element;
    this.lines = options.lines ?? {};
    this.base = options.mood ?? "idle";
    this.costume = options.costume === true ? nextCostume() : (options.costume || null);
    this.t = this.c.index * 1.37;
    this.clock = this.t;
    this.reaction = null;
    this.visible = false;
    this.key = "";
    this.el.classList.add("bip");
    this.box = document.createElement("div");
    this.box.className = "bip-svg";
    this.el.append(this.box);
    const size = frameOf(this.costume);
    this.box.innerHTML = `<svg viewBox="${viewBox(size)}" aria-hidden="true" focusable="false">${SHADOW}<g></g></svg>`;
    this.fig = this.box.querySelector("g");
    this.items = [];
    if (options.interactive) this.listen();
    if ("IntersectionObserver" in W) new IntersectionObserver(([entry]) => { this.visible = entry.isIntersecting; }, { rootMargin: "120px" }).observe(this.el);
    else this.visible = true;
    living.add(this);
    this.render();
  }

  set mood(value) {
    const was = this.thinking;
    this.base = value; this.t = 0;
    if (this.thinking && !was) { this.episode = nextEpisode(this.c.id); this.mt = 0; }   // the next thinking animation
  }
  get thinking() { const s = stateOf(this.base); return s === STATES.think || s === STATES.work; }
  /** Plays a moment once (« done », « oops », « wave », « born », « sent »), then back to its state. */
  play(moment) { if (MOMENTS[moment]) { this.moment = moment; this.momentT = 0; if (REDUCED) setTimeout(() => { this.moment = null; this.render(); }, 1400); this.render(); } }
  /** What it plays now among the animations of 2026-10-29, if any. */
  motionNow() {
    if (reshapes(this.costume) || this.reaction) return null;
    if (this.moment) return [this.moment, this.momentT];
    const st = stateOf(this.base);
    if (this.thinking) {
      if (this.episode == null) { this.episode = nextEpisode(this.c.id); this.mt = 0; }
      const [v, t] = thinkingNow(this.episode, this.mt, (this.el.clientWidth || 48) <= 56);
      return MOTIONS[v] ? [v, t] : null;
    }
    if (st === STATES.sleep) return ["sleep", this.t];
    if (st === STATES.listen) return ["listen", this.t];
    return null;
  }
  /** Stops drawing it and empties its element. */
  remove() { living.delete(this); this.el.innerHTML = ""; }
  get mood() { return this.base; }

  react(name) {
    this.reaction = REACTIONS[name] ? name : pick(REACTION_ORDER);
    this.rt = 0;
    if (REDUCED) { clearTimeout(this.still); this.still = setTimeout(() => { this.reaction = null; this.render(); }, 1400); this.render(); }
  }

  say(text) {
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
    babble(text, this.c, this.reaction);
  }
  line(kind) { return pick(this.lines[kind] ?? [""]); }

  tapped() {
    const now = performance.now();
    this.taps = (this.taps || []).filter(t => now - t < 1200).concat(now);
    if (this.taps.length >= 3) {
      this.taps = [];
      this.reaction = "happyJump"; this.rt = 0;
      this.say(this.line("thanks"));
      if (REDUCED) { clearTimeout(this.still); this.still = setTimeout(() => { this.reaction = null; this.render(); }, 1400); this.render(); }
      return;
    }
    this.react();
    this.say(this.line(SAYS[this.reaction]));
  }

  listen() {
    this.el.tabIndex = 0;
    this.el.setAttribute("role", "button");
    this.el.addEventListener("pointerdown", () => { try { audioContext(); } catch { /* no sound */ } });
    this.el.addEventListener("click", () => this.tapped());
    this.el.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); this.tapped(); } });
  }

  tick(dt) {
    this.t += dt; this.clock += dt; this.mt = (this.mt || 0) + dt;
    if (this.moment) { this.momentT += dt; if (this.momentT >= MOMENTS[this.moment]) this.moment = null; }
    if (this.reaction) {
      this.rt += dt;
      const st = this.reaction === "happyJump" ? STATES.happy : REACTIONS[this.reaction];
      if (this.rt >= st.dur) this.reaction = null;
    }
  }

  render() {
    const still = REDUCED;
    const motion = this.motionNow();
    if (motion) {
      const [id, mt] = motion, CO = this.costume && COSTUMES[this.costume], clock = still ? 0.6 : this.clock;
      const look = { color: this.c.color, costume: this.costume }, C = paletteOf(look, CO), fr = frameState(CO, clock);
      const pose = poseOf(STATES.idle, 0, this.costume, clock, true);
      this.fig.innerHTML = drawMotion(look, motionFrame(id, still ? (MOTION_STILL[id] ?? 1.2) : mt, C.body, C.deep), fr,
        (this.el.clientWidth || 48) / frameOf(this.costume), pose);
      this.fig.removeAttribute("transform");
      this.items = []; this.key = still ? "motion:" + id : "";
      return;
    }
    let st, t;
    if (this.reaction) { st = this.reaction === "happyJump" ? STATES.happy : REACTIONS[this.reaction]; t = still ? st.still : this.rt; }
    else { st = stateOf(this.base); t = still ? st.still : ((this.t % st.dur) + st.dur) % st.dur; }
    const clock = still ? 0.6 : this.clock;
    const pose = poseOf(st, t, this.costume, clock, false);
    // At rest, an eye on the pointer when it moves nearby (one face pixel at most, like the states' own glances).
    if (!this.reaction && st === STATES.idle && pointer && performance.now() - pointerAt < 2500) {
      const r = this.el.getBoundingClientRect();
      const dx = pointer.x - (r.left + r.width / 2), dy = pointer.y - (r.top + r.height / 2);
      if (Math.hypot(dx, dy) < 600) pose.d.gaze = [Math.abs(dx) > r.width * 0.6 ? Math.sign(dx) : 0, Math.abs(dy) > r.height * 0.6 ? Math.sign(dy) : 0];
    }
    const CO = this.costume && COSTUMES[this.costume], fr = frameState(CO, clock);
    const size = this.el.clientWidth || 48, box = frameOf(this.costume);
    const look = { color: this.c.color, costume: this.costume };
    const key = JSON.stringify([pose.d, fr, Math.round(size)]);
    if (key !== this.key) {
      this.key = key;
      this.fig.innerHTML = drawPixel(look, pose, fr, size / box);
      this.items = [...this.fig.querySelectorAll("[data-it]")];
    }
    if (CO) for (const n of this.items) {
      const it = CO.items[+n.dataset.it];
      if (!it.wave && !it.lift && !it.blink) continue;
      const q = itemDyn(it, clock, pose.m);
      n.setAttribute("transform", `translate(${fmt(q.x)} ${fmt(q.y)}) rotate(${fmt(q.rot)} ${n.dataset.pv})`);
      n.style.opacity = q.op ? "" : "0";
    }
    this.fig.setAttribute("transform", figTransform(pose.m));
  }
}

if (W) {
  let last = 0;
  const frame = now => {
    // At most 30 frames a second; nothing for Bips off screen (the browser stops frames in a hidden tab).
    if (last && now - last < 32) { requestAnimationFrame(frame); return; }
    const dt = last ? Math.min(0.1, (now - last) / 1000) : 0;
    last = now;
    for (const bip of living) {
      if (!bip.visible) continue;
      if (!REDUCED) bip.tick(dt); else if (bip.reaction || bip.moment) bip.tick(dt);
      if (!REDUCED || bip.reaction || !bip.key) bip.render();
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}
