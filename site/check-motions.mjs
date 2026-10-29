// Not under the MIT license: the Bips' drawings are reserved (LICENSE-ASSETS).
// The site's Bip animations (site/src/shared/bips.js, motionFrame) against the design page's own frames
// (scripts/pixel/motion-frames.json, written by scripts/pixel/motions-ref.mjs), as the apps' tests do:
//
//     node site/check-motions.mjs
//
// Exits 1 on the first difference past 0.01 (colours: one step per channel). Also draws every animation and costume
// once, to catch a markup error.
import { copyFileSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const dir = mkdtempSync(join(tmpdir(), "bips-"));
copyFileSync(join(ROOT, "site/src/shared/bips.js"), join(dir, "bips.js"));
writeFileSync(join(dir, "pixel-data.js"), "export default " + readFileSync(join(ROOT, "scripts/pixel/pixel.json"), "utf8") + ";\n");
const m = await import(pathToFileURL(join(dir, "bips.js")).href);
const ref = JSON.parse(readFileSync(join(ROOT, "scripts/pixel/motion-frames.json"), "utf8"));

let failures = 0, checked = 0;
const fail = msg => { failures++; if (failures <= 20) console.error("✗", msg); };
const num = (a, b, w) => { if (Math.abs((a ?? 0) - (b ?? 0)) > 0.01) fail(`${w}: ${a} ≠ ${b}`); };
const col = (a, b, w) => {
  if (!a || !b || !a.startsWith("#") || !b.startsWith("#")) { if ((a || "") !== (b || "")) fail(`${w}: ${a} ≠ ${b}`); return; }
  for (const i of [1, 3, 5]) if (Math.abs(parseInt(a.slice(i, i + 2), 16) - parseInt(b.slice(i, i + 2), 16)) > 1) return fail(`${w}: ${a} ≠ ${b}`);
};
for (const group of ["anims", "fun"]) {
  for (const [id, times] of Object.entries(ref[group])) {
    for (const [t, frame] of Object.entries(times)) {
      const mine = m.motionFrame(id, +t, ref.body, ref.deep), w = `${id} @ ${t}`;
      if (!mine) { if (id !== "today") fail(`${w}: missing`); continue; }
      if (mine.length !== frame.items.length) { fail(`${w}: ${mine.length} items ≠ ${frame.items.length}`); continue; }
      mine.forEach((a, n) => {
        const b = frame.items[n], ww = `${w} #${n}`;
        if (a.k !== b.k) return fail(`${ww}: ${a.k} ≠ ${b.k}`);
        for (const k of ["i", "j", "dx", "dy", "rot", "s", "op", "x", "y", "w", "h", "rx", "cx", "cy", "u", "ox", "oy"]) if (k in a || k in b) num(a[k], b[k], `${ww}.${k}`);
        for (const k of ["fill", "stroke"]) if (k in a || k in b) col(a[k], b[k], `${ww}.${k}`);
        for (const k of ["eyes", "mouth"]) if (k in b && a[k] !== b[k]) fail(`${ww}.${k}: ${a[k]} ≠ ${b[k]}`);
        if (b.gaze && a.gaze.join() !== b.gaze.join()) fail(`${ww}.gaze`);
        if (b.rows && a.rows.join() !== b.rows.join()) fail(`${ww}.rows`);
        if (b.pts) b.pts.forEach((p, q) => { num(a.pts[q][0], p[0], `${ww}.pts`); num(a.pts[q][1], p[1], `${ww}.pts`); });
      });
      checked++;
    }
  }
}
// Every motion and every costume draws (sizes 22, 44, 120).
for (const motion of [...m.THINKING, ...Object.keys(m.MOMENTS), "sleep", "listen"]) {
  for (const costume of [null, ...Object.keys(m.COSTUMES)]) {
    for (const size of [22, 44, 120]) {
      const svg = m.pixelSVG({ category: "creative", mood: "think", t: 1.3, mt: 1.3, motion, costume, size, still: false });
      if (!svg.startsWith("<svg") || /NaN|undefined/.test(svg)) fail(`draw ${motion} ${costume} ${size}`);
    }
  }
}
console.log(failures ? `${failures} difference(s)` : `ok: ${checked} frames match the design page`);
process.exit(failures ? 1 : 0);
