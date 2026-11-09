// bipagents.com, the home's living room: four Bips in 3D (Three.js r128, served from this site) over a painted room
// (salon-bg.webp, 1280×720). Each Bip in turn hops and shows what it just did (the labels are the page's own HTML, in
// its language); drag a Bip to spin it, drag the room to spin them all, two turns and everyone spins; on a phone the
// room is wider than the screen: a horizontal swipe looks around and only the Bips in view take their turn.
// Three.js is fetched only when the room comes near the screen. Without WebGL the painted room stays, without the Bips.
const stage = document.querySelector(".salon-stage");
const room = document.querySelector(".salon");
const VERSION = new URL(import.meta.url).search;   // the page's ?v=, for three.min.js too
const W0 = 1280, H0 = 720, TAU = Math.PI * 2, TILT = 0.21;
const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;

if (stage && room) {
  const pannable = () => stage.clientWidth > room.clientWidth + 2;
  // on a narrow screen the room opens on the sofa and the coffee table
  const centerRoom = () => {
    const sw = stage.clientWidth, rw = room.clientWidth;
    if (sw > rw + 2) room.scrollLeft = Math.max(0, Math.min(sw - rw, sw * (600 / W0) - rw / 2));
  };
  centerRoom();
  addEventListener("resize", centerRoom, { passive: true });
  fitEasel();
  document.fonts?.ready.then(fitEasel);

  const io = new IntersectionObserver(entries => {
    if (!entries[0].isIntersecting) return;
    io.disconnect();
    const s = document.createElement("script");
    s.src = "/assets/salon/three.min.js" + VERSION;
    s.onload = () => { try { start(pannable); } catch (e) { console.warn("salon: the painted room alone", e); } };
    document.head.appendChild(s);
  }, { rootMargin: "300px 0px" });
  io.observe(stage);
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

function start(pannable) {
  const THREE = window.THREE;
  if (!THREE) return;
  const canvas = stage.querySelector(".salon-canvas"), tip = stage.querySelector(".salon-tip");
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(devicePixelRatio || 1, 2));
  renderer.setClearColor(0x000000, 0);
  stage.classList.add("live");

  const scene = new THREE.Scene();
  // the camera in the picture's pixels: x right, y up (the picture's y becomes -y); lower in the picture = closer
  const cam = new THREE.OrthographicCamera(0, W0, 0, -H0, -4000, 4000);
  cam.position.z = 1500;
  const zOf = y => y * 0.6;

  // light that matches the painting: the warm floor lamp, the cool window on the right
  scene.add(new THREE.HemisphereLight(0xFFF0E2, 0x3B2C2E, 0.62));
  const lamp = new THREE.PointLight(0xFFC890, 0.75, 0, 1); lamp.position.set(490, -110, 820); scene.add(lamp);
  const key = new THREE.DirectionalLight(0xFFF3E6, 0.42); key.position.set(-0.35, 0.75, 1); scene.add(key);
  const moon = new THREE.DirectionalLight(0xA9BCFF, 0.32); moon.position.set(1, 0.35, 0.25); scene.add(moon);

  const lineMat = new THREE.MeshBasicMaterial({ color: 0x1C1416, side: THREE.BackSide });
  const inkMat = new THREE.MeshStandardMaterial({ color: 0x16181D, roughness: 0.35, metalness: 0 });
  const solid = (color, rough = 0.6, glow = 0.14) =>
    new THREE.MeshStandardMaterial({ color, roughness: rough, metalness: 0, emissive: color, emissiveIntensity: glow });
  // a box with the same thin dark outline as the painting
  function block(w, h, dp, mat, o = 1.9) {
    const g = new THREE.Group();
    if (o > 0) g.add(new THREE.Mesh(new THREE.BoxGeometry(w + 2 * o, h + 2 * o, dp + 2 * o), lineMat));
    g.add(new THREE.Mesh(new THREE.BoxGeometry(w, h, dp), mat));
    return g;
  }
  // soft contact shadows, in HTML between the painting and the 3D layer
  function shadow(w, x, y) {
    const el = document.createElement("i");
    el.className = "salon-shade";
    Object.assign(el.style, { left: `${x / W0 * 100}%`, top: `${y / H0 * 100}%`, width: `${w / W0 * 100}%`, height: `${w / 4 / H0 * 100}%` });
    stage.insertBefore(el, canvas);
    return el;
  }

  // -- The Bips: the base Bip, one row taller so it is less wide (6 × 7 squares)
  const OUTLINE = [[-2, 0], [2, 0], [2, 1], [3, 1], [3, 6], [2, 6], [2, 7], [-2, 7], [-2, 6], [-3, 6], [-3, 1], [-2, 1]];
  function makeBip(d, index) {
    const c = d.h / 7, D = 2.6 * c, U = c / 3, b = 0.11 * c, bt = 0.14 * c;
    const bodyW = 6 * c + 2 * b, bodyD = D + 2 * bt;
    function shapeOf(o) {   // the outline pushed out by o (exact for square corners)
      const n = OUTLINE.length, pts = [];
      for (let i = 0; i < n; i++) {
        const p = OUTLINE[(i + n - 1) % n], q = OUTLINE[i], r = OUTLINE[(i + 1) % n];
        const ax = q[0] - p[0], ay = q[1] - p[1], bx = r[0] - q[0], by = r[1] - q[1];
        const la = Math.hypot(ax, ay), lb = Math.hypot(bx, by);
        pts.push([q[0] * c + o * (ay / la + by / lb), q[1] * c + o * (-ax / la - bx / lb)]);
      }
      const sh = new THREE.Shape();
      sh.moveTo(pts[0][0], pts[0][1]);
      for (let k = 1; k < n; k++) sh.lineTo(pts[k][0], pts[k][1]);
      sh.lineTo(pts[0][0], pts[0][1]);
      return sh;
    }
    function extrude(o) {
      const g = new THREE.ExtrudeGeometry(shapeOf(o), { depth: D + 2 * o, bevelEnabled: true, bevelThickness: bt, bevelSize: b, bevelSegments: 3, curveSegments: 2 });
      g.translate(0, -3.5 * c, -(D + 2 * o) / 2);
      g.computeVertexNormals();
      return g;
    }
    const col = new THREE.Color(d.color), hsl = {};
    col.getHSL(hsl);
    const sideCol = new THREE.Color().setHSL(hsl.h, Math.min(1, hsl.s * 1.05), hsl.l * 0.9);
    const body = new THREE.Mesh(extrude(0), [solid(col, 0.5, 0.2), solid(sideCol, 0.55, 0.14)]);
    const hull = new THREE.Mesh(extrude(2.2), lineMat);
    const inner = new THREE.Group();
    inner.position.y = 3.5 * c;
    inner.add(hull, body);

    // the face: 2×2 eyes and the smile, the app's proportions
    const face = new THREE.Group();
    face.position.z = D / 2 + bt + 0.02 * c;
    inner.add(face);
    const eyeGeo = new THREE.BoxGeometry(2 * U, 2 * U, 0.08 * c);
    const eyes = new THREE.Group();
    face.add(eyes);
    for (const x of [-1.5, 1.5]) { const e = new THREE.Mesh(eyeGeo, inkMat); e.position.set(x * c, 1 * c, 0); eyes.add(e); }
    const px = new THREE.BoxGeometry(U, U, 0.08 * c);
    for (const p of [[-2, 0], [2, 0], [-1, 1], [0, 1], [1, 1]]) {
      const m = new THREE.Mesh(px, inkMat); m.position.set(p[0] * U, -1 * c + U / 2 - p[1] * U, 0); face.add(m);
    }

    if (d.gear === "band") {   // a sport headband just above the eyes, its knot at the back
      const y = 2.05 * c;
      const band = block(bodyW + 0.24 * c, 0.62 * c, bodyD + 0.24 * c, solid(0xFF3B6B, 0.6, 0.18)); band.position.y = y; inner.add(band);
      const stripe = new THREE.Mesh(new THREE.BoxGeometry(bodyW + 0.28 * c, 0.13 * c, bodyD + 0.28 * c), solid(0xFFFFFF, 0.5, 0.25));
      stripe.position.y = y; inner.add(stripe);
      for (const k of [[0.32, 0.12], [-0.38, -0.1]]) {
        const tail = block(0.85 * c, 0.26 * c, 0.3 * c, solid(0xFF3B6B, 0.6, 0.18), 1.5);
        tail.position.set(-bodyW / 2 - 0.38 * c, y + k[1] * c, -bodyD / 2 + 0.3 * c); tail.rotation.z = k[0]; inner.add(tail);
      }
    }
    if (d.gear === "hat") {   // a small straw hat with a red band
      const straw = solid(0xF3D184, 0.75, 0.16);
      const hat = new THREE.Group(); hat.position.y = 3.5 * c + b; hat.rotation.z = -0.07; inner.add(hat);
      const brim = block(7.4 * c, 0.26 * c, bodyD + 1.4 * c, straw); brim.position.y = 0.15 * c; hat.add(brim);
      const crown = block(4.4 * c, 1.15 * c, bodyD * 0.8, straw); crown.position.y = 0.28 * c + 0.575 * c; hat.add(crown);
      const hb = new THREE.Mesh(new THREE.BoxGeometry(4.46 * c, 0.34 * c, bodyD * 0.8 + 0.06 * c), solid(0xE8465A, 0.6, 0.18));
      hb.position.y = 0.28 * c + 0.2 * c; hat.add(hb);
    }

    const pose = new THREE.Group(); pose.add(inner);
    const root = new THREE.Group(); root.position.set(d.x, -d.y, zOf(d.y)); root.add(pose);
    scene.add(root);
    const sh = shadow(7.6 * c, d.x, d.y + 0.18 * c);
    root.traverse(o => { if (o.isMesh) o.userData.bi = index; });
    return { d, c, root, pose, inner, eyes, shadow: sh, phase: Math.random() * TAU, blinkAt: 1 + Math.random() * 3, blink: 0,
             hop: -1, move: "boing", spin: 0, vel: 0, travel: 0, dragging: false, partyAt: 0, ss: -1 };
  }

  // Dinners, Budget, Wellness, Trips: where they stand in the painting (the labels are the page's .salon-tag, same order)
  const BIPS = [
    { color: 0xFF6A48, x: 252, y: 410, h: 146, yaw: 0.42, gaze: [0.14, 0.02], move: "boing", tagUp: 0.9 },
    { color: 0xFFC01C, x: 556, y: 580, h: 130, yaw: 0.46, gaze: [0.14, -0.04], move: "toss", tagUp: 0.9 },
    { color: 0x1FCB7C, x: 628, y: 412, h: 102, yaw: 0.55, gaze: [0.12, 0.10], move: "jog", tagUp: 0.9, gear: "band" },
    { color: 0x4677FF, x: 1052, y: 604, h: 148, yaw: -0.40, gaze: [-0.12, 0.12], move: "boing", tagUp: 2.6, gear: "hat" },
  ];
  const bips = BIPS.map(makeBip);

  // -- Coins on the coffee table, around Budget; one flies when Budget hops
  const coinR = 12, coinH = 3.4;
  const coinGeo = new THREE.CylinderGeometry(coinR, coinR, coinH, 30);
  const coinHull = new THREE.CylinderGeometry(coinR + 1.5, coinR + 1.5, coinH + 2.4, 30);
  const coinMats = [
    new THREE.MeshStandardMaterial({ color: 0xD8960F, roughness: 0.35, metalness: 0.45, emissive: 0x6B4300, emissiveIntensity: 0.3 }),
    new THREE.MeshStandardMaterial({ color: 0xFFD54A, roughness: 0.28, metalness: 0.45, emissive: 0x7A5200, emissiveIntensity: 0.35 }),
  ];
  function coin() {
    const g = new THREE.Group();
    g.add(new THREE.Mesh(coinHull, lineMat), new THREE.Mesh(coinGeo, [coinMats[0], coinMats[1], coinMats[1]]));
    return g;
  }
  function stack(x, y, n, seed) {
    const g = new THREE.Group(); g.position.set(x, -y, zOf(y)); g.rotation.x = TILT;
    const coins = [];
    for (let k = 0; k < n; k++) {
      const m = coin(); m.position.set(Math.sin(seed + k * 2.1) * 0.9, coinH / 2 + k * (coinH + 0.25), Math.cos(seed + k * 1.7) * 0.9);
      g.add(m); coins.push(m);
    }
    scene.add(g); shadow(coinR * 3.4, x, y + 1.5);
    return { g, coins, x, y };
  }
  const stackA = stack(626, 594, 5, 0.4);
  stack(656, 600, 3, 2.2);
  stack(498, 592, 2, 1.0);
  const flying = coin(); flying.visible = false; scene.add(flying);
  let toss = -1;
  function startToss() {
    if (toss >= 0 || reduce) return;
    toss = 0; stackA.coins[stackA.coins.length - 1].visible = false; flying.visible = true;
  }
  function stepToss(dt) {
    if (toss < 0) return;
    toss += dt / 1.05;
    const top = stackA.coins.length - 1, y0 = coinH / 2 + top * (coinH + 0.25);
    if (toss >= 1) { toss = -1; flying.visible = false; stackA.coins[top].visible = true; return; }
    const h = Math.sin(toss * Math.PI) * 92;
    flying.position.set(stackA.x - Math.sin(toss * Math.PI) * 10, -stackA.y + y0 * Math.cos(TILT) + h, zOf(stackA.y) + 30);
    flying.rotation.set(TILT + toss * 3 * TAU, 0, 0.2 * Math.sin(toss * TAU));
  }

  // -- Labels: the page's own, placed above their Bip
  const tags = [...stage.querySelectorAll(".salon-tag")];
  tags.forEach((el, i) => {
    const b = bips[i];
    if (!b) return;
    el.style.left = `${b.d.x / W0 * 100}%`;
    el.style.top = `${(b.d.y - b.d.h - b.d.tagUp * b.c) / H0 * 100}%`;
    el.style.setProperty("--c", "#" + b.d.color.toString(16).padStart(6, "0"));
  });
  const showTag = i => tags.forEach((t, k) => t.classList.toggle("on", k === i));

  // -- Moves
  function curve(move, t) {   // → [height in cells, scale x, scale y]
    if (move === "jog") {   // three quick little jumps
      const p = (t * 3) % 1, y = Math.sin(p * Math.PI) * 0.4, sq = p < 0.12 || p > 0.88 ? 0.08 : 0;
      return [y, 1 + sq, 1 - sq + (y > 0.1 ? 0.04 : 0)];
    }
    if (t < 0.12) { const k = t / 0.12; return [0, 1 + 0.1 * k, 1 - 0.14 * k]; }
    if (t < 0.56) { const u = (t - 0.12) / 0.44; return [Math.sin(u * Math.PI) * 0.85, 1.1 - 0.16 * Math.min(1, u * 3), 0.86 + 0.22 * Math.min(1, u * 3)]; }
    if (t < 0.7) { const v = (t - 0.56) / 0.14; return [0, 1.1 - 0.1 * v + 0.08 * Math.sin(v * Math.PI), 0.88 + 0.12 * v]; }
    const w = (t - 0.7) / 0.3; return [0, 1 + 0.03 * Math.sin(w * TAU) * (1 - w), 1 - 0.03 * Math.sin(w * TAU) * (1 - w)];
  }
  function hop(i) {
    const b = bips[i];
    if (b.hop >= 0 || reduce) return;
    b.hop = 0; b.move = b.d.move === "jog" ? "jog" : "boing";
    if (b.d.move === "toss") startToss();
  }
  let partyReady = 0;
  function party(t) {   // two full turns: everyone spins
    partyReady = t + 5;
    bips.forEach((b, i) => { b.partyAt = t + 0.1 + i * 0.13; });
    setTimeout(startToss, 350);
  }

  let hover = -1, cycle = 0, nextCycle = 1.2, last = performance.now() / 1000, panVel = 0, drag = null;
  function frame(t) {
    const dt = Math.min(0.05, Math.max(0, t - last)); last = t;
    bips.forEach((b, i) => {
      if (b.partyAt && t >= b.partyAt) { b.partyAt = 0; b.hop = -1; hop(i); b.vel = (i % 2 ? -1 : 1) * 17; b.travel = 0; }
      // a spin follows the finger, coasts, then settles back facing its job
      if (!b.dragging) {
        if (Math.abs(b.vel) > 0.8) { b.spin += b.vel * dt; b.travel += Math.abs(b.vel * dt); b.vel *= Math.exp(-2 * dt); }
        else {
          b.vel = 0;
          const target = Math.round(b.spin / TAU) * TAU;
          b.spin += (target - b.spin) * (1 - Math.exp(-7 * dt));
          if (Math.abs(target - b.spin) < 0.003) { b.spin = 0; b.travel = 0; }
        }
      }
      if (b.travel > 2 * TAU && t > partyReady) party(t);

      let y = 0, sx = 1, sy = 1;
      if (!reduce) {
        const br = Math.sin(t * 2.4 + b.phase);
        sy = 1 + 0.014 * br; sx = 1 - 0.007 * br;
        b.blinkAt -= dt;
        if (b.blinkAt <= 0) { b.blink = 0.14; b.blinkAt = 2.6 + Math.random() * 3.4; }
        if (b.blink > 0) b.blink -= dt;
      }
      if (b.hop >= 0) {
        b.hop += dt / (b.move === "jog" ? 1.2 : 0.95);
        if (b.hop >= 1) b.hop = -1; else { const h = curve(b.move, b.hop); y = h[0] * b.c; sx *= h[1]; sy *= h[2]; }
      }
      b.pose.rotation.set(TILT - 0.01, b.d.yaw + b.spin, 0);
      b.inner.scale.set(sx, sy, 1);
      b.inner.position.y = 3.5 * b.c * sy + y;
      const look = (i === hover || Math.abs(b.spin) > 0.05) ? [0, 0] : b.d.gaze;
      b.eyes.position.set(look[0] * b.c, look[1] * b.c, 0);
      b.eyes.scale.y = b.blink > 0 ? 0.15 : 1;
      const s = 1 - Math.min(0.35, y / (2.2 * b.c));
      if (s !== b.ss) { b.ss = s; b.shadow.style.transform = `translate(-50%,-50%) scale(${s.toFixed(3)})`; b.shadow.style.opacity = s.toFixed(3); }
    });
    stepToss(dt);
    if (Math.abs(panVel) > 12) { room.scrollLeft -= panVel * dt; panVel *= Math.exp(-4 * dt); } else panVel = 0;
    renderer.render(scene, cam);
  }
  function resize() { renderer.setSize(stage.clientWidth, stage.clientHeight, false); frame(performance.now() / 1000); }
  function inView(b) {
    const x = b.d.x / W0 * stage.clientWidth - room.scrollLeft;
    return x > 70 && x < room.clientWidth - 70;
  }
  let running = true;
  function loop() {
    if (!running) return;
    const t = performance.now() / 1000;
    if (!reduce && hover < 0 && !drag) {
      nextCycle -= Math.min(0.05, t - last);
      if (nextCycle <= 0) {   // on a phone, only the Bips in view take their turn
        for (let n = 0; n < bips.length && !inView(bips[cycle]); n++) cycle = (cycle + 1) % bips.length;
        hop(cycle); showTag(cycle); cycle = (cycle + 1) % bips.length; nextCycle = 3.6;
      }
    }
    frame(t);
    requestAnimationFrame(loop);
  }

  // -- Pointer: tap a Bip to say hi, drag a Bip to spin it, drag the room to look around (or to spin them all)
  const ray = new THREE.Raycaster(), v2 = new THREE.Vector2(), meshes = [];
  bips.forEach(b => b.root.traverse(o => { if (o.isMesh) meshes.push(o); }));
  function pick(ev) {
    const r = canvas.getBoundingClientRect();
    v2.set((ev.clientX - r.left) / r.width * 2 - 1, -((ev.clientY - r.top) / r.height) * 2 + 1);
    ray.setFromCamera(v2, cam);
    const hits = ray.intersectObjects(meshes, false);
    return hits.length ? hits[0].object.userData.bi : -1;
  }
  const K = 0.016;
  let played = false;
  const targets = () => drag.target === "all" ? bips : [bips[drag.target]];
  canvas.addEventListener("pointerdown", ev => {
    if (ev.button) return;
    const i = pick(ev);
    drag = { id: ev.pointerId, x0: ev.clientX, lx: ev.clientX, lt: performance.now(), vx: 0, moved: false,
             target: i >= 0 ? i : (pannable() ? "pan" : "all") };
    panVel = 0;
    try { canvas.setPointerCapture(ev.pointerId); } catch (e) { /* already released */ }
  });
  canvas.addEventListener("pointermove", ev => {
    if (drag && ev.pointerId === drag.id) {
      const now = performance.now(), dx = ev.clientX - drag.lx, dtm = Math.max(1, now - drag.lt);
      if (!drag.moved && Math.abs(ev.clientX - drag.x0) > 6) {
        drag.moved = true;
        if (drag.target !== "pan") targets().forEach(b => { b.dragging = true; b.vel = 0; });
        if (!played) { played = true; tip?.classList.add("off"); }
      }
      if (drag.moved) {
        if (drag.target === "pan") room.scrollLeft -= dx;
        else targets().forEach(b => { b.spin += dx * K; b.travel += Math.abs(dx * K); });
        drag.vx = drag.vx * 0.6 + (dx / dtm * 1000) * 0.4;
      }
      drag.lx = ev.clientX; drag.lt = now;
      return;
    }
    if (ev.pointerType !== "mouse") return;
    const h = pick(ev);
    canvas.style.cursor = h >= 0 ? "grab" : (pannable() ? "default" : "ew-resize");
    if (h !== hover) { hover = h; if (h >= 0) { showTag(h); hop(h); } else nextCycle = 2.4; }
  });
  function end(ev) {
    if (!drag || ev.pointerId !== drag.id) return;
    const d = drag; drag = null;
    if (!d.moved) {
      const i = pick(ev);
      if (i >= 0) { showTag(i); hop(i); cycle = (i + 1) % bips.length; nextCycle = 4.5; } else showTag(-1);
      return;
    }
    if (performance.now() - d.lt > 120) d.vx = 0;   // the finger stopped before lifting
    if (d.target === "pan") { panVel = d.vx; return; }
    (d.target === "all" ? bips : [bips[d.target]]).forEach(b => { b.dragging = false; b.vel = d.vx * K; });
  }
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  canvas.addEventListener("pointerleave", () => { if (!drag) { hover = -1; nextCycle = 2.4; canvas.style.cursor = "default"; } });

  new ResizeObserver(resize).observe(stage);
  resize();
  // drawn only while the room is on screen
  new IntersectionObserver(entries => {
    const vis = entries[0].isIntersecting;
    if (vis && !running) { running = true; last = performance.now() / 1000; requestAnimationFrame(loop); }
    else if (!vis) running = false;
  }).observe(stage);
  if (reduce) showTag(0);
  requestAnimationFrame(loop);
}
