#!/usr/bin/env python3
"""DevDock diagnostics plugin — sends notifications to Slack."""

import json
import sys
import urllib.request


class SlackNotifications:
    def __init__(self):
        self.webhook_url = ""

    def run(self):
        for line in sys.stdin:
            try:
                msg = json.loads(line.strip())
                method = msg.get("method", "")

                if method == "initialize":
                    self.webhook_url = msg.get("config", {}).get("webhook_url", "")
                    print(json.dumps({"status": "ok"}), flush=True)
                elif method == "emit":
                    self._handle_event(msg.get("event", {}))
                    print(json.dumps({"status": "ok"}), flush=True)
                elif method == "shutdown":
                    print(json.dumps({"status": "ok"}), flush=True)
                    break
            except Exception as e:
                print(json.dumps({"status": "error", "message": str(e)}), flush=True)

    def _handle_event(self, event):
        if not self.webhook_url:
            return
        event_type = event.get("type", "")
        if event_type == "Error":
            message = event.get("message", "Unknown error")
            self._send_slack(f":x: DevDock Error: {message}")
        elif event_type == "SessionEnd":
            success = event.get("success", False)
            duration = event.get("duration_secs", 0)
            emoji = ":white_check_mark:" if success else ":x:"
            self._send_slack(f"{emoji} DevDock session ended ({duration}s)")

    def _send_slack(self, text):
        payload = json.dumps({"text": text}).encode()
        req = urllib.request.Request(
            self.webhook_url,
            data=payload,
            headers={"Content-Type": "application/json"},
        )
        try:
            urllib.request.urlopen(req)
        except Exception as e:
            print(f"Slack send failed: {e}", file=sys.stderr)


if __name__ == "__main__":
    SlackNotifications().run()
