// Second independent review of `page-store` (15/11, after 3ee0bdcc8): the kit's Bip.store
// (bridge-rs/assets/static/pagekit/store.js) run in node against a fake door, with a fake clock (the harness of
// store_review.test.mjs). Run from the repo root: node --test bridge-rs/tests_kit/store_review2.test.mjs
// Each test FAILS on 3ee0bdcc8 and shows one finding.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const REPO = process.env.REPO || path.resolve(path.dirname(new URL(import.meta.url).pathname), "../..");
const STORE_JS = fs.readFileSync(path.join(REPO, "bridge-rs/assets/static/pagekit/store.js"), "utf8");

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

/** The friend's door as both apps build it (ReadShareThreadModel.pageStore, ViewerShares.pageStore): a read answers the
 *  page item's sealed store as the owner's bridge last sent it; a write is sealed to the guichet (202: on its way).
 *  (Fixed 15/11, adapted here as the apps now do it: the app keeps the friend's writes for the page until the store
 *  lists them in `acts` or `refused`, and gives them back with each read as `pending`.) `view` is that sealed store,
 *  unchanged while the owner's bridge has not taken it. */
function friendDoor(view, queued) {
  return (op) => {
    if (op.op === "get") {
      const done = new Set([...(view.acts || []), ...(view.refused || []).map((r) => r.a)]);
      return { status: 200, body: { ...view, pending: queued.filter((q) => !done.has(q.a)) } };
    }
    queued.push(op);
    return { status: 202, body: { queued: true, a: op.a } };
  };
}


test("FINDING R2-3: a friend's line still at the guichet is still shown when the friend opens the page again", async () => {
  // the owner's bridge is away (asleep, offline, a round not come yet): the sealed copy doesn't change
  const view = { rev: 3, gen: "g1", keys: { depenses: { items: [{ id: "o1", value: { what: "Essence", cents: 6000 }, by: "Alex", at: 1 }] } },
                 acts: [], refused: [], write: true };
  const queued = [];
  const first = page(friendDoor(view, queued));
  first.store.onChange(() => {});
  await first.advance(100);
  first.store.add("depenses", { what: "Glaces", cents: 900 });
  await first.advance(2000);
  assert.equal(queued.length, 1, "the line went to the guichet");
  assert.equal(first.store.items("depenses").length, 2, "shown while the page stays open");
  // Léa closes the page (back to the conversation, the sheet dismissed) and opens it again a minute later: a new web
  // view, a new kit; her line is still waiting at the guichet and will be applied when the owner's bridge comes back
  const again = page(friendDoor(view, queued));
  again.store.onChange(() => {});
  await again.advance(60_000);
  const shown = again.store.items("depenses").map((i) => i.value.what);
  // gone from her page and nothing says it is on its way: she types « Glaces » again, and both are applied (18 €)
  assert.ok(shown.includes("Glaces") || again.notes.some((n) => n.startsWith("En attente")),
    `her waiting line vanished on reopening, nothing said: ${JSON.stringify(shown)} ${JSON.stringify(again.notes)}`);
});
