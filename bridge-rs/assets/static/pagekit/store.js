/* BipAgents page kit, its store (pages.rs puts it inline right after kit.js, only in a page that uses Bip.store):
   what is typed in the page, kept by the bridge beside it (bridge page_store.rs). */
(function () {
  "use strict";
  if (!window.Bip || window.Bip.store) return;
  var doc = document, root = doc.documentElement;
  var lang = function () { return root.lang || navigator.language || "en"; };
  var ANDROID = /Android/i.test(navigator.userAgent || "");
  // -- Pages that remember (bridge page_store.rs, 15/11) -----------------------------------------------------------
  // Bip.store: what is typed in the page, kept by the bridge beside it and seen by everyone who sees the page. Its
  // one door is answered by the app itself (`bipstore:` on the iPhone, `https://bipstore.invalid` on Android): the
  // app adds which page it shows, the page never names one. A write shows at once (the page's own copy on top of
  // what the bridge has), goes in order with an id of its own, and is sent again until the bridge lists that id
  // (`acts`: never counted twice); the page asks for news every few seconds while it is on screen. Outside the apps
  // (an export, a browser) there is no door: the page works in memory and says so at the first write.
  var store = (function () {
    var DOOR = ANDROID ? "https://bipstore.invalid/" : "bipstore://store/";
    var POLL = 5000, WAITING = 30 * 1000, VALUE_MAX = 32 * 1024;
    var KEY = /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,63}$/;
    var server = { rev: -1, keys: {}, acts: [], gen: "", refused: [] };
    var pending = [];
    var listeners = [];
    var mode = "idle", canWrite = true, loaded = false, sending = false, asking = false, again = false, started = false;
    var newest = -1;   // the latest rev a write of this page was answered with
    var stale = 0;     // reads in a row that came back older than that (once: asked again; twice: a new store)
    var cache = null, note = null, noteTimer = null, retry = null;
    var readyResolve;
    var ready = new Promise(function (resolve) { readyResolve = resolve; });
    var WORDS = {
      offline: { fr: "Pas encore enregistré — nouvel essai…", en: "Not saved yet — trying again…",
        de: "Noch nicht gespeichert — neuer Versuch…", es: "Aún no guardado — reintentando…", nl: "Nog niet bewaard — opnieuw…" },
      local: { fr: "Cette page ne garde pas ce que tu tapes ici", en: "This page doesn't keep what you type here",
        de: "Diese Seite behält nicht, was du hier eingibst", es: "Esta página no guarda lo que escribes aquí",
        nl: "Deze pagina bewaart niet wat je hier typt" },
      readonly: { fr: "Lecture seule : rien n'est enregistré", en: "Read only: nothing is saved",
        de: "Nur lesen: nichts wird gespeichert", es: "Solo lectura: no se guarda nada", nl: "Alleen lezen: niets wordt bewaard" },
      refused: { fr: "Pas enregistré", en: "Not saved", de: "Nicht gespeichert", es: "No guardado", nl: "Niet bewaard" },
      unloaded: { fr: "Hors ligne : données pas encore chargées", en: "Offline: the data isn't loaded yet",
        de: "Offline: Daten noch nicht geladen", es: "Sin conexión: datos aún no cargados", nl: "Offline: gegevens nog niet geladen" },
      waiting: { fr: "En attente : sera enregistré au prochain passage", en: "Waiting: saved at the next round",
        de: "Wartet: wird beim nächsten Durchgang gespeichert", es: "En espera: se guardará en la próxima ronda",
        nl: "Wacht: wordt bij de volgende ronde bewaard" },
    };
    var word = function (key) { var w = WORDS[key]; return w[lang().slice(0, 2).toLowerCase()] || w.en; };
    var copy = function (v) { return v === undefined ? undefined : JSON.parse(JSON.stringify(v)); };
    var same = function (a, b) { return JSON.stringify(a) === JSON.stringify(b); };

    function newId() {
      var out = "";
      if (window.crypto && crypto.getRandomValues) {
        var bytes = new Uint8Array(12);
        crypto.getRandomValues(bytes);
        for (var i = 0; i < bytes.length; i++) out += (bytes[i] % 36).toString(36);
      } else {
        out = Math.random().toString(36).slice(2, 14);
      }
      return out + Date.now().toString(36);
    }

    // The small line at the bottom: only when something isn't saved.
    function say(kind) {
      if (!doc.body) return;
      if (!note) {
        note = doc.createElement("div");
        note.id = "bip-store-note";
        note.setAttribute("role", "status");
        note.style.cssText = "position:fixed!important;left:50%!important;bottom:12px!important;transform:translateX(-50%)!important;" +
          "z-index:2147483647!important;max-width:86vw!important;padding:6px 14px!important;border-radius:999px!important;" +
          "background:rgba(60,60,67,.9)!important;color:#fff!important;font:600 12.5px/1.35 -apple-system,system-ui,sans-serif!important;" +
          "text-align:center!important;pointer-events:none!important;display:none";
        doc.body.appendChild(note);
      }
      clearTimeout(noteTimer);
      if (!kind) { note.style.display = "none"; return; }
      note.textContent = word(kind);
      note.style.display = "block";
      if (kind === "refused" || kind === "local") noteTimer = setTimeout(function () { say(null); }, 4000);
    }

    function setMode(next) {
      mode = next;
      if (next === "offline") {
        clearTimeout(noteTimer);
        noteTimer = setTimeout(function () { if (mode === "offline" && unsent()) say("offline"); }, 2000);
      } else if (next === "saved" && note && (note.textContent === word("offline") || note.textContent === word("unloaded"))) {
        say(null);
      }
    }

    function unsent() { for (var i = 0; i < pending.length; i++) if (!pending[i].sent) return true; return false; }

    // What the page sees: the bridge's keys, then its own writes not in them yet.
    function current() {
      if (cache) return cache;
      var keys = {};
      for (var k in server.keys) if (Object.prototype.hasOwnProperty.call(server.keys, k)) keys[k] = server.keys[k];
      pending.forEach(function (w) {
        var entry = keys[w.key];
        if (w.op === "set") { keys[w.key] = { value: w.value, at: w.at / 1000, by: "" }; return; }
        if (w.op === "del") { delete keys[w.key]; return; }
        if (entry && !entry.items) return;   // a value, not lines: the bridge will say no
        var items = entry ? entry.items.slice() : [];
        if (w.op === "add") {
          if (!items.some(function (i) { return i.id === w.id; })) items.push({ id: w.id, value: w.value, by: "", at: w.at / 1000 });
        } else if (w.op === "edit") {
          items = items.map(function (i) { return i.id === w.id ? { id: i.id, value: w.value, by: i.by, at: w.at / 1000 } : i; });
        } else if (w.op === "drop") {
          items = items.filter(function (i) { return i.id !== w.id; });
        }
        keys[w.key] = { items: items, ver: entry && entry.ver, at: w.at / 1000, by: entry && entry.by };
      });
      cache = keys;
      return keys;
    }

    function tell(keys, mine) {
      if (!loaded || !keys.length) return;
      listeners.forEach(function (cb) {
        try { cb({ keys: keys.slice(), mine: !!mine }); } catch (e) { setTimeout(function () { throw e; }); }
      });
    }

    // After any change: what the page sees again, and who listens told which keys changed.
    function changed(update, mine) {
      var before = current();
      update();
      cache = null;
      var after = current(), keys = [];
      var all = {};
      for (var a in before) all[a] = 1;
      for (var b in after) all[b] = 1;
      for (var k in all) if (!same(before[k], after[k])) keys.push(k);
      tell(keys, mine);
    }

    // The writes the store now lists (`acts`) or refused for good (`refused`: a friend's, after the owner's bridge
    // took it) leave the page's own copy. A friend's write on its way stays shown, « en attente », as long as it takes
    // (the owner's bridge may be asleep: it is still at the guichet, never announced lost).
    function prune() {
      var acts = server.acts || [], refused = (server.refused || []).map(function (r) { return r && r.a; });
      var now = Date.now(), lost = false, waiting = false;
      pending = pending.filter(function (w) {
        if (acts.indexOf(w.a) >= 0) return false;
        if (refused.indexOf(w.a) >= 0) { lost = true; if (w.done) w.done(false); return false; }
        if (w.sent && now - w.at > WAITING) waiting = true;
        return true;
      });
      if (lost) say("refused");
      else if (waiting) say("waiting");
      else if (note && note.textContent === word("waiting")) say(null);
    }

    function ask(op) {
      return fetch(DOOR + "?op=" + encodeURIComponent(JSON.stringify(op)), { cache: "no-store", credentials: "omit" })
        .then(function (r) {
          return r.text().then(function (t) {
            var body = null;
            try { body = JSON.parse(t); } catch (e) {}
            return { status: r.status, body: body || {} };
          });
        });
    }

    function first() {
      if (loaded) return;
      loaded = true;
      readyResolve(mode);
      if (mode === "offline") say("unloaded");   // an empty sheet that is only not loaded yet: said, never silent
      // writes made before the first answer: kept in memory only (no door), or dropped (read only)
      if (mode === "local" && pending.length) say("local");
      if (mode === "local" || !canWrite) pending.forEach(function (w) { if (w.done) w.done(false); });
      if (!canWrite && pending.length) { pending = []; cache = null; say("readonly"); }
      var keys = Object.keys(current());
      listeners.forEach(function (cb) { try { cb({ keys: keys, mine: false, first: true }); } catch (e) { setTimeout(function () { throw e; }); } });
    }

    function later() {
      if (retry) return;
      retry = setTimeout(function () { retry = null; pump(); }, 3000);
    }

    function poll() {
      if (mode === "local") return;
      if (asking) { again = true; return; }
      asking = true;
      var op = { op: "get" };
      if (server.rev >= 0) op.since = server.rev;
      ask(op).then(function (r) {
        asking = false;
        var b = r.body;
        if (r.status === 200 && typeof b.rev === "number") {
          if (b.write === false) canWrite = false;
          var reset = server.gen && b.gen && b.gen !== server.gen;   // a new store (the page made again, put back)
          if (!reset && b.rev < newest && stale < 1) { stale++; again = true; }   // asked before this page's write was in
          else if (!b.same && b.keys) {
            stale = 0;
            if (reset || b.rev < newest) newest = b.rev;   // older for good: the store started again, taken as it is
            changed(function () { server = { rev: b.rev, keys: b.keys, acts: b.acts || [], gen: b.gen || "", refused: b.refused || [] }; prune(); });
          } else stale = 0;
          setMode(!canWrite ? "readonly" : unsent() ? "offline" : "saved");
          first();
          pump();
        } else if (r.status === 404) {
          mode = "local";   // no store for this page here (the app's snapshot, a page it can't name)
          first();
        } else {
          if (!loaded) { mode = "offline"; first(); }
          setMode("offline");
        }
        if (again) { again = false; setTimeout(poll, 300); }
      }, function () {
        asking = false;
        if (!loaded) { mode = "local"; first(); } else setMode("offline");
      });
    }

    function pump() {
      if (sending || mode === "local" || mode === "readonly" || !loaded) return;
      var w = null;
      for (var i = 0; i < pending.length; i++) if (!pending[i].sent) { w = pending[i]; break; }
      if (!w) return;
      sending = true;
      var op = { op: w.op, key: w.key, a: w.a };
      if ("value" in w) op.value = w.value;
      if (w.id) op.id = w.id;
      ask(op).then(function (r) {
        sending = false;
        var b = r.body;
        if (r.status === 200 && "entry" in b) {
          changed(function () {
            if (b.entry === null) delete server.keys[b.key]; else server.keys[b.key] = b.entry;
            if (b.rev === server.rev + 1 && (!b.gen || !server.gen || b.gen === server.gen)) { server.rev = b.rev; if (b.gen) server.gen = b.gen; }
            else setTimeout(poll);   // others wrote meantime, or a new store: all of it again (the rev kept, so it all comes)
            newest = Math.max(newest, b.rev);
            if (b.acts) server.acts = b.acts;
            if (pending.indexOf(w) >= 0 && server.acts.indexOf(w.a) < 0) pending.splice(pending.indexOf(w), 1);
            prune();
          });
          if (w.done) w.done(true);
          setMode(unsent() ? "offline" : "saved");
        } else if (r.status === 202) {
          w.sent = true;   // a friend's: on its way, back with the page's next version
          if (w.done) w.done(true);
          setMode(unsent() ? "offline" : "saved");
        } else if (r.status === 429 || r.status >= 500 || r.status === 0) {
          setMode("offline");
          later();
          return;
        } else {
          changed(function () { pending.splice(pending.indexOf(w), 1); });
          if (b.code === "shared_read_only") { canWrite = false; setMode("readonly"); say("readonly"); }
          else if (!(w.op === "drop" && b.code === "no_line")) say("refused");
          if (w.done) w.done(false);
        }
        pump();
      }, function () {
        sending = false;
        setMode("offline");
        later();
      });
    }

    function start() {
      if (started) return;
      started = true;
      mode = "loading";
      poll();
      setInterval(function () { if (doc.visibilityState !== "hidden") { poll(); pump(); } }, POLL);
      doc.addEventListener("visibilitychange", function () { if (doc.visibilityState !== "hidden") poll(); });
    }

    // A write: its promise says true once the bridge has it (a friend's: once it is on its way), false when it
    // can't be kept (refused, too big, read only, no door): the page never has to catch anything.
    function write(w) {
      start();
      if (!KEY.test(String(w.key))) { say("refused"); return Promise.resolve(false); }
      if ("value" in w) {
        var text = JSON.stringify(w.value);
        if (text === undefined || text.length > VALUE_MAX) { say("refused"); return Promise.resolve(false); }
        w.value = JSON.parse(text);
      }
      if (!canWrite) { say("readonly"); return Promise.resolve(false); }
      // only what the person does is written (a tap, typing): never on its own when the page opens — a page can't
      // note who opened it when (review of 15/11). The person's first touch on the page opens it for good.
      var ua = navigator.userActivation;
      if (ua && !ua.hasBeenActive) {
        if (window.console) console.warn("Bip.store: not saved, written before the person did anything on the page");
        return Promise.resolve(false);
      }
      w.a = newId();
      w.at = Date.now();
      var done = new Promise(function (resolve) { w.done = resolve; });
      changed(function () { pending.push(w); }, true);
      if (mode === "local") { say("local"); w.done(false); } else pump();
      return done;
    }

    function entry(key) { start(); return current()[key]; }

    return {
      get: function (key) {
        var e = entry(key);
        if (!e) return undefined;
        return e.items ? e.items.map(function (i) { return copy(i.value); }) : copy(e.value);
      },
      items: function (key) {
        var e = entry(key);
        return e && e.items ? e.items.map(function (i) { return { id: i.id, value: copy(i.value), by: i.by || "", at: i.at }; }) : [];
      },
      list: function () {
        start();
        var all = current(), out = {};
        for (var k in all) out[k] = all[k].items ? all[k].items.map(function (i) { return copy(i.value); }) : copy(all[k].value);
        return out;
      },
      keys: function () { start(); return Object.keys(current()); },
      set: function (key, value) { return write({ op: "set", key: key, value: value }); },
      del: function (key) { return write({ op: "del", key: key }); },
      add: function (key, value) {
        var w = { op: "add", key: key, id: newId(), value: value };
        write(w);
        return w.id;
      },
      edit: function (key, id, value) { return write({ op: "edit", key: key, id: String(id), value: value }); },
      drop: function (key, id) { return write({ op: "drop", key: key, id: String(id) }); },
      onChange: function (cb) {
        start();
        if (typeof cb !== "function") return;
        listeners.push(cb);
        if (loaded) setTimeout(function () { try { cb({ keys: Object.keys(current()), mine: false, first: true }); } catch (e) { throw e; } });
      },
      get ready() { start(); return ready; },
      get state() { return mode; },
      get canWrite() { return canWrite; },
    };
  })();

  window.Bip.store = store;
})();
