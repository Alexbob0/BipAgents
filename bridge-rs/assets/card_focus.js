// Whether the field the person is typing into, in the agent's page held from the app (desktop_page.rs), is a payment
// card's number, expiry or security code. Only on the offer's places: no card is typed into an agent's browser there.
// It reads what kind of field has the focus (autocomplete tokens, names, labels, placeholders in English, French,
// German and Spanish), never a value. A payment provider's frame (Stripe Elements, Adyen, Braintree, Checkout.com…)
// with the focus counts as a card field: its fields can't be read from the page.
(() => {
  let el = document.activeElement;
  while (el && el.shadowRoot && el.shadowRoot.activeElement) el = el.shadowRoot.activeElement;
  if (!el) return false;
  const low = (parts) => parts.filter(Boolean).join(" ").toLowerCase().slice(0, 600);
  if (el.tagName === "IFRAME") {
    const hay = low([el.name, el.title, el.id, el.className, el.getAttribute("src"), el.getAttribute("aria-label")]);
    return /card|carte|karte|tarjeta|cvc|cvv|cryptogramme|stripe|adyen|braintree|hosted.?field|checkout\.com|checkoutshopper|mollie|payplug|stancer|payzen|lyra|worldline|ingenico|paybox|secure.?fields|spreedly|recurly|square|securepay/.test(hay);
  }
  const tag = el.tagName;
  if (!(tag === "INPUT" || tag === "TEXTAREA" || el.isContentEditable)) return false;
  const auto = (el.getAttribute("autocomplete") || "").toLowerCase();
  if (/\bcc-(number|csc|exp|exp-month|exp-year)\b/.test(auto)) return true;
  const labels = el.labels ? Array.from(el.labels).map((l) => l.textContent || "") : [];
  const data = Array.from(el.attributes || []).filter((a) => a.name.startsWith("data-")).map((a) => a.value);
  const hay = low([el.name, el.id, el.getAttribute("placeholder"), el.getAttribute("aria-label"), el.getAttribute("title"), ...labels, ...data]);
  return /\b(cvc|cvv|cvv2|cvc2|csc|cvn)\b|security.?code|code.?de.?s[ée]curit|cryptogramme|sicherheitscode|pr[üu]fnummer|c[oó]digo.?de.?seguridad|card.?num|cardnum|cc.?num|ccnum|kartennummer|num[ée]ro.?de.?(la.?)?carte|n[uú]mero.?de.?(la.?)?tarjeta|exp.?date|expir|date.?d.?exp|valid.?thru|ablaufdatum|caducidad|vencimiento/.test(hay);
})()
