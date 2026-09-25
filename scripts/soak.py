#!/usr/bin/env python3
"""Read-only live fleet observation. Never starts miners or changes host state."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import ssl
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument("--url", required=True)
parser.add_argument("--ca")
parser.add_argument("--hours", type=float, default=24)
parser.add_argument("--output", type=Path, default=Path("soak.jsonl"))
args = parser.parse_args()
if args.hours <= 0 or not args.url.startswith("https://"):
    parser.error("use HTTPS and a positive duration")
token = os.environ.get("RIGDECK_TOKEN")
if not token:
    parser.error("RIGDECK_TOKEN environment variable is required")
tls = ssl.create_default_context(cafile=args.ca)
deadline = time.monotonic() + args.hours * 3600
issues = 0
with args.output.open("x", encoding="utf-8") as output:
    while time.monotonic() < deadline:
        now = time.time()
        record = {"at": datetime.now(timezone.utc).isoformat(), "issues": []}
        try:
            def get(path):
                request = urllib.request.Request(args.url.rstrip("/") + "/api/v1/" + path, headers={"Authorization": "Bearer " + token})
                with urllib.request.urlopen(request, context=tls, timeout=20) as response:
                    return json.load(response)
            machines = get("machines")
            samples = get("telemetry")
            record["observations"] = samples
            if not machines:
                record["issues"].append("No machines configured")
            for machine in machines:
                readings = {o["kind"]: o for o in samples if o["machine_id"] == machine["id"]}
                for kind in ("system", "mining"):
                    observation = readings.get(kind)
                    fresh = observation and now - datetime.fromisoformat(observation["observed_at"].replace("Z", "+00:00")).timestamp() < 40
                    if not fresh or observation.get("error"):
                        record["issues"].append(f"{machine['name']}: {kind} unavailable/stale")
                for instance in (readings.get("mining", {}).get("data") or {}).get("instances", []):
                    stats = instance.get("stats") or {}
                    if instance.get("desired") == "running" and (not instance.get("process_alive") or now - instance.get("stats_observed_at", 0) > 40 or not stats.get("hashrate_hs", 0) or not stats.get("algorithm")):
                        record["issues"].append(f"{machine['name']}/{instance.get('instance')}: expected running without fresh mining evidence")
        except Exception as error:
            record["issues"].append(str(error))
        issues += len(record["issues"])
        output.write(json.dumps(record, ensure_ascii=False) + "\n"); output.flush()
        print(f"{record['at']} issues={len(record['issues'])}", flush=True)
        time.sleep(min(60, max(0, deadline - time.monotonic())))
print(f"Observation finished: {issues} issues; {args.output}")
raise SystemExit(1 if issues else 0)
