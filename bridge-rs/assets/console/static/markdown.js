// A reply as the app shows it (ChatText.swift, ConversationView.checkLinks): paragraphs, lists, headings, code,
// tables drawn as grids, links. Everything becomes Preact nodes, never HTML: what an agent writes cannot add markup
// or scripts to the page. Links open only for http(s), tel and mailto, and a label that looks like an address but
// names another site shows the real one.
import { h } from "./vendor/preact-htm.js";

const MEDIA_LINE = /^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$/gm;
const OPENABLE = new Set(["http:", "https:", "tel:", "mailto:"]);

/** The reply without the MEDIA lines and the tool-call markup a model sometimes writes as text. */
export function visible(text) {
  let result = (text || "").replace(MEDIA_LINE, "");
  if (result.includes("<")) {
    result = result.replace(/<tool_call>[\s\S]*?(?:<\/tool_call>|$)/g, "")
      .replace(/<\/?(?:arg_key|arg_value|tool_call|tool_response|function_calls?|invoke|parameter)\b[^>]*>/g, "");
  }
  return result.trim();
}

/** Files the reply points to with « MEDIA:<path> » lines. */
export function media(text) {
  return [...(text || "").matchAll(MEDIA_LINE)].map(m => m[1]);
}

function safeURL(raw) {
  try {
    const url = new URL(raw.startsWith("www.") ? `https://${raw}` : raw);
    return OPENABLE.has(url.protocol) ? url : null;
  } catch { return null; }
}

function shortLabel(url) {
  const host = url.hostname.replace(/^www\./, "");
  const last = decodeURIComponent(url.pathname.split("/").filter(Boolean).pop() || "").replace(/[-_]/g, " ");
  return last && last.length < 40 ? `${host} › ${last}` : host;
}

function link(label, raw, key) {
  const url = safeURL(raw);
  if (!url) return label;
  let shown = label;
  if (url.protocol.startsWith("http")) {
    const l = String(label).trim().toLowerCase();
    const looksLikeAddress = (!l.includes(" ") && /^[a-z0-9.-]+\.[a-z]{2,}(\/|$)/.test(l)) || l.startsWith("http") || l.startsWith("www.");
    const named = l.replace(/^https?:\/\//, "").replace(/^www\./, "").split("/")[0];
    const real = url.hostname.toLowerCase().replace(/^www\./, "");
    if (l === raw.toLowerCase()) shown = shortLabel(url);
    else if (looksLikeAddress && named !== real && !named.endsWith("." + real) && !real.endsWith("." + named)) shown = shortLabel(url);
  }
  return h("a", { key, href: url.href, target: "_blank", rel: "noopener noreferrer nofollow", title: url.href }, shown);
}

// Inline: `code`, **bold**, *italic* / _italic_, [label](url), bare https://… and www.…
const INLINE = /(`[^`\n]+`)|(\*\*[^*\n]+?\*\*)|(\*[^*\n]+?\*|\b_[^_\n]+?_\b)|(\[[^\]\n]+\]\([^)\s]+\))|((?:https?:\/\/|www\.)[^\s<>()]+[^\s<>().,;:!?'"»])/g;

export function inline(text, keyBase = "i") {
  const out = [];
  let last = 0, n = 0;
  for (const m of text.matchAll(INLINE)) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const key = `${keyBase}${n++}`;
    const [whole, code, bold, italic, md, bare] = m;
    if (code) out.push(h("code", { key }, code.slice(1, -1)));
    else if (bold) out.push(h("strong", { key }, inline(bold.slice(2, -2), key)));
    else if (italic) out.push(h("em", { key }, inline(italic.slice(1, -1), key)));
    else if (md) {
      const split = md.lastIndexOf("](");
      out.push(link(md.slice(1, split), md.slice(split + 2, -1), key));
    } else if (bare) out.push(link(bare, bare, key));
    else out.push(whole);
    last = m.index + whole.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

const cells = line => line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map(c => c.trim());
const isSeparator = line => line.includes("-") && /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/.test(line);

function table(lines, key) {
  const header = cells(lines[0]);
  const align = cells(lines[1]).map(c => c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : "left");
  const rows = lines.slice(2).map(cells);
  return h("div", { key, class: "table" }, h("table", null,
    h("thead", null, h("tr", null, header.map((c, i) => h("th", { key: i, class: `a-${align[i] || "left"}` }, inline(c, `${key}h${i}`))))),
    h("tbody", null, rows.map((r, j) => h("tr", { key: j }, header.map((_, i) => h("td", { key: i, class: `a-${align[i] || "left"}` }, inline(r[i] || "", `${key}r${j}c${i}`))))))));
}

/** Markdown blocks → nodes. */
export function render(text) {
  const lines = visible(text).split("\n");
  const out = [];
  let i = 0, k = 0;
  while (i < lines.length) {
    const line = lines[i];
    const key = `b${k++}`;
    if (/^\s*```/.test(line)) {
      const body = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) body.push(lines[i++]);
      i++;
      out.push(h("pre", { key }, h("code", null, body.join("\n"))));
    } else if (line.includes("|") && i + 1 < lines.length && isSeparator(lines[i + 1])) {
      const rows = [line, lines[i + 1]];
      i += 2;
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(lines[i++]);
      out.push(table(rows, key));
    } else if (/^\s*#{1,6}\s/.test(line)) {
      const level = Math.min(4, line.trim().match(/^#+/)[0].length + 1);
      out.push(h(`h${level}`, { key }, inline(line.trim().replace(/^#+\s*/, ""), key)));
      i++;
    } else if (/^\s*([-*•]|\d+[.)])\s+/.test(line)) {
      const ordered = /^\s*\d/.test(line);
      const items = [];
      while (i < lines.length && /^\s*([-*•]|\d+[.)])\s+/.test(lines[i])) {
        let item = lines[i++].replace(/^\s*([-*•]|\d+[.)])\s+/, "");
        while (i < lines.length && /^\s{2,}\S/.test(lines[i]) && !/^\s*([-*•]|\d+[.)])\s+/.test(lines[i])) item += " " + lines[i++].trim();
        items.push(item);
      }
      out.push(h(ordered ? "ol" : "ul", { key }, items.map((t, j) => h("li", { key: j }, inline(t, `${key}l${j}`)))));
    } else if (/^\s*>/.test(line)) {
      const quote = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) quote.push(lines[i++].replace(/^\s*>\s?/, ""));
      out.push(h("blockquote", { key }, inline(quote.join("\n"), key)));
    } else if (/^\s*(-{3,}|\*{3,})\s*$/.test(line)) {
      out.push(h("hr", { key }));
      i++;
    } else if (!line.trim()) {
      i++;
    } else {
      const para = [];
      while (i < lines.length && lines[i].trim() && !/^\s*(```|#{1,6}\s|[-*•]\s|\d+[.)]\s|>)/.test(lines[i])
             && !(lines[i].includes("|") && i + 1 < lines.length && isSeparator(lines[i + 1]))) para.push(lines[i++]);
      if (!para.length) para.push(lines[i++]);
      out.push(h("p", { key }, inline(para.join("\n"), key)));
    }
  }
  return out;
}
