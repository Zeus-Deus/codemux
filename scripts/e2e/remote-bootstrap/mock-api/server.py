#!/usr/bin/env python3
"""Stand-in for the Codemux account API, for the remote-bootstrap E2E harness.

Dependency-free (Python 3 stdlib only) so the container is a plain
`python:3-slim` with no pip step.

It implements exactly the surface `codemux login` / `codemux connect` /
`web_remote::registration` touch, and nothing else:

  POST /api/auth/desktop/signin   -> {token, expiresAt, user{id,email,name,image}}
  GET  /api/auth/desktop/verify   -> {user{...}, session{expiresAt}}   (Bearer)
  POST /api/devices               -> {ok:true}                         (Bearer)

Everything it receives is appended to a JSONL request log so the harness can
assert *after the fact* that, say, a `POST /api/devices` really arrived with
the right bearer and a `nodeId`. Two introspection endpoints expose that log:

  GET  /_e2e/requests             -> the whole log as a JSON array
  GET  /_e2e/health               -> liveness probe for the harness

The server is deliberately permissive about the credential itself: the client
sends `password` = `derive_auth_secret(password, email)` (Argon2-stretched), and
reproducing that derivation here would mean vendoring the KDF. What it *does*
enforce is the shape — an unknown email, a missing `password`, or a bad bearer
is rejected — so the negative paths in the CLI still have something to fail
against.

Config comes from the environment:
  E2E_EMAIL      the one address that may sign in   (default test@example.com)
  E2E_TOKEN      the session bearer handed out      (default e2e-session-token)
  E2E_USER_ID    the account id reported back       (default usr_e2e_1)
  E2E_LOG        request log path        (default /var/log/mock-api/requests.jsonl)
  E2E_PORT       listen port                        (default 8787)
"""

import json
import os
import sys
import threading
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

EMAIL = os.environ.get("E2E_EMAIL", "test@example.com")
TOKEN = os.environ.get("E2E_TOKEN", "e2e-session-token")
USER_ID = os.environ.get("E2E_USER_ID", "usr_e2e_1")
USER_NAME = os.environ.get("E2E_USER_NAME", "E2E Tester")
LOG_PATH = os.environ.get("E2E_LOG", "/var/log/mock-api/requests.jsonl")
PORT = int(os.environ.get("E2E_PORT", "8787"))

# Far enough out that `is_token_expired` (chrono RFC3339 parse + compare) is
# never the reason a run fails, but still a real timestamp rather than a
# sentinel the client might special-case.
EXPIRES_AT = (datetime.now(timezone.utc) + timedelta(days=30)).strftime(
    "%Y-%m-%dT%H:%M:%SZ"
)

_log_lock = threading.Lock()


def record(entry):
    """Append one request to the JSONL log (and echo it to stdout for
    `docker compose logs`). Serialized because ThreadingHTTPServer handles
    each request on its own thread."""
    line = json.dumps(entry, sort_keys=True)
    with _log_lock:
        with open(LOG_PATH, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")
            fh.flush()
    print("[mock-api] " + line, flush=True)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    # ── plumbing ────────────────────────────────────────────────────

    def log_message(self, fmt, *args):  # quieter than the default
        pass

    def _bearer(self):
        raw = self.headers.get("Authorization", "")
        return raw[7:].strip() if raw.startswith("Bearer ") else None

    def _body(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0:
            return None, b""
        raw = self.rfile.read(length)
        try:
            return json.loads(raw.decode("utf-8")), raw
        except Exception:
            return None, raw

    def _send(self, status, payload):
        blob = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(blob)))
        self.end_headers()
        self.wfile.write(blob)

    def _note(self, body, extra=None):
        """Log the request. The bearer is recorded so the harness can prove the
        device registration carried the session token `login` persisted; the
        signin `password` is redacted to a length because it is a (derived, but
        still credential-shaped) secret and this log is dumped on failure."""
        safe = dict(body) if isinstance(body, dict) else body
        if isinstance(safe, dict) and "password" in safe:
            safe["password"] = "<redacted:%d chars>" % len(str(safe["password"]))
        entry = {
            "ts": datetime.now(timezone.utc).isoformat(),
            "method": self.command,
            "path": self.path,
            "bearer": self._bearer(),
            "body": safe,
            "remote": self.client_address[0],
        }
        if extra:
            entry.update(extra)
        record(entry)
        return entry

    # ── routes ──────────────────────────────────────────────────────

    def do_GET(self):
        if self.path.startswith("/_e2e/health"):
            self._send(200, {"ok": True, "email": EMAIL})
            return

        if self.path.startswith("/_e2e/requests"):
            entries = []
            try:
                with open(LOG_PATH, "r", encoding="utf-8") as fh:
                    for line in fh:
                        line = line.strip()
                        if line:
                            entries.append(json.loads(line))
            except FileNotFoundError:
                pass
            self._send(200, entries)
            return

        if self.path.startswith("/api/auth/desktop/verify"):
            self._note(None)
            if self._bearer() != TOKEN:
                self._send(401, {"error": "invalid token"})
                return
            self._send(
                200,
                {
                    "user": {
                        "id": USER_ID,
                        "email": EMAIL,
                        "name": USER_NAME,
                        "image": None,
                    },
                    "session": {"expiresAt": EXPIRES_AT},
                },
            )
            return

        self._note(None)
        self._send(404, {"error": "not found"})

    def do_POST(self):
        body, _raw = self._body()
        self._note(body)

        if self.path.startswith("/api/auth/desktop/signin"):
            if not isinstance(body, dict):
                self._send(400, {"error": "malformed body"})
                return
            # The wire `password` is the Argon2-derived AuthSecret, not the
            # user's password — only its presence is checkable here.
            if not body.get("password"):
                self._send(400, {"error": "missing password"})
                return
            if body.get("email") != EMAIL:
                # Same non-enumerating stance the real API takes; the CLI
                # collapses any 4xx into its generic credential error anyway.
                self._send(401, {"error": "Invalid email or password"})
                return
            self._send(
                200,
                {
                    "token": TOKEN,
                    "expiresAt": EXPIRES_AT,
                    "user": {
                        "id": USER_ID,
                        "email": EMAIL,
                        "name": USER_NAME,
                        "image": None,
                    },
                },
            )
            return

        if self.path.startswith("/api/devices"):
            if self._bearer() != TOKEN:
                self._send(401, {"error": "unauthorized"})
                return
            if not isinstance(body, dict) or not body.get("nodeId"):
                self._send(400, {"error": "missing nodeId"})
                return
            self._send(
                200,
                {
                    "ok": True,
                    "device": {
                        "id": "row_" + str(body.get("deviceId")),
                        "deviceId": body.get("deviceId"),
                        "nodeId": body.get("nodeId"),
                        "name": body.get("name"),
                        "platform": body.get("platform"),
                    },
                },
            )
            return

        self._send(404, {"error": "not found"})


def main():
    os.makedirs(os.path.dirname(LOG_PATH), exist_ok=True)
    # Truncate on boot so each `run.sh` invocation starts from an empty log and
    # "no POST /api/devices arrived" can never be a leftover from last run.
    open(LOG_PATH, "w", encoding="utf-8").close()
    print(
        "[mock-api] listening on 0.0.0.0:%d  email=%s  token=%s  log=%s"
        % (PORT, EMAIL, TOKEN, LOG_PATH),
        flush=True,
    )
    ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()


if __name__ == "__main__":
    sys.exit(main())
