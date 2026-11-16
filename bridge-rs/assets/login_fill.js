// The password fields of a sign-up form in one frame (login_fill.rs), and a login form's password and identifier
// fields (`detectLogin`). Runs in an isolated world of its own, in each frame of the agent's page. For a sign-up it only
// ever looks at fields of type « password »: the one the agent focused and its confirmation on the same form. It never
// returns a value: only how many fields there are, and whether a field the bridge typed into holds what it typed
// (compared here, in the page).
(() => {
  if (globalThis.__bipLogin) return;
  const isPassword = (el) => !!el && el.tagName === "INPUT" && (el.getAttribute("type") || "").toLowerCase() === "password";
  const usable = (el) => {
    if (!isPassword(el) || el.disabled || el.readOnly) return false;
    if (el.getClientRects().length === 0) return false;
    const style = getComputedStyle(el);
    return style.visibility !== "hidden" && style.display !== "none";
  };
  const passwords = (root, out) => {
    for (const el of root.querySelectorAll("*")) {
      if (isPassword(el)) out.push(el);
      if (el.shadowRoot) passwords(el.shadowRoot, out);
    }
    return out;
  };
  const focusedIn = (doc) => {
    let el = doc.activeElement;
    while (el && el.shadowRoot && el.shadowRoot.activeElement) el = el.shadowRoot.activeElement;
    return el;
  };
  const formOf = (el) => el.form || el.closest("form");
  const current = (el) => (el.getAttribute("autocomplete") || "").toLowerCase().split(/\s+/).includes("current-password");
  const fresh = (el) => (el.getAttribute("autocomplete") || "").toLowerCase().split(/\s+/).includes("new-password");
  // The field and the password fields after it on its form (its confirmation), never one asking the current
  // password; two at most.
  const group = (first, all) => {
    const form = formOf(first);
    const scope = form ? all.filter((e) => formOf(e) === form) : (all.length <= 2 ? all : [first]);
    const after = scope.slice(Math.max(scope.indexOf(first), 0)).filter((e) => e === first || !current(e));
    return after.slice(0, 2);
  };
  const P = (globalThis.__bipLogin = { els: [] });
  // {focused, count, fields}: `focused` when the agent's focus is on a password field of this frame; otherwise, a
  // form whose password fields say they want a new password (or the page's only password fields) as a fallback.
  P.detect = () => {
    const all = passwords(document, []).filter(usable);
    const focused = focusedIn(document);
    let els = [];
    let isFocused = false;
    if (usable(focused) && all.includes(focused)) {
      els = group(focused, all);
      isFocused = true;
    } else {
      const wanting = all.filter(fresh);
      if (wanting.length) els = group(wanting[0], all);
      else if (all.length && all.length <= 2 && all.every((e) => formOf(e) === formOf(all[0])) && !all.some(current)) els = all;
    }
    P.els = els;
    return { focused: isFocused, count: all.length, fields: els.map((e, i) => ({ i, type: "password", empty: !e.value })) };
  };
  // -- logging in with a saved login (`vault_login`, login_fill.rs `find_login`) --------------------------------------
  // The form's current-password field (never one asking for a new password or its confirmation, nor a code) and its
  // identifier field: a text, e-mail or phone input before it on its form that says it is one (autocomplete username or
  // email, type email, a name, id, label or placeholder like login, user, e-mail, identifiant), else the only such input
  // just before it. A first step that asks for the identifier alone: that field, when it says it is one.
  const words = (el) => [el.getAttribute("autocomplete"), el.name, el.id, el.getAttribute("placeholder"),
    el.getAttribute("aria-label"), el.getAttribute("title"), ...[...(el.labels || [])].map((l) => l.textContent)]
    .filter(Boolean).join(" ").normalize("NFKD").replace(/[\u0300-\u036f]/g, "").toLowerCase();
  const NOT_CURRENT = /\b(new|nouveau|nouvel|confirm\w*|repeat|retype|create|again|verif\w*|neues|wiederholen)\b|one-time-code|\b(otp|totp|2fa|mfa|sms)\b/;
  const current_ = (el) => usable(el) && !fresh(el) && !NOT_CURRENT.test(words(el));
  const TEXTISH = ["text", "email", "tel", ""];
  const isText = (el) => !!el && el.tagName === "INPUT" && TEXTISH.includes((el.getAttribute("type") || "").toLowerCase());
  const shown = (el) => {
    if (!isText(el) || el.disabled || el.readOnly || el.getClientRects().length === 0) return false;
    const style = getComputedStyle(el);
    return style.visibility !== "hidden" && style.display !== "none";
  };
  const NOT_IDENTIFIER = /\bcc-|card|carte|cvc|cvv|search|recherche|captcha|\b(otp|sms)\b|one-time-code|\b(zip|postal)\b/;
  const IDENTIFIER = /\b(user\w*|login|log-in|identifi\w*|ident|e-?mail|mail|courriel|account|compte|member\w*|adherent|client|benutzer\w*|usuario|nom d.utilisateur)\b/;
  const scoreOf = (el) => {
    const w = words(el);
    if (NOT_IDENTIFIER.test(w)) return -1;
    const ac = (el.getAttribute("autocomplete") || "").toLowerCase().split(/\s+/);
    if (ac.includes("username") || ac.includes("email")) return 100;
    if ((el.getAttribute("type") || "").toLowerCase() === "email") return 85;
    return IDENTIFIER.test(w) ? 70 : 0;
  };
  const texts = (root, out) => {
    for (const el of root.querySelectorAll("*")) {
      if (shown(el)) out.push(el);
      if (el.shadowRoot) texts(el.shadowRoot, out);
    }
    return out;
  };
  const before = (a, b) => !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
  const identifierFor = (pw, all) => {
    const form = pw ? formOf(pw) : null;
    const scope = all.filter((e) => (pw ? (form ? formOf(e) === form : true) && before(e, pw) : true));
    const scored = scope.map((e) => [e, scoreOf(e)]).filter(([, s]) => s > 0);
    if (scored.length) {
      const top = Math.max(...scored.map(([, s]) => s));
      const best = scored.filter(([, s]) => s === top).map(([e]) => e);
      // beside a password field: the nearest before it; alone: only when there's one
      if (pw) return best[best.length - 1];
      return best.length === 1 ? best[0] : null;
    }
    if (!pw) return null;
    const plain = scope.filter((e) => scoreOf(e) === 0);
    return plain.length === 1 ? plain[0] : null;
  };
  // {focused, password, identifier}: whether this frame has a current-password field (the focused one, else its only
  // one, else its only one asking for the current password), and an identifier field for it.
  P.detectLogin = () => {
    const all = passwords(document, []).filter(current_);
    const focused = focusedIn(document);
    let pw = null;
    let isFocused = false;
    if (focused && all.includes(focused)) {
      pw = focused;
      isFocused = true;
    } else if (all.length === 1) {
      pw = all[0];
    } else if (all.filter(current).length === 1) {
      pw = all.filter(current)[0];
    }
    // a first step asking for the identifier alone: a page with no password field at all (a sign-up form's new
    // password is one: never a login's first step)
    const alone = !pw && !passwords(document, []).some(usable);
    let user = (pw || alone) ? identifierFor(pw, texts(document, [])) : null;
    // the field the agent clicked into, when it can be this form's identifier
    if (shown(focused) && scoreOf(focused) >= 0 && (pw ? formOf(focused) === formOf(pw) && before(focused, pw) : alone)) {
      user = focused;
    }
    if (!pw && user && focused === user) isFocused = true;
    P.els = pw ? [pw] : [];
    P.user = user;
    return { focused: isFocused, password: !!pw, identifier: !!user, count: all.length };
  };
  // The identifier field focused, emptied unless it holds `value` already (then nothing is typed: false).
  P.prepareUser = (value) => {
    const el = P.user;
    if (!shown(el)) return false;
    el.scrollIntoView({ block: "center" });
    el.focus();
    if (el.value === value) return false;
    if (el.value) P.setUser("");
    return true;
  };
  P.setUser = (value) => {
    const el = P.user;
    if (!isText(el)) return false;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  };
  P.userEl = () => P.user;
  P.el = (i) => P.els[i];
  // Focused and emptied before typing; false when the field is no longer a password field.
  P.prepare = (i) => {
    const el = P.els[i];
    if (!usable(el)) return false;
    el.scrollIntoView({ block: "center" });
    el.focus();
    if (el.value) P.set(i, "");
    return true;
  };
  P.set = (i, value) => {
    const el = P.els[i];
    if (!isPassword(el)) return false;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  };
  P.blur = (i) => {
    const el = P.els[i];
    if (el) {
      el.dispatchEvent(new Event("change", { bubbles: true }));
      el.blur();
    }
    return true;
  };
})();
