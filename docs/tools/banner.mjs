// Builds docs/bips.svg, the animated README banner, from the drawing code of docs/play/index.html.
// CSS animations only (GitHub shows SVG through <img>, scripts never run). Run: node docs/tools/banner.mjs
import { readFileSync, writeFileSync } from "node:fs";

const html = readFileSync(new URL("../play/index.html", import.meta.url), "utf8");
const start = html.indexOf('const INK');
// The drawing code ends where the page texts (i18n) or the Animalese section begin.
const end = Math.min(...["// English by default", "// Animalese"].map(m => html.indexOf(m)).filter(i => i >= 0));
const drawing = html.slice(start, html.lastIndexOf("// ----", end));
const { CATEGORIES, bodyShape, accessory, eyes, face, ell } = new Function(
  "matchMedia", `${drawing}; return { CATEGORIES, bodyShape, accessory, eyes, face, ell };`)(() => ({ matches: false }));

const moods = ["happy", "winking", "happy", "content", "happy", "giggling", "happy", "listening"];
const order = ["daily", "wellness", "finance", "work", "learning", "creative", "home", "tech"];
const gaze = { x: 0, y: 0 };
const W = 120, gap = 4, width = order.length * (W + gap) - gap;

const bip = (c, mood, i) => {
  const blinking = mood === "happy" || mood === "listening";
  const faceParts = blinking
    ? `<g class="eyes" style="animation-delay:${(i * 1.37) % 4.6}s">${eyes(c, 0, gaze, ...(mood === "listening" ? [7, 9, 63, [46, 74], 2.6] : [5.5, 7.5, 64, [47, 73], 2]), false)}</g>`
      + (mood === "listening" ? ell(60, 81, 3.6, 4.2, `fill="#16181D"`) : `<path d="M53 78q7 6 14 0" fill="none" stroke="#16181D" stroke-width="3" stroke-linecap="round"/>`)
    : face(c, mood, 0, gaze);
  return `<svg x="${i * (W + gap)}" y="18" width="${W}" height="${W}" viewBox="0 0 120 120" overflow="visible">`
    + ell(60, 113, 32, 4.5, `fill="#16181D" fill-opacity="0.1"`)
    + [45, 75].map(x => ell(x, 106, 10.5, 6.5, `fill="${c.deep}"`) + ell(x - 3, 104.5, 3.5, 1.8, `fill="#fff" fill-opacity="0.25"`)).join("")
    + `<g class="${i === 3 ? "hop" : "bob"}" style="animation-delay:${-i * 0.37}s">`
    + accessory(c, false, 0) + bodyShape(c, c.main) + accessory(c, true, 0)
    + `<g transform="translate(43 44) rotate(-30)">` + ell(0, 0, 12, 7, `fill="#fff" fill-opacity="0.32"`) + `</g>`
    + ell(37, 76, 6.5, 4, `fill="#FF6F91" fill-opacity="0.5"`) + ell(83, 76, 6.5, 4, `fill="#FF6F91" fill-opacity="0.5"`)
    + faceParts + `</g></svg>`;
};

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} 150" width="${width}" height="150">
<title>The BipAgents Bips</title>
<style>
  .bob { animation: bob 3s ease-in-out infinite; }
  .hop { animation: hop 4.2s ease-in-out infinite; transform-origin: 60px 106px; transform-box: view-box; }
  .eyes { animation: blink 4.6s infinite; transform-origin: 60px 64px; transform-box: view-box; }
  @keyframes bob { 0%, 100% { transform: translateY(-2px); } 50% { transform: translateY(-6px); } }
  @keyframes hop {
    0%, 62%, 100% { transform: translateY(-2px) scale(1, 1); }
    68% { transform: translateY(0) scale(1.1, .9); }
    76% { transform: translateY(-20px) scale(.92, 1.08); }
    84% { transform: translateY(0) scale(1.08, .92); }
    90% { transform: translateY(-2px) scale(1, 1); }
  }
  @keyframes blink { 0%, 93%, 100% { transform: scaleY(1); } 96.5% { transform: scaleY(.1); } }
  @media (prefers-reduced-motion: reduce) { .bob, .hop, .eyes { animation: none; } }
</style>
${order.map((id, i) => bip(CATEGORIES.find(c => c.id === id), moods[i], i)).join("\n")}
</svg>
`;
writeFileSync(new URL("../bips.svg", import.meta.url), svg);
console.log(`docs/bips.svg ${svg.length} bytes`);
