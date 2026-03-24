#!/usr/bin/env python3
"""
session-logger — DevDock lifecycle plugin

Logs session telemetry to stdout. Wire into devdock.toml:

    [plugins.hooks]
    after_tilt  = ["./plugins/session-logger/main.py"]
    before_stop = ["./plugins/session-logger/main.py"]
    on_error    = ["./plugins/session-logger/main.py"]
"""

import json
import sys
import datetime


def _now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def respond(result: dict) -> None:
    print(json.dumps(result), flush=True)


def handle(method: str, params: dict) -> dict:
    if method == "initialize":
        return {"status": "ok", "name": "session-logger", "version": "0.1.0"}

    if method == "after_tilt":
        session_id = params.get("session_id", "unknown")
        profile = params.get("profile", "unknown")
        ts = _now()
        print(
            f"[session-logger] Session started | id={session_id} profile={profile} at={ts}",
            file=sys.stderr,
        )
        return {"status": "ok"}

    if method == "before_stop":
        session_id = params.get("session_id", "unknown")
        ts = _now()
        print(
            f"[session-logger] Session ending  | id={session_id} at={ts}",
            file=sys.stderr,
        )
        return {"status": "ok"}

    if method == "on_error":
        error = params.get("error", "unknown error")
        session_id = params.get("session_id", "unknown")
        ts = _now()
        print(
            f"[session-logger] Session error   | id={session_id} error={error!r} at={ts}",
            file=sys.stderr,
        )
        return {"status": "ok"}

    if method == "shutdown":
        return {"status": "ok"}

    # Unknown method — return ok to avoid blocking the lifecycle
    return {"status": "ok"}


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError as e:
            respond({"status": "error", "error": f"invalid JSON: {e}"})
            continue

        method = msg.get("method", "")
        params = msg.get("params", {})

        result = handle(method, params)
        respond(result)


if __name__ == "__main__":
    main()
