// bipagents.com, the home's living room: a painted room (salon-bg.webp, 1280×720) and four Bips drawn once from the
// former 3D scene (sprites/, scripts/salon-sprites/). They breathe and blink in CSS; this only says whose turn it is
// (every 3.6 s one Bip hops and shows what it just did; the labels are the page's own HTML, in its language), pauses
// everything off screen or in a background tab, and fits the easel's words. The room fills the hero and is cropped
// at its edges (on a phone, to the coffee table): only the Bips in view take their turn. With reduced motion nothing
// moves and the Dinners label stays; pointing at or tapping a Bip shows its label.
const stage = document.querySelector(".salon-stage");
const room = document.querySelector(".salon");
const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;

if (stage && room) {
  // the room is wider than the hero and cropped by it: nothing may ever scroll it (that slid the promise off screen)
  const hero = room.closest(".hero");
  hero?.addEventListener("scroll", () => { hero.scrollLeft = 0; hero.scrollTop = 0; }, { passive: true });
  whenLoaded();
  fitEasel();
  document.fonts?.ready.then(fitEasel);
  turns();
}

// Each Bip (and the coins) shows once all its pictures are loaded and decoded, never half drawn nor with a missing
// picture's frame; a picture that failed is asked again (twice, a little later each time). The painting too: hidden
// while it fails, asked again; the Bips wait for it.
function whenLoaded() {
  const again = (img, n) => { const u = new URL(img.src); u.searchParams.set("r", n); img.src = u.href; };
  for (const group of stage.querySelectorAll(".salon-bip, .salon-coins, .salon-coin")) {
    const imgs = [...group.querySelectorAll("img")];
    (async () => {
      for (let n = 1; n <= 3; n++) {
        try {
          await Promise.all(imgs.map(img => img.decode()));
          group.classList.add("ready");
          if (group.classList.contains("salon-coins")) stage.classList.add("coins-ready");
          return;
        } catch (e) {
          if (n === 3) return;   // stays hidden
          await new Promise(r => setTimeout(r, 700 * n));
          for (const img of imgs) if (img.complete && !img.naturalWidth) again(img, n);   // broken, not still coming
        }
      }
    })();
  }
  const bg = stage.querySelector(":scope > img");
  let tries = 0;
  const failed = () => {
    bg.style.visibility = "hidden";
    if (++tries <= 2) setTimeout(() => again(bg, tries), 700 * tries);
  };
  // the Bips, their shadows and the easel's words wait for the painting
  const shown = () => { bg.style.visibility = ""; stage.classList.add("bg-ready"); };
  if (!bg) return stage.classList.add("bg-ready");
  bg.addEventListener("error", failed);
  bg.addEventListener("load", shown);
  if (bg.complete) bg.naturalWidth ? shown() : failed();
}

// The easel's and the screens' lines are the page's text: a line too long for its spot (a long word in another
// language) is drawn a little narrower instead of running off it.
function fitEasel() {
  for (const t of document.querySelectorAll(".salon-easel text[data-max]")) {
    t.removeAttribute("textLength");
    const max = +t.dataset.max;
    try {
      if (t.getComputedTextLength() > max) { t.setAttribute("textLength", max); t.setAttribute("lengthAdjust", "spacingAndGlyphs"); }
    } catch (e) { /* not laid out yet */ }
  }
}

function turns() {
  const order = ["dinners", "budget", "wellness", "trips"];   // the labels' order (the page's .salon-tag)
  const bips = order.map(n => stage.querySelector(".b-" + n));
  const tags = [...stage.querySelectorAll(".salon-tag")];
  if (bips.some(b => !b)) return;
  stage.classList.add("live");
  let cur = -1, next = 0, timer = 0, onScreen = false;

  function show(i) {
    tags.forEach((t, k) => t.classList.toggle("on", k === i));
    cur = i;
    if (reduce) return;
    const b = bips[i];
    bips.forEach(o => { if (o !== b) o.classList.remove("on"); });
    if (b.classList.contains("on")) {   // the same Bip again (only one in view): restart its hop
      b.classList.remove("on");
      requestAnimationFrame(() => requestAnimationFrame(() => b.classList.add("on")));
    } else b.classList.add("on");
  }
  const inView = b => {
    const r = b.getBoundingClientRect(), v = room.getBoundingClientRect(), x = (r.left + r.right) / 2;
    return x > v.left + 40 && x < v.right - 40;
  };
  function tick() {
    timer = 0;
    if (!onScreen || document.hidden) return;
    let i = next;
    for (let n = 0; n < bips.length && !(inView(bips[i]) && bips[i].classList.contains("ready")); n++) i = (i + 1) % bips.length;
    if (bips[i].classList.contains("ready")) { show(i); next = (i + 1) % bips.length; }
    timer = setTimeout(tick, 3600);
  }
  function play(on) {
    clearTimeout(timer); timer = 0;
    stage.classList.toggle("idle", !on);
    if (on && !reduce) timer = setTimeout(tick, cur < 0 ? 1200 : 2400);
  }
  if (reduce) show(Math.max(0, bips.findIndex(inView)));   // Dinners, or the first Bip in view (phone)
  new IntersectionObserver(entries => { onScreen = entries[0].isIntersecting; play(onScreen && !document.hidden); }).observe(stage);
  document.addEventListener("visibilitychange", () => play(onScreen && !document.hidden));
  // pointing at a Bip (mouse) or tapping it: its turn now
  bips.forEach((b, i) => {
    const go = () => {
      if (!b.classList.contains("ready")) return;
      show(i); next = (i + 1) % bips.length;
      if (!reduce && onScreen) { clearTimeout(timer); timer = setTimeout(tick, 4500); }
    };
    b.addEventListener("pointerenter", e => { if (e.pointerType === "mouse" && cur !== i) go(); });
    b.addEventListener("click", go);
  });
}
