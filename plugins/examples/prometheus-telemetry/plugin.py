#!/usr/bin/env python3
"""DevDock telemetry plugin — pushes metrics to Prometheus push gateway."""

import json
import sys

try:
    from prometheus_client import CollectorRegistry, Counter, Gauge, push_to_gateway
    HAS_PROMETHEUS = True
except ImportError:
    HAS_PROMETHEUS = False


class PrometheusTelemetry:
    def __init__(self):
        self.gateway = "localhost:9091"
        self.registry = CollectorRegistry() if HAS_PROMETHEUS else None
        if HAS_PROMETHEUS:
            self.sessions = Counter("devdock_sessions_total", "Total sessions", registry=self.registry)
            self.bootstrap_ms = Gauge("devdock_bootstrap_ms", "Bootstrap time", registry=self.registry)

    def run(self):
        for line in sys.stdin:
            try:
                msg = json.loads(line.strip())
                method = msg.get("method", "")

                if method == "initialize":
                    self.gateway = msg.get("config", {}).get("push_gateway", self.gateway)
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
        if not HAS_PROMETHEUS:
            return
        event_type = event.get("type", "")
        if event_type == "SessionStart":
            self.sessions.inc()
        elif event_type == "BootstrapComplete":
            self.bootstrap_ms.set(event.get("bootstrap_time_ms", 0))
        push_to_gateway(self.gateway, job="devdock", registry=self.registry)


if __name__ == "__main__":
    PrometheusTelemetry().run()
