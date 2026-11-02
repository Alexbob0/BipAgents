// Not under the MIT license: the Bips' drawings and sounds are reserved (LICENSE-ASSETS).
// Builds docs/bips.svg, the animated README banner: the eight Bips in pixels, drawn by the websites' engine
// (site/src/shared/bips.js) from scripts/pixel/pixel.json, as the apps draw them, each bobbing.
// CSS animations only (GitHub shows SVG through <img>, scripts never run). Run: node docs/tools/banner.mjs
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "../..");
// The engine imports ./pixel-data.js, which site/build.py writes next to it: do the same in a temporary folder.
const dir = mkdtempSync(join(tmpdir(), "bips-banner-"));
copyFileSync(join(ROOT, "site/src/shared/bips.js"), join(dir, "bips.js"));
writeFileSync(join(dir, "pixel-data.js"), "export default " + readFileSync(join(ROOT, "scripts/pixel/pixel.json"), "utf8") + ";\n");
const { CATEGORIES, pixelSVG } = await import(pathToFileURL(join(dir, "bips.js")).href);
rmSync(dir, { recursive: true, force: true });

const W = 120, step = 123.5, height = 150;
const width = 988;
const bips = Object.keys(CATEGORIES).map((category, i) =>
  `<g class="bob" style="animation-delay:-${(i * 0.4).toFixed(1)}s">`
  + pixelSVG({ category, still: true, size: W }).replace("<svg ", `<svg x="${(i * step).toFixed(1)}" y="15" width="${W}" height="${W}" `)
  + "</g>");

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}">
<title>The BipAgents Bips</title>
<style>
  .bob { animation: bob 3.2s ease-in-out infinite; }
  @keyframes bob { 0%, 100% { transform: translateY(0); } 50% { transform: translateY(-6px); } }
  @media (prefers-reduced-motion: reduce) { .bob { animation: none; } }
</style>
${bips.join("\n")}
</svg>
`;
writeFileSync(join(ROOT, "docs/bips.svg"), svg);
console.log(`docs/bips.svg: ${svg.length} bytes`);
