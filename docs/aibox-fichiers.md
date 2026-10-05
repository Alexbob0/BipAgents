🇬🇧 English · [🇫🇷 Français](aibox-fichiers.fr.md)

# SPEC addendum — sending files (PDF, Excel, text…)

Decision of 2026-10-04: the app must send **all file types**, as Telegram does today.

## Findings
- The Hermes v0.21.5 `api_server` rejects non-image files in `content` (`400 unsupported_content_type`, see `api-server.md` § Limitations).
- The messaging adapters (Telegram, SimpleX) download the document into a local cache in the container and pass **its path** to the agent, which reads it with its tools (`read_file`, terminal, PDF extraction…).
- `POST /v1/artifacts/upload` (checked on aibox on 2026-10-04): reserved for the browser extension broker (404 without `browser.extension_control.enabled`), ephemeral 5-min storage, single read, no disk path, no consumer on the agent side. **Unusable for handing a file to the agent.**

## Chosen mechanism (same principle as Telegram)
1. The app sends the file to the **bridge**: `POST https://aibox.example.ts.net:8643/v1/files` (multipart: `agent`, `file`; Bearer bridge key).
2. The bridge writes it into a folder **already visible to the agent**: a volume mounted in the container (like `/workspace` today) or the mounted Hermes home (`~/hermes-agent/hermes-home` → `/home/hermes/.hermes`). Final choice to be made on the aibox side.
3. Response: `{"path": "<path as seen by the container>", "filename": "...", "size": n}`.
4. The app adds a reference line to the message, then sends it normally through `chat/stream`:
   `[Pièce jointe : releve.pdf (application/pdf, 220 Ko) → /home/hermes/.hermes/profiles/vie/uploads/2026-10/…-releve.pdf]`
   Implemented in `HermesKit` (`MessageInput.prepared(uploader:)`, `BridgeDocumentUploader`).

aibox's Claude will add `POST /v1/files` to contract C2 of the SPEC.

## To do on the aibox side (bridge, §B3)
- Route `POST /v1/files`: size limit (e.g. 50 MB), sanitized file names, 640 permissions, purge after 30 days.
- Images are still sent inline (`image_url` data URI ≤ 1600 px), without going through the bridge.
