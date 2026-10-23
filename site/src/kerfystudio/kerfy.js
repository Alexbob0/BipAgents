// kerfystudio.com: the Bips in BipAgents' showcase, the app tiles that lean toward the pointer, the reveals.
import { Bip, REDUCED } from "./bips.js";

for (const el of document.querySelectorAll("[data-bip]")) new Bip(el, el.dataset.bip, { interactive: el.hasAttribute("data-interactive") });

const io = new IntersectionObserver(entries => {
  for (const e of entries) if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
}, { threshold: 0.12 });
document.querySelectorAll(".reveal").forEach(el => io.observe(el));

if (!REDUCED && matchMedia("(hover: hover)").matches) {
  const tilts = [...document.querySelectorAll("[data-tilt]")];
  addEventListener("pointermove", e => {
    for (const el of tilts) {
      const r = el.getBoundingClientRect();
      const x = (e.clientX - (r.left + r.width / 2)) / innerWidth, y = (e.clientY - (r.top + r.height / 2)) / innerHeight;
      el.style.setProperty("--rx", `${(-y * 10).toFixed(2)}deg`);
      el.style.setProperty("--ry", `${(x * 12).toFixed(2)}deg`);
    }
  }, { passive: true });
}
