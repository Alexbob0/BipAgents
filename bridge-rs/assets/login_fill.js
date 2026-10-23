// The password fields of a sign-up form in one frame (login_fill.rs). Runs in an isolated world of its own, in each
// frame of the agent's page. It only ever looks at fields of type « password »: the one the agent focused and its
// confirmation on the same form. It never returns a value: only how many fields there are, and whether a field the
// bridge typed into holds what it typed (compared here, in the page).
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
