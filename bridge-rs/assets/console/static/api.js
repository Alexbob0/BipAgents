// Calls to this bridge. The session cookie (HttpOnly: no script here can read it) plus the X-Bip-Console header stand
// for the bridge key; an agent's Hermes is reached through /hermes/<agent>/…, where the bridge adds the agent's key.

export class HttpError extends Error {
  constructor(status, detail) {
    super(detail || `HTTP ${status}`);
    this.status = status;
  }
}

async function failure(response) {
  let detail = "";
  try {
    const body = await response.json();
    detail = typeof body.detail === "string" ? body.detail : body.error?.message || body.message || "";
  } catch {}
  return new HttpError(response.status, detail);
}

export async function raw(path, { method = "GET", body, headers = {}, signal } = {}) {
  const init = { method, credentials: "same-origin", signal, headers: { "X-Bip-Console": "1", ...headers } };
  if (body !== undefined) {
    init.body = JSON.stringify(body);
    init.headers["Content-Type"] = "application/json";
  }
  const response = await fetch(path, init);
  if (!response.ok) throw await failure(response);
  return response;
}

export async function call(path, options) {
  const response = await raw(path, options);
  if (response.status === 204) return null;
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

const segment = s => encodeURIComponent(s);

/** An agent's Hermes, through the bridge. */
export const hermes = (agent, path, options) => call(`/hermes/${segment(agent)}/${path}`, options);

/** A file (audio, image…) as an object URL: `<audio src>` cannot send the console header. */
export async function blobURL(path, options) {
  for (let attempt = 0; attempt < 10; attempt++) {
    const response = await raw(path, options);
    if (response.status === 202) {   // still being made (an inbox message's audio)
      await new Promise(r => setTimeout(r, 3000));
      continue;
    }
    return URL.createObjectURL(await response.blob());
  }
  throw new HttpError(504, "not ready");
}

/** Reads a Server-Sent Events stream; `onEvent({type, data})` per event. Resolves when the stream ends. */
export async function events(path, onEvent, signal) {
  const response = await raw(path, { headers: { Accept: "text/event-stream" }, signal });
  const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
  let buffer = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += value.replace(/\r\n?/g, "\n");
    let end;
    while ((end = buffer.indexOf("\n\n")) >= 0) {
      const frame = buffer.slice(0, end);
      buffer = buffer.slice(end + 2);
      let name = null;
      const data = [];
      for (const line of frame.split("\n")) {
        if (line.startsWith(":")) continue;
        const colon = line.indexOf(":");
        const field = colon < 0 ? line : line.slice(0, colon);
        const val = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
        if (field === "event") name = val;
        else if (field === "data") data.push(val);
      }
      if (!data.length && !name) continue;
      const text = data.join("\n");
      if (text.trim() === "[DONE]") continue;
      let json = {};
      try { json = text ? JSON.parse(text) : {}; } catch { json = { text }; }
      const candidates = [json.type, json.event, json.event_type, name].filter(t => typeof t === "string");
      const type = candidates.find(t => t.includes(".")) || candidates[0] || "message";
      onEvent({ type, data: json });
    }
  }
}
