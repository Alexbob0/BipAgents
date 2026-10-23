/* BipAgents page kit (pages.rs puts it inline in every page, before the agent's own scripts): charts drawn in SVG in
   the app's colours, a small diagram renderer, sortable tables, tabs, what-ifs, number formatting, and the app's
   colours for Chart.js when the page uses it. No network, no storage. window.Bip is the whole API. */
(function () {
  "use strict";
  var doc = document, root = doc.documentElement, NS = "http://www.w3.org/2000/svg";
  var TINT = "__BIP_TINT__";
  if (TINT && TINT.indexOf("__") !== 0 && !root.getAttribute("data-tint")) root.setAttribute("data-tint", TINT);
  var lang = function () { return root.lang || navigator.language || "en"; };
  var css = function (name) { return getComputedStyle(root).getPropertyValue(name).trim(); };
  var $ = function (target) { return typeof target === "string" ? doc.querySelector(target) : target; };
  var esc = function (text) {
    return String(text).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; });
  };
  var COLORS = ["var(--c1)", "var(--c2)", "var(--c3)", "var(--c4)", "var(--c5)", "var(--c6)"];
  var color = function (spec, i) { return (spec.colors && spec.colors[i]) || COLORS[i % COLORS.length]; };

  function fmt(n, digits, unit) {
    if (typeof n !== "number" || !isFinite(n)) return n == null ? "" : String(n);
    var d = digits == null ? (Math.round(n * 10) % 10 ? 1 : 0) : digits;
    var text = n.toLocaleString(lang(), { minimumFractionDigits: d, maximumFractionDigits: d });
    return unit ? text + (/^[%°]/.test(unit) ? "" : " ") + unit : text;
  }

  // A <svg> filling the target (a <div>, or the element itself when it is an <svg>).
  function canvas(target, height) {
    var host = $(target);
    if (!host) return null;
    var width = Math.max(240, Math.min(720, host.clientWidth || 320));
    var svg = host.tagName && host.tagName.toLowerCase() === "svg" ? host : null;
    if (!svg) {
      host.innerHTML = "";
      svg = doc.createElementNS(NS, "svg");
      host.appendChild(svg);
    }
    svg.setAttribute("viewBox", "0 0 " + width + " " + height);
    svg.setAttribute("class", ((svg.getAttribute("class") || "") + " chart").trim());
    svg.setAttribute("role", "img");
    svg.style.height = "auto";
    return { svg: svg, w: width, h: height, host: host };
  }
  function draw(svg, markup) { svg.insertAdjacentHTML("beforeend", markup); }
  function nice(max) {
    if (max <= 0) return 1;
    var step = Math.pow(10, Math.floor(Math.log10(max))), f = max / step;
    return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 2.5 ? 2.5 : f <= 5 ? 5 : 10) * step;
  }
  function legend(c, names, spec) {
    if (!names || names.length < 2) return;
    var div = doc.createElement("div");
    div.className = "legend";
    div.innerHTML = names.map(function (n, i) { return '<span><i style="background:' + color(spec, i) + '"></i>' + esc(n) + "</span>"; }).join("");
    c.host.tagName.toLowerCase() === "svg" ? c.host.after(div) : c.host.appendChild(div);
  }
  function grid(c, top, bottom, left, max, unit) {
    var out = "";
    // round steps (0, 10, 20, 30 rather than 0, 16,7, 33,3)
    var step = nice(max / 4), count = Math.round(max / step);
    for (var k = 0; k <= count; k++) {
      var y = bottom - (bottom - top) * k / count, v = step * k;
      out += '<line class="bip-grid" x1="' + left + '" x2="' + c.w + '" y1="' + y + '" y2="' + y + '"/>' +
        '<text class="bip-axis" x="' + (left - 6) + '" y="' + (y + 3.5) + '" text-anchor="end">' + esc(fmt(v, v % 1 ? 1 : 0)) + "</text>";
    }
    return out;
  }

  // Bars: {labels, values | series:[{name, values}], unit, goal, highlight: index, height}
  function bars(target, spec) {
    var c = canvas(target, spec.height || 190);
    if (!c) return;
    var series = spec.series || [{ name: spec.name || "", values: spec.values || [] }];
    var labels = spec.labels || [], n = Math.max(labels.length, series[0].values.length);
    var all = [].concat.apply([], series.map(function (s) { return s.values; })).concat(spec.goal || 0);
    var highest = Math.max.apply(null, all.map(Number).filter(isFinite)) || 1, unitStep = nice(highest / 4);
    var max = Math.ceil(highest * 1.08 / unitStep) * unitStep;
    var left = 34, top = 16, bottom = c.h - 22, band = (c.w - left) / n, gap = band * 0.28;
    var width = (band - gap) / series.length, out = grid(c, top, bottom, left, max);
    series.forEach(function (s, j) {
      s.values.forEach(function (v, i) {
        var h = Math.max(0, (bottom - top) * v / max), x = left + i * band + gap / 2 + j * width;
        var fill = series.length === 1 && spec.highlight != null && spec.highlight !== i ? "var(--tint)" : color(spec, j);
        out += '<rect x="' + x + '" y="' + (bottom - h) + '" width="' + Math.max(2, width - 2) + '" height="' + h +
          '" rx="' + Math.min(6, width / 3) + '" style="fill:' + fill + '"><title>' + esc((labels[i] || "") + " " + fmt(v, null, spec.unit)) + "</title></rect>";
        if (series.length === 1 && n <= 12)
          out += '<text class="bip-value" x="' + (x + width / 2 - 1) + '" y="' + (bottom - h - 4) + '" text-anchor="middle">' + esc(spec.format ? spec.format(v) : fmt(v)) + "</text>";
      });
    });
    labels.forEach(function (l, i) {
      out += '<text class="bip-axis" x="' + (left + i * band + band / 2) + '" y="' + (c.h - 6) + '" text-anchor="middle">' + esc(l) + "</text>";
    });
    if (spec.goal != null) {
      var gy = bottom - (bottom - top) * spec.goal / max;
      out += '<line x1="' + left + '" x2="' + c.w + '" y1="' + gy + '" y2="' + gy + '" style="stroke:var(--good);stroke-width:1.5;stroke-dasharray:5 4"/>';
    }
    draw(c.svg, out);
    legend(c, series.map(function (s) { return s.name; }), spec);
  }

  // Lines: {labels, values | series:[{name, values}], unit, goal, area: true, height}
  function line(target, spec) {
    var c = canvas(target, spec.height || 190);
    if (!c) return;
    var series = spec.series || [{ name: spec.name || "", values: spec.values || [] }];
    var labels = spec.labels || [], n = Math.max(labels.length, series[0].values.length);
    var all = [].concat.apply([], series.map(function (s) { return s.values; })).map(Number).filter(isFinite);
    if (spec.goal != null) all.push(spec.goal);
    var lo = spec.min != null ? spec.min : Math.min.apply(null, all), hi = Math.max.apply(null, all);
    if (spec.min == null) { var pad = (hi - lo) * 0.15 || 1; lo = lo - pad < 0 && lo >= 0 ? 0 : lo - pad; hi += pad; }
    var left = 34, top = 14, bottom = c.h - 22, step = (c.w - left - 12) / Math.max(1, n - 1);
    var x = function (i) { return left + 6 + i * step; }, y = function (v) { return bottom - (bottom - top) * (v - lo) / (hi - lo || 1); };
    var out = "";
    for (var k = 0; k <= 3; k++) {
      var v = lo + (hi - lo) * k / 3, gy = y(v);
      out += '<line class="bip-grid" x1="' + left + '" x2="' + c.w + '" y1="' + gy + '" y2="' + gy + '"/>' +
        '<text class="bip-axis" x="' + (left - 6) + '" y="' + (gy + 3.5) + '" text-anchor="end">' + esc(spec.format ? spec.format(v) : fmt(v, Math.abs(hi - lo) < 10 ? 1 : 0)) + "</text>";
    }
    if (spec.goal != null) out += '<line x1="' + left + '" x2="' + c.w + '" y1="' + y(spec.goal) + '" y2="' + y(spec.goal) + '" style="stroke:var(--good);stroke-width:1.5;stroke-dasharray:5 4"/>';
    series.forEach(function (s, j) {
      var pts = s.values.map(function (v, i) { return v == null ? null : [x(i), y(v)]; }).filter(Boolean);
      if (!pts.length) return;
      var path = pts.map(function (p, i) { return (i ? "L" : "M") + p[0].toFixed(1) + " " + p[1].toFixed(1); }).join("");
      if (spec.area !== false && series.length === 1)
        out += '<path d="' + path + "L" + pts[pts.length - 1][0] + " " + bottom + "L" + pts[0][0] + " " + bottom + 'Z" style="fill:' + color(spec, j) + ';opacity:.14"/>';
      out += '<path d="' + path + '" style="fill:none;stroke:' + color(spec, j) + ';stroke-width:2.5;stroke-linejoin:round;stroke-linecap:round"/>';
      pts.forEach(function (p, i) {
        var last = i === pts.length - 1;
        out += '<circle cx="' + p[0] + '" cy="' + p[1] + '" r="' + (last ? 4.5 : 3) + '" style="fill:' + (last ? color(spec, j) : "var(--card)") + ";stroke:" + color(spec, j) + ';stroke-width:2"/>';
      });
      if (series.length === 1) {
        var lp = pts[pts.length - 1];
        out += '<text class="bip-value" x="' + Math.min(lp[0], c.w - 4) + '" y="' + (lp[1] - 9) + '" text-anchor="end">' + esc(spec.format ? spec.format(s.values[s.values.length - 1]) : fmt(s.values[s.values.length - 1], null, spec.unit)) + "</text>";
      }
    });
    var every = Math.ceil(n / 7);
    labels.forEach(function (l, i) {
      if (i % every === 0 || i === n - 1) out += '<text class="bip-axis" x="' + x(i) + '" y="' + (c.h - 6) + '" text-anchor="middle">' + esc(l) + "</text>";
    });
    draw(c.svg, out);
    legend(c, series.map(function (s) { return s.name; }), spec);
  }

  // Donut: {labels, values, unit, center: "1 748 €", height}
  function donut(target, spec) {
    var c = canvas(target, spec.height || 170);
    if (!c) return;
    var values = spec.values || [], total = values.reduce(function (a, b) { return a + b; }, 0) || 1;
    var r = c.h / 2 - 8, cx = r + 8, cy = c.h / 2, angle = -Math.PI / 2, out = "";
    values.forEach(function (v, i) {
      var a2 = angle + 2 * Math.PI * v / total, large = a2 - angle > Math.PI ? 1 : 0;
      var p = function (a, rr) { return (cx + rr * Math.cos(a)).toFixed(2) + " " + (cy + rr * Math.sin(a)).toFixed(2); };
      var ri = r * 0.62;
      out += '<path d="M' + p(angle, r) + "A" + r + " " + r + " 0 " + large + " 1 " + p(a2 - 0.0001, r) + "L" + p(a2 - 0.0001, ri) +
        "A" + ri + " " + ri + " 0 " + large + " 0 " + p(angle, ri) + 'Z" style="fill:' + color(spec, i) + ';stroke:var(--card);stroke-width:2"/>';
      angle = a2;
    });
    if (spec.center) out += '<text x="' + cx + '" y="' + (cy + 6) + '" text-anchor="middle" style="fill:var(--ink);font:900 17px var(--font)">' + esc(spec.center) + "</text>";
    var lx = cx + r + 22, rows = Math.min(values.length, Math.floor((c.h - 10) / 22));
    for (var i = 0; i < rows; i++) {
      var ly = cy - rows * 11 + i * 22 + 14;
      out += '<rect x="' + lx + '" y="' + (ly - 9) + '" width="10" height="10" rx="3" style="fill:' + color(spec, i) + '"/>' +
        '<text x="' + (lx + 16) + '" y="' + ly + '" style="fill:var(--ink2);font:700 12.5px var(--font)">' + esc(spec.labels[i] || "") +
        ' <tspan style="fill:var(--ink);font-weight:900">' + esc(spec.format ? spec.format(values[i]) : fmt(values[i], null, spec.unit)) + "</tspan></text>";
    }
    draw(c.svg, out);
  }

  // A sparkline in a small element (a KPI tile): Bip.spark("#s", [3, 5, 4, 7])
  function spark(target, values, spec) {
    spec = spec || {};
    var c = canvas(target, spec.height || 36);
    if (!c) return;
    var lo = Math.min.apply(null, values), hi = Math.max.apply(null, values), step = c.w / Math.max(1, values.length - 1);
    var path = values.map(function (v, i) { return (i ? "L" : "M") + (i * step).toFixed(1) + " " + (c.h - 3 - (c.h - 6) * (v - lo) / (hi - lo || 1)).toFixed(1); }).join("");
    draw(c.svg, '<path d="' + path + '" style="fill:none;stroke:' + (spec.color || "var(--accent)") + ';stroke-width:2.5;stroke-linecap:round;stroke-linejoin:round"/>');
  }

  // Diagram: lines « A -> B » (or « A -> B: label »), drawn top-down in layers, boxes and arrows.
  function diagram(target, text) {
    var host = $(target);
    if (!host) return;
    var source = text != null ? text : host.textContent, nodes = [], edges = [], index = {};
    var node = function (name) {
      name = name.trim();
      if (!(name in index)) { index[name] = nodes.length; nodes.push({ name: name, layer: 0 }); }
      return index[name];
    };
    source.split(/\n|;/).forEach(function (raw) {
      var line = raw.trim();
      if (!line || line.indexOf("//") === 0) return;
      var parts = line.split(/\s*(?:-->|->|→)\s*/);
      if (parts.length < 2) { node(line); return; }
      for (var i = 0; i + 1 < parts.length; i++) {
        var to = parts[i + 1], label = "";
        var colon = to.indexOf(":");
        if (colon > 0 && i + 2 === parts.length) { label = to.slice(colon + 1).trim(); to = to.slice(0, colon); }
        edges.push({ from: node(parts[i]), to: node(to), label: label });
      }
    });
    for (var pass = 0; pass < nodes.length; pass++)
      edges.forEach(function (e) { if (e.from !== e.to && nodes[e.to].layer <= nodes[e.from].layer && nodes[e.from].layer < nodes.length) nodes[e.to].layer = nodes[e.from].layer + 1; });
    var layers = [];
    nodes.forEach(function (n, i) { (layers[n.layer] = layers[n.layer] || []).push(i); });
    layers = layers.filter(Boolean);
    var width = Math.max(260, Math.min(720, host.clientWidth || 320)), boxH = 38, gapY = 34;
    var height = layers.length * boxH + (layers.length - 1) * gapY + 8, out = "", pos = {};
    var measure = function (s) { return Math.min(width - 8, Math.max(64, s.length * 7.4 + 24)); };
    layers.forEach(function (layer, li) {
      var slot = width / layer.length;
      layer.forEach(function (i, k) {
        var w = Math.min(measure(nodes[i].name), slot - 8);
        pos[i] = { x: slot * k + slot / 2, y: 4 + li * (boxH + gapY), w: w };
      });
    });
    edges.forEach(function (e) {
      var a = pos[e.from], b = pos[e.to];
      if (!a || !b || e.from === e.to) return;
      var y1 = a.y + boxH, y2 = b.y - 3;
      if (b.y <= a.y) { y1 = a.y + boxH / 2; y2 = b.y + boxH / 2; }
      var mid = (y1 + y2) / 2;
      out += '<path class="bip-edge" marker-end="url(#bip-arrow)" d="M' + a.x + " " + y1 + "C" + a.x + " " + mid + " " + b.x + " " + mid + " " + b.x + " " + y2 + '"/>';
      if (e.label) out += '<text class="bip-axis" x="' + ((a.x + b.x) / 2 + 6) + '" y="' + (mid + 3) + '">' + esc(e.label) + "</text>";
    });
    nodes.forEach(function (n, i) {
      var p = pos[i];
      var label = n.name.length * 7.4 + 24 > p.w ? n.name.slice(0, Math.max(3, Math.floor((p.w - 24) / 7.4) - 1)) + "…" : n.name;
      out += '<g class="bip-node"><rect x="' + (p.x - p.w / 2) + '" y="' + p.y + '" width="' + p.w + '" height="' + boxH + '" rx="12"/>' +
        '<text x="' + p.x + '" y="' + (p.y + boxH / 2 + 4.5) + '" text-anchor="middle"><title>' + esc(n.name) + "</title>" + esc(label) + "</text></g>";
    });
    var svg = '<svg class="diagram" viewBox="0 0 ' + width + " " + height + '" role="img"><defs><marker id="bip-arrow" viewBox="0 0 10 10" refX="8" refY="5" ' +
      'markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" style="fill:var(--muted)"/></marker></defs>' + out + "</svg>";
    var wrap = doc.createElement("div");
    wrap.innerHTML = svg;
    host.replaceWith(wrap.firstChild);
  }

  // Tables with class « sort »: a tap on a header sorts by that column (numbers as numbers).
  function sortable(table) {
    var heads = table.querySelectorAll("th");
    var number = function (text) {
      var t = text.replace(/[\s  ]/g, "").replace(/[^\d,.\-−]/g, "").replace("−", "-");
      if (/,\d{1,2}$/.test(t)) t = t.replace(/\./g, "").replace(",", ".");
      else t = t.replace(/,/g, "");
      return t === "" || t === "-" ? NaN : parseFloat(t);
    };
    heads.forEach(function (th, col) {
      th.addEventListener("click", function () {
        var body = table.tBodies[0];
        if (!body) return;
        var rows = Array.prototype.slice.call(body.rows), up = th.getAttribute("aria-sort") !== "ascending";
        heads.forEach(function (h) { h.removeAttribute("aria-sort"); });
        th.setAttribute("aria-sort", up ? "ascending" : "descending");
        rows.sort(function (a, b) {
          var x = (a.cells[col] || {}).textContent || "", y = (b.cells[col] || {}).textContent || "";
          var nx = number(x), ny = number(y);
          var r = isNaN(nx) || isNaN(ny) ? x.localeCompare(y, lang()) : nx - ny;
          return up ? r : -r;
        });
        rows.forEach(function (r) { body.appendChild(r); });
      });
    });
  }

  // <div class="tabs"><button>…</button>…</div> followed by as many <section class="tab">.
  function tabs(bar) {
    var buttons = Array.prototype.slice.call(bar.querySelectorAll("button")), sections = [], el = bar.nextElementSibling;
    while (el && sections.length < buttons.length) { if (el.matches("section.tab")) sections.push(el); el = el.nextElementSibling; }
    var show = function (n) {
      buttons.forEach(function (b, i) { b.setAttribute("aria-selected", i === n ? "true" : "false"); });
      sections.forEach(function (s, i) { s.hidden = i !== n; });
    };
    buttons.forEach(function (b, i) { b.type = "button"; b.addEventListener("click", function () { show(i); }); });
    show(0);
  }

  // What-ifs: render(values) runs now and after every change of an input with an id (numbers as numbers,
  // switches as true/false); <output for="id"> elements show their input's value.
  function whatIf(render) {
    var run = function () {
      var values = {};
      doc.querySelectorAll("input[id],select[id]").forEach(function (input) {
        values[input.id] = input.type === "checkbox" ? input.checked : input.type === "range" || input.type === "number" ? parseFloat(input.value) : input.value;
      });
      doc.querySelectorAll("output[for]").forEach(function (o) {
        var v = values[o.htmlFor.value || o.getAttribute("for")];
        if (typeof v === "number") o.textContent = fmt(v, null, o.dataset.unit);
      });
      if (render) render(values);
    };
    doc.addEventListener("input", run);
    doc.readyState === "loading" ? doc.addEventListener("DOMContentLoaded", run) : run();
  }

  function set(target, text) { var el = $(target); if (el) el.textContent = text; }

  // The app's colours and font for Chart.js (called right after it, when the page uses it).
  function chartDefaults() {
    var C = window.Chart;
    if (!C || !C.defaults) return;
    C.defaults.color = css("--muted");
    C.defaults.borderColor = css("--line");
    C.defaults.font.family = css("--font");
    C.defaults.font.weight = "600";
    C.defaults.plugins.legend.labels.boxWidth = 10;
    C.defaults.plugins.legend.labels.boxHeight = 10;
    C.defaults.elements.bar.borderRadius = 6;
    C.defaults.elements.line.borderWidth = 2.5;
    C.defaults.elements.line.tension = 0.3;
    C.defaults.elements.point.radius = 3;
    C.defaults.maintainAspectRatio = false;
    var palette = ["--c1", "--c2", "--c3", "--c4", "--c5", "--c6"].map(css);
    if (C.defaults.plugins.colors) C.defaults.plugins.colors.enabled = false;
    C.register && C.register({
      id: "bipColors",
      beforeUpdate: function (chart) {
        (chart.config.data.datasets || []).forEach(function (d, i) {
          var many = chart.config.type === "pie" || chart.config.type === "doughnut" || chart.config.type === "polarArea";
          if (!d.backgroundColor) d.backgroundColor = many ? palette : palette[i % palette.length];
          if (!d.borderColor) d.borderColor = many ? css("--card") : palette[i % palette.length];
        });
      },
    });
  }

  // A map card (the bridge's map_image): its picture, and a list where a tap on a line rings its marker. Inside the
  // app the picture becomes a live map (MapLibre, which the app ships; the map's files come through the bridge, never
  // from here): pan, pinch, double tap, a tap on a line flies to its marker, and « Open in Maps » links the app
  // answers itself (bipmaps:, only numbers and a short name). Elsewhere (a browser, an export) the picture stays.
  var ANDROID = /Android/i.test(navigator.userAgent || "");
  var MAP_BASE = ANDROID ? "https://bipmap.invalid/" : "bipmap://map/";
  var MAP_WORDS = {
    open: ANDROID ? { fr: "Ouvrir dans Google Maps", en: "Open in Google Maps", de: "In Google Maps öffnen", es: "Abrir en Google Maps" }
      : { fr: "Ouvrir dans Plans", en: "Open in Maps", de: "In Karten öffnen", es: "Abrir en Mapas" },
    route: { fr: "Itinéraire", en: "Directions", de: "Route", es: "Cómo llegar" },
    fingers: { fr: "Deux doigts pour déplacer la carte", en: "Use two fingers to move the map",
      de: "Mit zwei Fingern die Karte bewegen", es: "Usa dos dedos para mover el mapa" },
  };
  var mapWord = function (key) { var w = MAP_WORDS[key]; return w[lang().slice(0, 2).toLowerCase()] || w.en; };
  var library = null;

  function maplibre() {
    if (library) return library;
    library = new Promise(function (resolve, reject) {
      if (window.maplibregl) { resolve(window.maplibregl); return; }
      var script = doc.createElement("script");
      var timer = setTimeout(function () { reject(new Error("no map library")); }, 8000);
      script.onload = function () {
        clearTimeout(timer);
        window.maplibregl ? resolve(window.maplibregl) : reject(new Error("no map library"));
      };
      script.onerror = function () { clearTimeout(timer); reject(new Error("no map library")); };
      script.src = MAP_BASE + "lib.js";
      (doc.head || root).appendChild(script);
    }).then(function (ml) {
      // every map file asked from the page's own thread, through the app's door
      ml.addProtocol("bipmap", function (params, abort) {
        var url = MAP_BASE + String(params.url).replace(/^bipmap:\/\/map\//, "");
        return fetch(url, { signal: abort && abort.signal }).then(function (r) {
          if (r.status === 204) return { data: params.type === "json" ? {} : new ArrayBuffer(0) };
          if (!r.ok) throw new Error("map file " + r.status);
          var body = params.type === "json" ? r.json() : params.type === "string" ? r.text() : r.arrayBuffer();
          return body.then(function (data) { return { data: data }; });
        });
      });
      return ml;
    });
    return library;
  }

  var coords = function (lat, lon) { return (+lat).toFixed(6) + "," + (+lon).toFixed(6); };
  var shortName = function (text) { return encodeURIComponent(String(text || "").replace(/\s+/g, " ").trim().slice(0, 80)); };

  function mapCard(card) {
    var img = card.querySelector("img");
    if (!img) return;
    var frame = doc.createElement("div"), ring = doc.createElement("div");
    frame.className = "map-frame";
    ring.className = "map-ring";
    ring.hidden = true;
    img.replaceWith(frame);
    frame.appendChild(img);
    frame.appendChild(ring);
    var items = Array.prototype.slice.call(card.querySelectorAll("li[data-x]"));
    var live = null;
    items.forEach(function (li) {
      li.addEventListener("click", function () {
        var again = li.getAttribute("aria-current") === "true";
        items.forEach(function (other) { other.removeAttribute("aria-current"); });
        if (live) { live.select(again ? -1 : points.map(function (p) { return p.li; }).indexOf(li)); if (!again) li.setAttribute("aria-current", "true"); return; }
        ring.hidden = again;
        if (again) return;
        li.setAttribute("aria-current", "true");
        ring.style.left = li.dataset.x + "%";
        ring.style.top = li.dataset.y + "%";
      });
    });
    var points = items.map(function (li) {
      return { lat: parseFloat(li.dataset.lat), lon: parseFloat(li.dataset.lon), color: li.dataset.color, li: li,
        label: (li.firstChild && li.firstChild.nodeType === 3 ? li.firstChild.nodeValue : li.textContent).trim() };
    }).filter(function (p) { return isFinite(p.lat) && isFinite(p.lon) && Math.abs(p.lat) <= 90 && Math.abs(p.lon) <= 180; });
    var center = (card.dataset.center || "").split(",").map(parseFloat);
    if (!points.length && !(center.length === 2 && isFinite(center[0]) && isFinite(center[1]))) return;
    maplibre().then(function (ml) {
      live = liveMap(ml, card, frame, img, ring, points, center);
    }, function () { /* outside the app: the picture stays */ });
  }

  function liveMap(ml, card, frame, img, ring, points, center) {
    var inline = root.getAttribute("data-bip-view") === "inline";
    var box = doc.createElement("div");
    box.className = "map-live";
    box.style.cssText = "position:absolute;inset:0";   // over the picture (MapLibre's own CSS says relative)
    frame.appendChild(box);
    var options = {
      container: box, style: "bipmap://map/style.json", attributionControl: { compact: true }, dragRotate: false,
      pitchWithRotate: false, touchPitch: false, maxZoom: 18, fadeDuration: 0, cooperativeGestures: inline,
      canvasContextAttributes: { preserveDrawingBuffer: true },
      locale: { "CooperativeGesturesHandler.MobileHelpText": mapWord("fingers"),
        "CooperativeGesturesHandler.WindowsHelpText": mapWord("fingers"), "CooperativeGesturesHandler.MacHelpText": mapWord("fingers") },
    };
    var spots = points.map(function (p) { return [p.lon, p.lat]; });
    if (spots.length > 1) {
      options.bounds = spots.reduce(function (b, s) { return b.extend(s); }, new ml.LngLatBounds(spots[0], spots[0]));
      options.fitBoundsOptions = { padding: { top: 40, right: 40, bottom: 56, left: 40 }, maxZoom: 16 };
    } else {
      options.center = spots.length ? spots[0] : [center[1], center[0]];
      options.zoom = parseFloat(card.dataset.zoom) || 15;
    }
    var map, styled = false, ready = false;
    try { map = new ml.Map(options); } catch (e) { box.remove(); return null; }
    var giveUp = function () {
      if (ready) return;
      try { map.remove(); } catch (e) { /* gone already */ }
      box.remove();
    };
    var timer = setTimeout(function () { if (!styled) giveUp(); }, 12000);
    map.on("styledata", function () { styled = true; });
    map.on("error", function () { if (!styled) { clearTimeout(timer); giveUp(); } });
    map.touchZoomRotate.disableRotation();
    var pins = points.map(function (p, i) {
      var holder = doc.createElement("div"), pin = doc.createElement("span");
      holder.className = "map-pin";
      pin.textContent = String(i + 1);
      if (p.color) pin.style.background = p.color;
      holder.appendChild(pin);
      holder.addEventListener("click", function (event) { event.stopPropagation(); p.li.click(); });
      new ml.Marker({ element: holder }).setLngLat([p.lon, p.lat]).addTo(map);
      return holder;
    });
    map.on("load", function () {
      ready = true;
      clearTimeout(timer);
      if (card.dataset.line && spots.length > 1) {
        map.addSource("bip-line", { type: "geojson", data: { type: "Feature", properties: {},
          geometry: { type: "LineString", coordinates: spots } } });
        map.addLayer({ id: "bip-line", type: "line", source: "bip-line", layout: { "line-cap": "round", "line-join": "round" },
          paint: { "line-color": css("--accent") || "#7C6CF2", "line-width": 4, "line-opacity": 0.85 } });
      }
      card.classList.add("live");
      // the credits folded into their ⓘ once the map has drawn (MapLibre shows them open at first)
      map.once("idle", function () {
        var credits = box.querySelector(".maplibregl-ctrl-attrib");
        if (credits) { credits.removeAttribute("open"); credits.classList.remove("maplibregl-compact-show"); }
      });
      ring.hidden = true;
      img.setAttribute("aria-hidden", "true");
      points.forEach(function (p) {
        var open = doc.createElement("a");
        open.className = "map-open";
        open.href = "bipmaps:place?lat=" + (+p.lat).toFixed(6) + "&lon=" + (+p.lon).toFixed(6) + "&label=" + shortName(p.label);
        open.textContent = mapWord("open");
        open.addEventListener("click", function (event) { event.stopPropagation(); });
        p.li.appendChild(open);
      });
      var mode = card.dataset.directions;
      if (mode && points.length > 1) {
        var first = points[0], last = points[points.length - 1], route = doc.createElement("a");
        route.className = "map-route";
        route.href = "bipmaps:route?from=" + coords(first.lat, first.lon) + "&to=" + coords(last.lat, last.lon) +
          "&mode=" + encodeURIComponent(mode) + "&from_label=" + shortName(first.label) + "&to_label=" + shortName(last.label);
        route.textContent = mapWord("route") + " · " + mapWord("open");
        card.appendChild(route);
      }
    });
    return {
      select: function (i) {
        pins.forEach(function (pin, j) { pin.toggleAttribute("data-current", j === i); });
        if (i < 0 || !points[i]) return;
        map.easeTo({ center: [points[i].lon, points[i].lat], zoom: Math.max(map.getZoom(), 15), duration: 450 });
      },
    };
  }

  function enhance() {
    doc.querySelectorAll(".map-card").forEach(mapCard);
    doc.querySelectorAll("table.sort").forEach(sortable);
    doc.querySelectorAll(".tabs").forEach(tabs);
    doc.querySelectorAll("pre.diagram,div.diagram[data-src]").forEach(function (el) { diagram(el, el.dataset.src || null); });
  }
  doc.readyState === "loading" ? doc.addEventListener("DOMContentLoaded", enhance) : enhance();

  window.Bip = { fmt: fmt, bars: bars, line: line, donut: donut, spark: spark, diagram: diagram, whatIf: whatIf, set: set,
    css: css, _chart: chartDefaults };
})();
