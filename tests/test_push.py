"""Watchdog telemetry push: compression, signing, patches, resync, events, policy and set-push.

The fake controller below follows crates/application/src/push.rs. The patch cases are also
written to tests/fixtures/snapshot-patches.json, which the Rust apply_patch test replays, so the
two sides of the diff format cannot drift apart.
"""
import copy
import hashlib
import hmac
import http.server
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
import uuid
import zlib

RUNTIME = Path(__file__).resolve().parents[1] / "runtime/rig-runtime.py"
FIXTURE = Path(__file__).resolve().parent / "fixtures/snapshot-patches.json"
KEY = "ab" * 32


def load(root, snapshots):
    os.environ["RIG_RUNTIME_ROOT"] = str(root)
    os.environ["RIG_SNAPSHOT_DIR"] = str(snapshots)
    spec = importlib.util.spec_from_file_location(f"rig_runtime_push_{id(root)}", RUNTIME)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.ROOT.mkdir(parents=True, exist_ok=True)
    return module


def apply_patch(target, change):
    """Python mirror of the controller's apply_patch."""
    if isinstance(target, dict) and isinstance(change, dict):
        result = dict(target)
        for key in change.get("$del", []):
            result.pop(key, None)
        for key, value in change.items():
            if key != "$del":
                result[key] = apply_patch(result[key], value) if key in result else value
        return result
    if isinstance(target, list) and isinstance(change, dict) and "$items" in change:
        result = list(target)
        for index, value in change["$items"].items():
            result[int(index)] = apply_patch(result[int(index)], value)
        return result
    return change


class Controller:
    def __init__(self, key=KEY):
        self.key, self.epoch, self.base = key, uuid.uuid4().hex, None
        self.messages, self.sizes, self.stored, self.events = [], [], [], {}
        self.policy = None
        self.bad_reply = False
        controller = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                body = self.rfile.read(int(self.headers["Content-Length"]))
                status, reply = controller.handle(self.headers, body)
                data = json.dumps(reply).encode()
                self.send_response(status)
                self.send_header("Content-Length", str(len(data)))
                prefix = f"rigdeck-push-reply-v1\n{self.headers['X-Rig-Signature']}\n{status}\n".encode()
                signature = hmac.new(bytes.fromhex(controller.key), prefix + data, hashlib.sha256).hexdigest()
                self.send_header("X-Rig-Reply-Signature", "bad" if controller.bad_reply else signature)
                self.end_headers()
                self.wfile.write(data)

        self.server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_port}/ingest/v1/push"

    def handle(self, headers, body):
        expected = hmac.new(bytes.fromhex(self.key), body, hashlib.sha256).hexdigest()
        if not hmac.compare_digest(expected, headers.get("X-Rig-Signature", "")):
            return 401, {"error": "bad signature"}
        assert headers["Content-Encoding"] == "deflate"
        message = json.loads(zlib.decompress(body))
        self.messages.append(message)
        self.sizes.append(len(body))
        if message["epoch"] != self.epoch:
            return 409, {"epoch": self.epoch, "need": "full"}
        if "snapshot" in message:
            snapshot = message["snapshot"]
        elif self.base and self.base[0] == message["base"]:
            snapshot = apply_patch(self.base[1], message["patch"])
        else:
            return 409, {"epoch": self.epoch, "need": "full"}
        self.base = (message["seq"], snapshot)
        self.stored.append(snapshot)
        for event in message.get("events", []):
            self.events[event["id"]] = event
        reply = {"ok": True}
        if self.events:
            reply["events_seq"] = max(e["seq"] for e in self.events.values())
        if self.policy and snapshot.get("policy_digest") != self.policy["digest"]:
            reply["policy"] = self.policy
        return 200, reply

    def restart(self):
        self.epoch, self.base = uuid.uuid4().hex, None

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def snapshot(rt, cards=8, events=0, tick=0):
    """A realistic 8-GPU, two-instance rig snapshot; `tick` moves the live readings."""
    value = {
        "schema": 1, "runtime_version": rt.VERSION, "boot_id": rt.boot_id(), "uptime": rt.uptime(),
        "written_at": 1.7e9 + tick, "policy_digest": None, "push_digest": "d" * 64,
        "system": {"cpu_pct": 37.5 + tick % 3, "cpu_temperature": 61.0 + tick % 2, "logical_cpus": 64,
                   "memory_total": 64 << 30, "memory_used": (20 << 30) + tick * 4096, "memory_pct": 31.25,
                   "swap_total": 0, "swap_used": 0, "load": [12.1 + tick / 10, 11.9, 11.7], "uptime": 86400.0 + tick * 10,
                   "addresses": [{"interface": "enp3s0", "address": "192.168.1.50"}], "runtime_version": rt.VERSION,
                   "cpu_power_w": 180.4 + tick, "cpu_power_source": "Linux powercap / RAPL",
                   "cpu_temperatures": [{"driver": "k10temp", "label": f"Tccd{i}", "celsius": 60.0 + i} for i in range(8)],
                   "gpu_drivers": {"NVIDIA": "550.120"},
                   "gpus": [{"index": i, "vendor": "NVIDIA", "name": "NVIDIA GeForce RTX 3070", "bus_id": f"0000:{i + 1:02x}:00.0",
                             "temperature": 55 + (i + tick) % 4, "fan_pct": 60, "power_w": 120.5 + i, "core_clock": 1500,
                             "memory_clock": 7000, "memory_temperature": None, "memory_total": 8 << 30, "memory_used": 4 << 30}
                            for i in range(cards)]},
        "mining": {"instances": [
            {"instance": name, "boot_id": rt.boot_id(), "sample_uptime": rt.uptime(), "process_alive": True, "desired": "running",
             "phase": "running", "version": "v1", "observed_at": 1.7e9 + tick, "stats_observed_at": 1.7e9 + tick,
             "stats": {"hashrate_hs": 61_000_000.0 + tick * 1000, "algorithm": algorithm, "accepted": 1200 + tick, "rejected": 3,
                       "connected": True, "pool": "stratum+tcp://pool.example:3333", "uptime": 86000 + tick * 10,
                       "gpus": [{"index": i, "hashrate_hs": 7_600_000.0 + i + tick} for i in range(cards)]}}
            for name, algorithm in (("etc", "etchash"), ("xmr", "rx/0"))]},
        "events": [{"id": str(uuid.UUID(int=n + 1)), "seq": 1_700_000_000_000_000_000 + n, "at": 1.7e9, "uptime": 100.0 + n,
                    "boot_id": rt.boot_id(), "level": "info", "kind": "watchdog_restart", "instance": "etc",
                    "message": "看门狗重启矿工：算力过低持续 120 秒", "detail": {"reason": "low_hashrate"}} for n in range(events)],
    }
    return value


class PushTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        base = Path(self.temp.name)
        self.rt = load(base / "state", base / "run")
        self.controller = Controller()
        self.config = {"url": self.controller.url, "machine_id": str(uuid.uuid4()), "key": KEY, "digest": "d" * 64}
        # The real collectors walk /proc and /sys; their output is not what is under test here.
        for name, value in (("collect_software", {"processes": []}), ("collect_inventory", {"cpu_model": "EPYC"})):
            patcher = patch.object(self.rt, name, return_value=value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def tearDown(self):
        self.controller.close()
        self.temp.cleanup()

    def test_resync_then_patches_that_rebuild_the_exact_snapshot(self):
        pusher = self.rt.Pusher()
        first = snapshot(self.rt, events=3)
        self.assertEqual(pusher.push(first, self.config)[0], 200)
        # No epoch yet: the controller answers 409 and the full snapshot follows at once.
        self.assertEqual([("snapshot" in m, m["epoch"]) for m in self.controller.messages],
                         [(True, None), (True, self.controller.epoch)])
        self.assertEqual(len(self.controller.messages[1]["events"]), 3)
        self.assertIn("software", self.controller.messages[1])
        self.assertIn("hardware", self.controller.messages[1])
        for tick in range(1, 5):
            current = snapshot(self.rt, events=3, tick=tick)
            self.assertEqual(pusher.push(current, self.config)[0], 200)
            message = self.controller.messages[-1]
            self.assertNotIn("snapshot", message)
            self.assertEqual(message["base"], self.controller.messages[-2]["seq"])
            self.assertNotIn("events", message, "acknowledged events are not sent again")
            self.assertNotIn("software", message)
            expected = {k: v for k, v in current.items() if k != "events"}
            self.assertEqual(self.controller.stored[-1], expected)
        # 8 GPUs, two instances: about 4.4 KB of JSON per tick before compression.
        patch_size, full_size = self.controller.sizes[-1], self.controller.sizes[1]
        self.assertLess(patch_size, 600, f"patch {patch_size} B")
        self.assertLess(patch_size * 2, full_size, f"patch {patch_size} B vs full {full_size} B")
        # Only a new event goes out.
        later = snapshot(self.rt, events=4, tick=5)
        pusher.push(later, self.config)
        self.assertEqual([e["seq"] for e in self.controller.messages[-1]["events"]], [later["events"][-1]["seq"]])

    def test_keyframe_and_restarted_controller_get_full_snapshots(self):
        pusher = self.rt.Pusher()
        pusher.KEYFRAME = 2
        for tick in range(4):
            pusher.push(snapshot(self.rt, tick=tick), self.config)
        full = ["snapshot" in m for m in self.controller.messages[1:]]
        self.assertEqual(full, [True, False, False, True])
        self.controller.restart()
        self.assertEqual(pusher.push(snapshot(self.rt, tick=9), self.config)[0], 200)
        self.assertEqual([("snapshot" in m) for m in self.controller.messages[-2:]], [False, True])
        self.assertEqual(self.controller.stored[-1]["written_at"], 1.7e9 + 9)

    def test_lost_base_is_resent_in_full(self):
        pusher = self.rt.Pusher()
        pusher.push(snapshot(self.rt), self.config)
        self.controller.base = None  # e.g. the reply to the last push never reached the rig
        self.assertEqual(pusher.push(snapshot(self.rt, tick=1), self.config)[0], 200)
        self.assertEqual(["snapshot" in m for m in self.controller.messages[-2:]], [False, True])

    def test_policy_in_the_reply_is_applied_locally(self):
        self.controller.policy = {"digest": "e" * 64, "policy": {"max_restarts": 3}}
        self.rt.Pusher().push(snapshot(self.rt), self.config)
        saved = json.loads((self.rt.ROOT / "policy.json").read_text())
        self.assertEqual((saved["digest"], saved["policy"]), ("e" * 64, {"max_restarts": 3}))

    def test_rejected_key_backs_off_and_unreachable_controller_is_harmless(self):
        pusher = self.rt.Pusher()
        wrong = {**self.config, "key": "cd" * 32}
        status, _ = pusher.push(snapshot(self.rt), wrong)
        self.assertEqual(status, 401)
        sent = len(self.controller.messages)
        self.assertIsNone(pusher.push(snapshot(self.rt), wrong), "waits a minute after a rejection")
        self.assertEqual(len(self.controller.messages), sent)
        self.assertEqual(pusher.push(snapshot(self.rt), self.config)[0], 200, "new credentials reset backoff")
        self.controller.close()
        offline = self.rt.Pusher()
        offline.TIMEOUT = 1
        self.assertIsNone(offline.push(snapshot(self.rt), self.config))
        self.assertIsNone(self.rt.Pusher().push(snapshot(self.rt), {}), "nothing configured, nothing sent")

    def test_untrusted_response_cannot_change_policy_or_acknowledge(self):
        pusher = self.rt.Pusher()
        pusher.push(snapshot(self.rt), self.config)
        old_base = pusher.base_seq
        self.controller.policy = {"digest": "e" * 64, "policy": {"max_restarts": 0}}
        self.controller.bad_reply = True
        self.assertIsNone(pusher.push(snapshot(self.rt, tick=1), self.config))
        self.assertEqual(pusher.base_seq, old_base)
        self.assertFalse((self.rt.ROOT / "policy.json").exists())
        fresh = self.rt.Pusher()
        self.assertIsNone(fresh.push(snapshot(self.rt), self.config))
        self.assertIsNone(fresh.epoch, "unsigned resync must not be trusted")

    def test_slow_push_worker_keeps_only_latest_pending_sample(self):
        entered, release, done = threading.Event(), threading.Event(), threading.Event()
        seen = []
        class SlowPusher:
            def push(self, value):
                seen.append(value)
                if value == 1:
                    entered.set()
                    release.wait(3)
                else:
                    done.set()
        worker = self.rt.PushWorker(SlowPusher())
        worker.submit(1)
        self.assertTrue(entered.wait(1))
        try:
            worker.submit(2)
            worker.submit(3)
            self.assertEqual(worker.pending.qsize(), 1)
        finally:
            release.set()
        self.assertTrue(done.wait(1))
        self.assertEqual(seen, [1, 3])

    def test_set_push_validates_installs_and_deletes_the_upload(self):
        for name, value in (("bad", {"url": "ftp://x"}), ("short", {**self.config, "key": "ab"})):
            path = Path(f"/tmp/rigdeck-push-{uuid.uuid4()}.json")
            path.write_text(json.dumps(value))
            with self.assertRaises(ValueError, msg=name):
                self.rt.set_push(str(path))
            self.assertFalse(path.exists())
        with self.assertRaises(ValueError):
            self.rt.set_push("/etc/passwd")
        path = Path(f"/tmp/rigdeck-push-{uuid.uuid4()}.json")
        path.write_text(json.dumps({**self.config, "extra": 1}))
        self.assertEqual(self.rt.set_push(str(path))["digest"], "d" * 64)
        self.assertFalse(path.exists())
        installed = json.loads(self.rt.PUSH_FILE.read_text())
        self.assertEqual(installed, self.config)
        self.assertEqual(oct(self.rt.PUSH_FILE.stat().st_mode & 0o777), "0o600")
        self.assertEqual(self.rt.publish_snapshot()["push_digest"], "d" * 64)
        self.assertEqual(self.rt.recent_events()[-1]["kind"], "push_configured")

    def test_patch_cases_match_the_shared_fixture(self):
        cases = [
            ({"a": 1, "b": {"c": [1, 2, 3], "d": "x"}, "e": None}, {"a": 1, "b": {"c": [1, 5, 3]}, "e": 2}),
            ({"l": [1, 2]}, {"l": [1, 2, 3]}),
            ({"o": {"x": 1}}, {"o": 7}),
            ({"s": 1}, {"s": {"t": {"u": None}}}),
            ({"g": [{"t": 50, "f": 1}, {"t": 51}]}, {"g": [{"t": 50}, {"t": 52, "n": None}]}),
            ({"same": [1, {"a": 2}]}, {"same": [1, {"a": 2}]}),
        ]
        live = snapshot(self.rt, events=0)
        later = copy.deepcopy(live)
        later["system"]["gpus"][3]["temperature"] = 80
        later["mining"]["instances"].pop()
        cases.append((live, later))
        recorded = []
        for old, new in cases:
            diff = self.rt.snapshot_diff(old, new)
            self.assertEqual(apply_patch(old, diff), new)
            recorded.append({"old": old, "new": new, "patch": diff})
        if os.environ.get("RIGDECK_WRITE_FIXTURES") == "1" or not FIXTURE.exists():
            FIXTURE.parent.mkdir(exist_ok=True)
            FIXTURE.write_text(json.dumps(recorded, ensure_ascii=False, indent=1) + "\n")
        for case in json.loads(FIXTURE.read_text()):
            self.assertEqual(self.rt.snapshot_diff(case["old"], case["new"]), case["patch"])


if __name__ == "__main__":
    unittest.main()
