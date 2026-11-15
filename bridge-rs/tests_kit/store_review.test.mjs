// Independent review of `page-store` (15/11): the kit's Bip.store (bridge-rs/assets/static/pagekit/store.js) run in
// node against a fake door, with a fake clock. Run: node --test kit_store_review.test.mjs <repo root>
// REPO=<a checkout of page-store at cc4cce534> (default: the review worktree, removed after the review). FAILING tests show a finding.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const REPO = process.env.REPO || path.resolve(path.dirname(new URL(import.meta.url).pathname), "../..");
const STORE_JS = fs.readFileSync(path.join(REPO, "bridge-rs/assets/static/pagekit/store.js"), "utf8");
const TOOLS_RS = fs.readFileSync(path.join(REPO, "bridge-rs/src/agent_tools.rs"), "utf8");

/** A page with the kit's store, its door answered by `door(op) -> {status, body}`, time in our hands. */
function page(door, { android = false, activation = undefined } = {}) {
  let now = 1_000_000;
  let timers = [];
  let nextId = 1;
  const asked = [];
  const notes = [];
  const schedule = (fn, ms, every) => {
    const id = nextId++;
    timers.push({ id, at: now + (ms || 0), fn, every });
    return id;
  };
  const clear = (id) => { timers = timers.filter((t) => t.id !== id); };
  class FakeDate extends Date {
    constructor(...a) { super(...(a.length ? a : [now])); }
    static now() { return now; }
  }
  const note = { style: { cssText: "", display: "none" }, setAttribute() {}, _text: "" };
  Object.defineProperty(note, "textContent", {
    get() { return this._text; },
    set(v) { this._text = v; notes.push(v); },
  });
  const document = {
    documentElement: { lang: "fr" },
    body: { appendChild() {} },
    createElement: () => note,
    visibilityState: "visible",
    addEventListener() {},
  };
  const window = { Bip: {} };
  const context = {
    window, document, navigator: { userAgent: android ? "Android" : "iPhone", language: "fr", userActivation: activation },
    crypto: globalThis.crypto, Date: FakeDate, JSON, Promise, Object, Math, String, Array, Uint8Array,
    encodeURIComponent, decodeURIComponent,
    setTimeout: (fn, ms) => schedule(fn, ms, 0), clearTimeout: clear,
    setInterval: (fn, ms) => schedule(fn, ms, ms), clearInterval: clear,
    fetch: (url) => {
      const op = JSON.parse(decodeURIComponent(String(url).split("?op=")[1]));
      asked.push({ op, at: now });
      return Promise.resolve(door(op)).then((a) => ({
        status: a.status,
        text: () => Promise.resolve(JSON.stringify(a.body || {})),
      }));
    },
  };
  vm.createContext(context);
  vm.runInContext(STORE_JS, context);
  const flush = async () => { for (let i = 0; i < 20; i++) await new Promise((r) => setImmediate(r)); };
  const advance = async (ms) => {
    const end = now + ms;
    await flush();
    for (;;) {
      timers.sort((a, b) => a.at - b.at);
      const t = timers[0];
      if (!t || t.at > end) break;
      now = t.at;
      if (t.every) t.at = now + t.every; else timers.shift();
      t.fn();
      await flush();
    }
    now = end;
    await flush();
  };
  return { store: window.Bip.store, asked, notes, note, advance, now: () => now };
}

test("FINDING 2: a friend's line still waiting for the owner's bridge is not announced as lost after 10 min", async () => {
  // the friend's door: the sealed copy (no news while the owner's bridge is off), writes on their way (202)
  const view = { rev: 3, keys: {}, acts: [], write: true };
  const p = page((op) => (op.op === "get" ? { status: 200, body: view } : { status: 202, body: { queued: true } }));
  p.store.onChange(() => {});
  await p.advance(100);
  p.store.add("depenses", { what: "Glaces", cents: 900 });
  await p.advance(11 * 60 * 1000);   // the owner's laptop asleep: the action waits at the guichet, as it should
  // the action is still queued at the guichet and will be applied when the owner's bridge comes back: telling the
  // friend « Pas enregistré » and hiding the line makes them type it again (a double expense)
  assert.equal(p.store.items("depenses").length, 1, "the friend's line vanished from their page");
  assert.ok(!p.notes.includes("Pas enregistré"), `the page said: ${JSON.stringify(p.notes)}`);
});

test("FINDING 3: a store whose rev went back (page made again, store reset) is read again, not polled every 300 ms", async () => {
  let server = { rev: 0, keys: {}, acts: [] };
  const p = page((op) => {
    if (op.op === "get") return { status: 200, body: { ...server, write: true } };
    // a write answered at rev 5 (others wrote before)
    server = { rev: 5, keys: { k: { value: op.value, ver: 1 } }, acts: [op.a] };
    return { status: 200, body: { rev: 5, key: op.key, entry: server.keys.k, acts: server.acts } };
  });
  p.store.onChange(() => {});
  await p.advance(100);
  p.store.set("k", 1);
  await p.advance(1000);
  // the store starts again (the page deleted and made again under its id, a restore, a broken file read as empty)
  server = { rev: 1, keys: { other: { value: "nouveau", ver: 1 } }, acts: [] };
  const before = p.asked.length;
  await p.advance(10_000);
  const gets = p.asked.slice(before).filter((a) => a.op.op === "get").length;
  assert.equal(p.store.get("other"), "nouveau", "the page never takes the store as it now is");
  assert.ok(gets <= 4, `${gets} reads in 10 s (one every 300 ms, forever)`);
});

test("FINDING 12: a page opened while its store can't be reached says so (not an empty sheet in silence)", async () => {
  const p = page(() => ({ status: 503, body: { code: "offline" } }));
  p.store.onChange(() => {});
  await p.advance(5000);
  assert.equal(p.store.state, "offline");
  assert.ok(p.notes.length > 0 && p.note.style.display !== "none",
    "nothing tells the person the sheet isn't loaded: they see it empty and may type everything again");
});

test("FINDING 6: create_page's expense example reads French amounts right (whole cents)", () => {
  // (fixed 15/11: the example reads amounts with the kit's Bip.cents; adapted from `parseFloat(e.value…)`: the
  // example's own expression, evaluated with the kit as the page has it)
  const m = TOOLS_RS.match(/var c=([^;]*);/);
  assert.ok(m, "the example's parsing");
  const kit = { window: { Bip: {} }, document: { documentElement: { getAttribute() {}, setAttribute() {} }, readyState: "complete",
    querySelectorAll: () => [], addEventListener() {} }, navigator: { userAgent: "" }, getComputedStyle: () => ({ getPropertyValue: () => "" }) };
  vm.createContext(kit);
  vm.runInContext(fs.readFileSync(path.join(REPO, "bridge-rs/assets/static/pagekit/kit.js"), "utf8"), kit);
  const cents = (typed) => vm.runInNewContext(m[1], { e: { value: typed }, Bip: kit.window.Bip, Math, parseFloat });
  assert.equal(cents("42,50"), 4250);
  for (const [typed, want] of [["1 234,56", 123456], ["1\u202f234,56", 123456], ["1.234,56", 123456], ["1 200", 120000]]) {
    assert.equal(cents(typed), want, `« ${typed} € » kept as ${cents(typed)} cents`);
  }
});

// -- the fixes' own (15/11) ----------------------------------------------------------------------------------------

test("a friend's write refused for good by the owner's bridge comes back: the line leaves, « Pas enregistré »", async () => {
  let view = { rev: 1, gen: "g1", keys: {}, acts: [], refused: [], write: true };
  let sentAct = null;
  const p = page((op) => {
    if (op.op === "get") return { status: 200, body: view };
    sentAct = op.a;
    return { status: 202, body: { queued: true } };
  });
  p.store.onChange(() => {});
  await p.advance(100);
  p.store.add("depenses", { what: "Trop long", cents: 100 });
  await p.advance(1000);
  assert.equal(p.store.items("depenses").length, 1);
  view = { ...view, rev: 2, refused: [{ a: sentAct, code: "store_full" }] };   // the owner's bridge took it, refused
  await p.advance(6000);
  assert.equal(p.store.items("depenses").length, 0);
  assert.ok(p.notes.includes("Pas enregistré"), JSON.stringify(p.notes));
});

test("a friend's line waiting longer than 30 s says « En attente », and stays", async () => {
  const view = { rev: 1, gen: "g1", keys: {}, acts: [], refused: [], write: true };
  const p = page((op) => (op.op === "get" ? { status: 200, body: view } : { status: 202, body: { queued: true } }));
  p.store.onChange(() => {});
  await p.advance(100);
  p.store.add("depenses", { what: "Glaces", cents: 900 });
  await p.advance(40_000);
  assert.ok(p.notes.some((n) => n.startsWith("En attente")), JSON.stringify(p.notes));
  assert.equal(p.store.items("depenses").length, 1);
});

test("a new store generation (the page made again, a copy put back) is taken as it is, at once", async () => {
  let server = { rev: 7, gen: "old", keys: { k: { value: 1 } }, acts: [] };
  const p = page((op) => (op.op === "get" ? { status: 200, body: { ...server, write: true } } : { status: 500, body: {} }));
  p.store.onChange(() => {});
  await p.advance(100);
  assert.equal(p.store.get("k"), 1);
  server = { rev: 2, gen: "new", keys: { k: { value: 2 } }, acts: [] };
  const before = p.asked.length;
  await p.advance(5_100);
  assert.equal(p.store.get("k"), 2);
  assert.equal(p.asked.slice(before).filter((a) => a.op.op === "get").length, 1, "taken at the first read, no retry");
});

test("nothing is written before the person does anything on the page (no « who opened when »)", async () => {
  const writes = [];
  const p = page((op) => {
    if (op.op !== "get") writes.push(op);
    return op.op === "get" ? { status: 200, body: { rev: 0, gen: "", keys: {}, acts: [], write: true } }
      : { status: 200, body: { rev: 1, key: op.key, entry: { value: op.value }, acts: [op.a] } };
  }, { activation: { hasBeenActive: false, isActive: false } });
  await p.advance(100);
  assert.equal(await p.store.set("opened", Date.now()), false);
  await p.advance(1000);
  assert.deepEqual(writes, []);
});
