"""The fake shop of the sign-up tests and labs (signup.html, modelled on a real French pharmacy's « Se connecter /
S'inscrire » page: a login form and a sign-up form on one page, honeypot fields, a native-looking select replaced by a
custom « Civilité » dropdown, the sign-up form in a panel that unfolds, its password fields rendered a moment later).

    python3 site.py <port> <submissions.jsonl> [--host 127.0.0.1]

GET / and /connexion: the page. POST /inscription: the form as received kept as one JSON line (the password's length
and whether the confirmation matched, never the password itself), then « Votre compte a été créé ». Standard library
only (python 3.9+)."""
from __future__ import annotations

import http.server
import json
import os
import sys
import time
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))


class Shop(http.server.BaseHTTPRequestHandler):
    submissions = ""

    def log_message(self, *args):
        pass

    def _send(self, status: int, body: str, kind: str = "text/html; charset=utf-8") -> None:
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = urllib.parse.urlsplit(self.path).path
        if path in ("/", "/connexion", "/signup.html"):
            with open(os.path.join(HERE, "signup.html"), encoding="utf-8") as page:
                return self._send(200, page.read())
        if path == "/health":
            return self._send(200, "ok", "text/plain")
        return self._send(404, "<h1>Page introuvable</h1>")

    def do_POST(self):
        path = urllib.parse.urlsplit(self.path).path
        length = int(self.headers.get("Content-Length") or 0)
        form = urllib.parse.parse_qs(self.rfile.read(length).decode(), keep_blank_values=True)
        one = {k: v[0] for k, v in form.items()}
        if path != "/inscription":
            return self._send(200, "<h1>Identifiants incorrects</h1>")
        password = one.pop("password", "")
        confirmation = one.pop("password_confirmation", "")
        kept = {"at": time.time(), "fields": one, "password_length": len(password),
                "confirmation_matches": bool(password) and password == confirmation,
                "honeypot_filled": bool(one.get("my_name_reg"))}
        with open(Shop.submissions, "a", encoding="utf-8") as out:
            out.write(json.dumps(kept, ensure_ascii=False) + "\n")
        return self._send(200, "<title>Compte créé</title><h1>Votre compte a été créé</h1>"
                               "<p>Un e-mail de confirmation vous a été envoyé.</p>")


def main() -> None:
    port, Shop.submissions = int(sys.argv[1]), sys.argv[2]
    host = sys.argv[sys.argv.index("--host") + 1] if "--host" in sys.argv else "127.0.0.1"
    http.server.ThreadingHTTPServer((host, port), Shop).serve_forever()


if __name__ == "__main__":
    main()
