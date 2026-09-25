"""Snapshot, message feed, log tail and policy push. No screen session or root required."""
import base64
import http.server
import importlib.util
import json
import os
from pathlib import Path
import socket
import stat
import tempfile
import threading
import unittest
from unittest.mock import patch

RUNTIME = Path(__file__).resolve().parents[1] / "runtime/rig-runtime.py"


def load(root, snapshots):
    os.environ["RIG_RUNTIME_ROOT"] = str(root)
    os.environ["RIG_SNAPSHOT_DIR"] = str(snapshots)
    spec = importlib.util.spec_from_file_location(f"rig_runtime_{id(root)}", RUNTIME)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.ROOT.mkdir(parents=True, exist_ok=True)
    return module


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        base = Path(self.temp.name)
        self.rt = load(base / "state", base / "run")

    def tearDown(self):
        self.temp.cleanup()

    def test_snapshot_contains_everything_the_controller_polls(self):
        self.rt.emit("warning", "watchdog_restart", "看门狗重启矿工", "xmr", reason="low_hashrate")
        (self.rt.ROOT / "instances/xmr").mkdir(parents=True)
        self.rt.atomic_volatile(self.rt.ROOT / "instances/xmr/stats.json", {"instance": "xmr", "boot_id": self.rt.boot_id(), "sample_uptime": self.rt.uptime(), "process_alive": True, "stats": {"hashrate_hs": 5}})
        (self.rt.ROOT / "instances/old").mkdir(parents=True)
        self.rt.atomic_volatile(self.rt.ROOT / "instances/old/stats.json", {"instance": "old", "boot_id": "previous-boot", "process_alive": True, "stats": {"hashrate_hs": 5}, "stats_observed_at": 1})
        snapshot = self.rt.publish_snapshot()
        on_disk = json.loads((self.rt.SNAPSHOT_DIR / "snapshot.json").read_text())
        self.assertEqual(on_disk["schema"], 1)
        self.assertEqual(snapshot["events"][-1]["kind"], "watchdog_restart")
        self.assertEqual(snapshot["events"][-1]["detail"], {"reason": "low_hashrate"})
        by_name = {i["instance"]: i for i in snapshot["mining"]["instances"]}
        self.assertTrue(by_name["xmr"]["process_alive"])
        self.assertFalse(by_name["old"]["process_alive"], "a sample from an earlier boot is never live")
        self.assertIsNone(by_name["old"]["stats"])
        self.assertIn("cpu_pct", snapshot["system"])
        self.assertNotIn("topology", snapshot["system"])
        mode = stat.S_IMODE(os.stat(self.rt.SNAPSHOT_DIR / "snapshot.json").st_mode)
        self.assertEqual(mode, 0o600, "without a configured reader only root may read it")

    def test_snapshot_is_owned_by_the_configured_ssh_reader(self):
        self.rt.atomic(self.rt.ROOT / "reader.json", {"user": "rigdeck-reader"})
        fake = type("pw", (), {"pw_uid": os.getuid()})
        with patch.object(self.rt.pwd, "getpwnam", return_value=fake):
            self.rt.publish_snapshot()
        info = os.stat(self.rt.SNAPSHOT_DIR / "snapshot.json")
        self.assertEqual(stat.S_IMODE(info.st_mode), 0o400)
        self.assertEqual(info.st_uid, os.getuid())

    def test_event_log_is_bounded_and_tail_is_parsed(self):
        for n in range(3000):
            self.rt.emit("info", "test", "x" * 400 + str(n))
        self.assertLessEqual(self.rt.EVENTS.stat().st_size, 1024 * 1024 + 4096)
        events = self.rt.recent_events(10)
        self.assertEqual(len(events), 10)
        self.assertTrue(events[-1]["message"].endswith("2999"))
        self.assertEqual(len({e["id"] for e in events}), 10)
        self.assertEqual(sorted(e["seq"] for e in events), [e["seq"] for e in events])

    def test_log_tail_strips_terminal_sequences(self):
        directory = self.rt.ROOT / "instances/xmr"
        directory.mkdir(parents=True)
        (directory / "console.log").write_bytes(b"\x1b[32mold\x1b[0m\r\n" + b"".join(b"line %d\n" % n for n in range(10)))
        tail = self.rt.log_tail("xmr", 3)
        self.assertEqual(tail["text"], "line 7\nline 8\nline 9")
        self.assertEqual(self.rt.log_tail("missing")["text"], "")
        with self.assertRaises(ValueError):
            self.rt.log_tail("../etc")

    def test_pushed_policy_overrides_flight_snapshot_policy(self):
        with self.assertRaises(ValueError):
            self.rt.set_policy({"digest": "short", "policy": {}})
        self.rt.set_policy({"digest": "a" * 64, "policy": {"max_restarts": 2, "watchdog_enabled": False}})
        policy = self.rt.effective_policy({"policy": {"max_restarts": 5, "cooldown_seconds": 9}})
        self.assertEqual(policy, {"max_restarts": 2, "cooldown_seconds": 9, "watchdog_enabled": False})
        self.assertEqual(self.rt.publish_snapshot()["policy_digest"], "a" * 64)

    def test_disabled_watchdog_never_restarts_for_low_hashrate(self):
        name = "xmr"
        (self.rt.ROOT / "instances" / name).mkdir(parents=True)
        self.rt.set_policy({"digest": "b" * 64, "policy": {"watchdog_enabled": False, "failure_seconds": 0}})
        state = {"desired": "running", "supervisor_pid": os.getpid(), "supervisor_start": self.rt.identity(os.getpid()), "bad_reason": "low_hashrate", "bad_since": 1}
        self.rt.atomic(self.rt.ROOT / "instances" / name / "state.json", state)
        with patch.object(self.rt, "sample", return_value={"process_alive": True, "stats": {"hashrate_hs": 0}}), \
                patch.object(self.rt, "current_config", return_value={"adapter": "xmrig", "policy": {}}), \
                patch.object(self.rt, "restart") as restart:
            self.rt.watchdog_once()
        restart.assert_not_called()
        self.assertIsNone(self.rt.state(name).get("bad_reason"))

    def test_native_stats_are_read_in_process(self):
        body = json.dumps({"hashrate": {"total": [100]}, "algo": "rx/0", "version": "6", "results": {"shares_good": 2, "shares_total": 3}, "connection": {"pool": "p", "uptime": 5}}).encode()

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass
        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            config = {"adapter": "xmrig", "api_port": server.server_address[1], "algorithm": "rx/0"}
            with patch.object(self.rt.subprocess, "run", side_effect=AssertionError("no subprocess")):
                stats = self.rt.normalize(config, self.rt.native_stats(config))
            self.assertEqual((stats["hashrate_hs"], stats["accepted"], stats["rejected"]), (100, 2, 1))
        finally:
            server.shutdown()
            server.server_close()

    def test_system_sample_does_not_upload_disks_or_network_interfaces(self):
        system = self.rt.collect_system()
        for key in ("disks", "network", "interfaces"):
            self.assertNotIn(key, system)
        self.assertIn("cpu_pct", system)
        self.assertIn("memory_pct", system)
        self.assertNotIn("interfaces", self.rt.collect_hardware())



class FetchPackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        base = Path(self.temp.name)
        self.rt = load(base / "state", base / "run")
        self.payload = b"miner package bytes" * 1000
        payload = self.payload

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path != "/pkg.tar":
                    self.send_response(404)
                    self.end_headers()
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass
        self.server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.temp.cleanup()

    def test_pinned_download_lands_in_cache_and_is_reused(self):
        import hashlib
        digest = hashlib.sha256(self.payload).hexdigest()
        self.assertEqual(self.rt.fetch_package(digest, f"{self.url}/pkg.tar"), digest)
        target = self.rt.ROOT / "cache" / f"{digest}.tar"
        self.assertEqual(target.read_bytes(), self.payload)
        self.assertEqual(stat.S_IMODE(os.stat(target).st_mode), 0o600)
        self.server.shutdown()  # reuse must not touch the network
        self.assertEqual(self.rt.fetch_package(digest, f"{self.url}/pkg.tar"), digest)

    def test_unpinned_requires_https_and_is_indexed_by_url(self):
        import hashlib
        with self.assertRaisesRegex(ValueError, "pinned SHA256"):
            self.rt.fetch_package("-", f"{self.url}/pkg.tar")
        # Simulate an earlier HTTPS download recorded in the URL index.
        digest = hashlib.sha256(self.payload).hexdigest()
        cache = self.rt.ROOT / "cache"
        cache.mkdir(parents=True)
        (cache / f"{digest}.tar").write_bytes(self.payload)
        (cache / "url-index.json").write_text(json.dumps({"https://github.com/x.tar.gz": digest}))
        self.assertEqual(self.rt.fetch_package("-", "https://github.com/x.tar.gz"), digest)
        # A tampered cache entry is not trusted: it would re-download (and fail offline here).
        (cache / f"{digest}.tar").write_bytes(b"tampered")
        with self.assertRaises(RuntimeError):
            self.rt.fetch_package("-", "https://127.0.0.1:1/x.tar.gz")

    def test_wrong_digest_or_missing_file_leaves_nothing_behind(self):
        with self.assertRaisesRegex(ValueError, "SHA256 不匹配"):
            self.rt.fetch_package("0" * 64, f"{self.url}/pkg.tar")
        with self.assertRaisesRegex(RuntimeError, "下载失败"):
            self.rt.fetch_package("1" * 64, f"{self.url}/missing.tar")
        self.assertEqual([p.name for p in (self.rt.ROOT / "cache").iterdir()], [])
        with self.assertRaises(ValueError):
            self.rt.fetch_package("1" * 64, "file:///etc/passwd")


class GpuAndMixedMiningTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        base = Path(self.temp.name)
        self.rt = load(base / "state", base / "run")
        self.base = base / "root"

    def tearDown(self):
        self.temp.cleanup()

    def card(self, name, vendor, files):
        directory = self.base / "sys/bus/pci/devices" / name
        directory.mkdir(parents=True)
        for key, value in {"vendor": vendor, "device": "0x744c", "class": "0x030000", **files}.items():
            path = directory / key
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)

    def test_amd_gpu_tile_readings_from_sysfs(self):
        self.card("0000:03:00.0", "0x1002", {"hwmon/hwmon2/temp1_input": "61000", "hwmon/hwmon2/pwm1": "128",
                                             "hwmon/hwmon2/power1_average": "180000000", "gpu_busy_percent": "99",
                                             "hwmon/hwmon2/temp3_label": "mem", "hwmon/hwmon2/temp3_input": "86000",
                                             "hwmon/hwmon2/in0_input": "700", "mem_info_vram_total": str(8176 * 1048576),
                                             "pp_dpm_sclk": "0: 500Mhz\n1: 1050Mhz *\n", "pp_dpm_mclk": "0: 96Mhz\n1: 1120Mhz *\n"})
        self.card("0000:04:00.0", "0x1002", {})
        gpus = self.rt.collect_gpus(self.base)
        self.assertEqual(gpus[0]["bus"], 3)
        self.assertEqual((gpus[0]["temperature_c"], gpus[0]["fan_pct"], gpus[0]["power_w"], gpus[0]["util_pct"]), (61, 50.2, 180, 99))
        self.assertEqual((gpus[0]["memory_temperature_c"], gpus[0]["core_mhz"], gpus[0]["mem_mhz"], gpus[0]["core_mv"], gpus[0]["vram_mb"]),
                         (86, 1050, 1120, 700, 8176))
        # Missing sensors stay unknown, never zero.
        self.assertEqual([gpus[1][k] for k in ("temperature_c", "fan_pct", "power_w", "util_pct")], [None] * 4)

    def test_hive_custom_gpu_miner_reports_per_card_hashrate(self):
        raw = {"khs": 120000, "stats": {"hs": [60, 60.5], "hs_units": "mhs", "bus_numbers": [1, 2], "algo": "kawpow", "ar": [5, 0]}}
        stats = self.rt.normalize({"adapter": "hive-custom", "coin": "RVN"}, raw)
        self.assertEqual(stats["device"], "gpu")
        self.assertEqual(stats["gpu_hs"], [{"bus": 1, "hs": 60e6}, {"bus": 2, "hs": 60.5e6}])
        cpu = self.rt.normalize({"adapter": "hive-custom", "coin": "XMR"}, {"khs": 68, "stats": {"hs": [68], "algo": "rx/0"}})
        self.assertEqual((cpu["device"], cpu["gpu_hs"]), ("cpu", []))

    def test_srbminer_splits_cpu_and_gpu(self):
        raw = {"miner_version": "3.4.6", "gpu_devices": [{"device": "gpu0", "bus_id": 5}],
               "algorithms": [{"name": "randomx", "hashrate": {"cpu": {"total": 1000}, "gpu": {"total": 2000, "gpu0": 2000}},
                               "shares": {"accepted": 1, "rejected": 0}, "pool": {"connected": True}}]}
        stats = self.rt.normalize({"adapter": "srbminer", "algorithm": "randomx"}, raw)
        self.assertEqual(stats["device"], "cpu+gpu")
        self.assertEqual(stats["hashrate_hs"], 3000)
        self.assertEqual(stats["gpu_hs"], [{"bus": 5, "hs": 2000}])

if __name__ == "__main__":
    unittest.main()
