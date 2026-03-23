#!/usr/bin/env python3
"""DevDock {{type}} plugin: {{name}}

Protocol: JSON-lines over stdin/stdout.
Receives: {"method": "initialize"|"emit"|"shutdown", ...}
Responds: {"status": "ok"} or {"status": "error", "message": "..."}
"""

import json
import sys


class DevDockPlugin:
    """Base class with protocol handling."""

    def run(self):
        for line in sys.stdin:
            try:
                msg = json.loads(line.strip())
                method = msg.get("method", "")

                if method == "initialize":
                    self.handle_initialize(msg.get("config", {}))
                    self._respond_ok()
                elif method == "emit":
                    self.handle_emit(msg.get("event", {}))
                    self._respond_ok()
                elif method == "shutdown":
                    self.handle_shutdown()
                    self._respond_ok()
                    break
                else:
                    self._respond_error(f"Unknown method: {method}")
            except Exception as e:
                self._respond_error(str(e))

    def _respond_ok(self):
        print(json.dumps({"status": "ok"}), flush=True)

    def _respond_error(self, message):
        print(json.dumps({"status": "error", "message": message}), flush=True)

    def handle_initialize(self, config):
        """Override: set up your plugin with the given config."""
        pass

    def handle_emit(self, event):
        """Override: handle a telemetry/diagnostic event."""
        pass

    def handle_shutdown(self):
        """Override: clean up resources."""
        pass


class MyPlugin(DevDockPlugin):
    def handle_initialize(self, config):
        # YOUR CODE HERE
        pass

    def handle_emit(self, event):
        # YOUR CODE HERE
        event_type = event.get("type", "unknown")
        print(f"[{event_type}] {event}", file=sys.stderr)

    def handle_shutdown(self):
        pass


if __name__ == "__main__":
    MyPlugin().run()
