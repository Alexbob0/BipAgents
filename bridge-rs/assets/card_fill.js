// The card form's fields in one frame (card_fill.rs). Runs in an isolated world of its own, in each frame of the
// agent's page, the payment providers' cross-origin frames included (Stripe Elements, Adyen, Checkout.com Frames,
// Mollie, PayPlug…): it finds the fields by their autocomplete tokens first (cc-number, cc-exp, cc-csc, cc-name…),
// then by their names, labels and placeholders in English, French, German and Spanish. It never reads or returns a
// value the person typed: only what kind of field each one is, and whether a field the bridge filled holds something.
(() => {
  if (globalThis.__bipPay) return;
  const AUTOCOMPLETE = {
    "cc-number": "number", "cc-exp": "exp", "cc-exp-month": "exp_month", "cc-exp-year": "exp_year", "cc-csc": "csc",
    "cc-name": "name", "cc-given-name": "given_name", "cc-family-name": "family_name",
  };
  // Order matters: the security code and the expiry's parts before the number (« card security code »).
  const RULES = [
    ["csc", /\b(cvc|cvv|cvv2|cvc2|csc|cvn|cid)\b|cvc|cvv|security.?code|securitycode|code.?de.?s[ée]curit|cryptogramme|\bcrypto\b|sicherheitscode|pr[üu]fnummer|c[oó]digo.?de.?seguridad/],
    ["exp_month", /(exp|expir|valid|ablauf|caduc|venc)[a-z]*.{0,12}(month|mois|monat|\bmes\b)|(month|mois|monat)[a-z]*.{0,12}(exp|expir)|exp.?month|expirymonth|\bexp.?mm\b/],
    ["exp_year", /(exp|expir|valid|ablauf|caduc|venc)[a-z]*.{0,12}(year|ann[ée]e|jahr|a[nñ]o)|(year|ann[ée]e|jahr)[a-z]*.{0,12}(exp|expir)|exp.?year|expiryyear|\bexp.?yy(yy)?\b/],
    ["exp", /exp.?date|expir|date.?d.?exp|date.?de.?validit|validit[ée]|valid.?thru|mm.?\/.?(yy|aa|jj)|ablaufdatum|g[üu]ltig|caducidad|vencimiento/],
    ["name", /card.?holder|cardholder|name.?on.?(the.?)?card|nameoncard|cc.?name|holder.?name|titulaire|porteur|nom.?sur.?la.?carte|karteninhaber|titular/],
    ["number", /card.?num|cardnum|cc.?num|ccnum|card.?no\b|kartennummer|num[ée]ro.?de.?(la.?)?carte|num.?(de.?)?carte|n[uú]mero.?de.?(la.?)?tarjeta|\bpan\b/],
  ];
  const LOOSE = [
    ["exp_month", /\b(month|mois|monat|mes|mm)\b/],
    ["exp_year", /\b(year|ann[ée]e|jahr|a[nñ]o|yy|yyyy|aa|aaaa)\b/],
    ["postal", /postal|post.?code|\bzip\b|code.?postal|\bplz\b|c[oó]digo.?postal/],
  ];
  const TEXT_TYPES = new Set(["", "text", "tel", "number", "password", "search", "email"]);

  const usable = (el) => {
    if (el.disabled || el.readOnly) return false;
    if (el.tagName === "INPUT" && !TEXT_TYPES.has((el.getAttribute("type") || "").toLowerCase())) return false;
    if (el.getClientRects().length === 0) return false;
    const style = getComputedStyle(el);
    return style.visibility !== "hidden" && style.display !== "none";
  };
  const labelOf = (el) => {
    const parts = [];
    if (el.labels) for (const l of el.labels) parts.push(l.textContent || "");
    const by = el.getAttribute("aria-labelledby");
    if (by) for (const id of by.split(/\s+/)) {
      const ref = el.ownerDocument.getElementById(id);
      if (ref) parts.push(ref.textContent || "");
    }
    return parts.join(" ");
  };
  const haystack = (el) => {
    const parts = [el.name, el.id, el.getAttribute("placeholder"), el.getAttribute("aria-label"), el.getAttribute("title"),
                   labelOf(el)];
    for (const a of el.attributes) if (a.name.startsWith("data-")) parts.push(a.value);
    return parts.filter(Boolean).join(" ").toLowerCase().slice(0, 600);
  };
  const tokens = (el) => (el.getAttribute("autocomplete") || "").toLowerCase().split(/\s+/).filter(Boolean);
  const elements = (root, out) => {
    for (const el of root.querySelectorAll("input, select, textarea, *")) {
      if (el.tagName === "INPUT" || el.tagName === "SELECT") out.push(el);
      if (el.shadowRoot) elements(el.shadowRoot, out);
    }
    return out;
  };
  const optionNumbers = (el) => Array.from(el.options || []).map((o) =>
    parseInt(((o.value || "") + " " + (o.text || "")).replace(/^\D+/, ""), 10)).filter((n) => !isNaN(n));

  const api = {
    els: [], kinds: [], button: null,
    detect() {
      this.els = []; this.kinds = [];
      const found = [];
      const all = elements(document, []).filter(usable);
      const strict = all.map((el) => {
        const t = tokens(el);
        for (const k of t) if (AUTOCOMPLETE[k]) return AUTOCOMPLETE[k];
        if (t.includes("postal-code") && t.includes("billing")) return "postal";
        const hay = haystack(el);
        for (const [kind, rule] of RULES) if (rule.test(hay)) return kind;
        return null;
      });
      // A frame with a card number or code: its other fields are the card's (the expiry's month and year, a postcode).
      const cardFrame = strict.includes("number") || strict.includes("csc");
      all.forEach((el, i) => {
        let kind = strict[i];
        if (!kind && cardFrame) {
          const hay = haystack(el);
          for (const [k, rule] of LOOSE) if (rule.test(hay)) { kind = k; break; }
          if (!kind && el.tagName === "SELECT") {
            const n = optionNumbers(el);
            const year = new Date().getFullYear();
            if (n.length >= 12 && n.filter((x) => x >= 1 && x <= 12).length >= 12 && Math.max(...n) <= 12) kind = "exp_month";
            else if (n.filter((x) => x >= year && x <= year + 30).length >= 5) kind = "exp_year";
          }
        }
        if (!kind || found.some((f) => f.kind === kind)) return;
        const hint = ((el.getAttribute("placeholder") || "") + " " + (el.getAttribute("aria-label") || "")).toLowerCase();
        const max = el.maxLength > 0 ? el.maxLength : 0;
        let fmt = "";
        if (kind === "exp") fmt = /yyyy|aaaa|jjjj/.test(hint) || max === 7 ? "MM/YYYY" : max === 4 ? "MMYY" : "MM/YY";
        if (kind === "exp_year") fmt = /yyyy|aaaa|jjjj/.test(hint) || max >= 4 ? "YYYY" : "YY";
        if (kind === "exp_year" && el.tagName === "SELECT") fmt = "";
        this.els.push(el); this.kinds.push(kind);
        found.push({i: this.els.length - 1, kind, tag: el.tagName.toLowerCase(), fmt,
                    empty: el.tagName === "SELECT" ? false : !el.value});
      });
      return found;
    },
    el(i) { return this.els[i]; },
    // Ready for typing: focused, what it held selected (typing replaces it).
    prepare(i) {
      const el = this.els[i];
      el.scrollIntoView({block: "center"});
      el.focus();
      try { el.select(); } catch (e) { /* type=number */ }
      return el.ownerDocument.activeElement === el || (el.getRootNode() && el.getRootNode().activeElement === el);
    },
    // The keyboard's focus is in this field (through shadow roots) and in this frame (the page's focused frame is
    // this one: an input holds no frame): what Input.insertText types goes into it, nowhere else.
    focused(i) {
      const el = this.els[i];
      let active = el.ownerDocument.activeElement;
      while (active && active.shadowRoot && active.shadowRoot.activeElement) active = active.shadowRoot.activeElement;
      return active === el && el.ownerDocument.hasFocus();
    },
    // What the field holds, reduced to compare with what was typed (digits only for numbers and dates).
    holds(i, digitsOnly) {
      const v = this.els[i].value || "";
      return digitsOnly ? v.replace(/\D/g, "") : v.trim().toLowerCase();
    },
    // Typing did not reach it: the value set the way the page's framework sees it.
    set(i, value) {
      const el = this.els[i];
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      el.focus();
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, value);
      el.dispatchEvent(new Event("input", {bubbles: true}));
      el.dispatchEvent(new Event("change", {bubbles: true}));
      el.dispatchEvent(new Event("blur"));
      return true;
    },
    choose(i, month, year) {
      const el = this.els[i];
      const wanted = this.kinds[i] === "exp_month" ? [month] : [year, year % 100];
      for (const o of el.options) {
        const byValue = parseInt((o.value || "").replace(/^\D+/, ""), 10);
        const byText = parseInt((o.text || "").replace(/^\D+/, ""), 10);
        if (wanted.includes(byValue) || wanted.includes(byText)) {
          el.value = o.value;
          el.dispatchEvent(new Event("input", {bubbles: true}));
          el.dispatchEvent(new Event("change", {bubbles: true}));
          return true;
        }
      }
      return false;
    },
    blur(i) { this.els[i].dispatchEvent(new Event("blur")); this.els[i].blur(); return true; },
    // The elements of this document around which the card's form is: the iframes holding its fields (marked by the
    // bridge), besides the card fields `detect` found here.
    anchors: [],
    anchor(el) { if (!this.anchors.includes(el)) this.anchors.push(el); return true; },
    // The card form's pay button: a button (never a link) of the form holding the card's fields (`detect`'s, and the
    // iframes `anchor` marked), else of the nearest part of the page around them that holds one; the one whose text
    // holds `wanted` (as the agent read it), else a « pay » word, the amount's digits making a better match. Its
    // text, or null (no card field here, or no such button).
    findButton(wanted, amountDigits) {
      const PAY = /\b(pay|payer|paiement|bezahlen|zahlen|kaufen|pagar|comprar|confirm|confirmer|valider|commander|order|acheter|buy|checkout)\b/i;
      const norm = (t) => (t || "").toLowerCase().replace(/\s+/g, " ").trim();
      const all = [];
      const control = (el) => el.matches("button, input[type=submit], input[type=button], input[type=image], [role=button]")
        && !el.matches("a, button[type=reset]") && !el.closest("a[href], [role=link]");
      const walk = (root) => {
        for (const el of root.querySelectorAll("*")) {
          if (control(el)) all.push(el);
          if (el.shadowRoot) walk(el.shadowRoot);
        }
      };
      walk(document);
      const anchors = this.anchors.concat(this.els);
      const parent = (el) => el.parentElement || (el.getRootNode && el.getRootNode().host) || null;
      const inside = (outer, el) => {
        for (let up = el; up; up = parent(up)) if (up === outer) return true;
        return false;
      };
      let pool = [];
      for (const a of anchors) {
        const form = a.form || a.closest("form");
        if (form) {
          pool = pool.concat(all.filter((el) => el.form === form || inside(form, el)));
          continue;
        }
        for (let up = parent(a); up; up = parent(up)) {   // no form: the nearest element around it with a button
          const near = all.filter((el) => inside(up, el));
          if (near.length) { pool = pool.concat(near); break; }
        }
      }
      let best = null, bestScore = 0;
      for (const el of pool) {
        if (el.disabled || el.getClientRects().length === 0) continue;
        const text = norm(el.innerText || el.value || el.getAttribute("aria-label") || "");
        if (!text || text.length > 120) continue;
        let score = 0;
        if (wanted && text.includes(norm(wanted))) score += 3;
        if (PAY.test(text)) score += 1;
        if (score && amountDigits && text.replace(/\D/g, "").includes(amountDigits)) score += 1;
        if (score > bestScore) { best = el; bestScore = score; }
      }
      this.button = best;
      return best ? norm(best.innerText || best.value || best.getAttribute("aria-label")).slice(0, 80) : null;
    },
    clickButton() {
      if (!this.button) return false;
      this.button.scrollIntoView({block: "center"});
      this.button.click();
      return true;
    },
    // The bank asks the person (3-D Secure, « confirm in your banking app »).
    bankAsks() {
      const text = (document.body && document.body.innerText || "").slice(0, 20000);
      return /3-?d ?secure|banking app|app bancaire|application bancaire|banking-app|banca m[oó]vil|confirm.{0,40}\b(app|application)\b|valide[rz]?.{0,40}\b(app|application)\b/i.test(text);
    },
  };
  globalThis.__bipPay = api;
})();
