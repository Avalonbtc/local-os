"""Linux integration tests use real screen sessions and local HTTP test miners only."""
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import socket
import tarfile
import tempfile
import time
import unittest
import uuid
import hashlib
from unittest.mock import patch

RUNTIME = Path(__file__).resolve().parents[1] / "runtime/rig-runtime.py"
MOCK = b'''#!/usr/bin/env python3
import http.server,json,sys,time
started=time.time()
port=int(sys.argv[1]); mode=sys.argv[2] if len(sys.argv)>2 else 'positive'
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  body=json.dumps({'version':'test-1','algo':'rx/0','uptime':int(time.time()-started),'hashrate':{'total':[12500 if mode=='positive' else 0]},'results':{'shares_good':1,'shares_total':1},'connection':{'pool':'local-test','uptime':10}}).encode()
  self.send_response(200);self.end_headers();self.wfile.write(body)
 def log_message(self,*args): pass
http.server.HTTPServer(('127.0.0.1',port),Handler).serve_forever()
'''


def port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait(predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError("Condition did not become true")


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rigdeck-test-")
        os.environ["RIG_RUNTIME_ROOT"] = self.temp.name
        os.environ["RIG_SNAPSHOT_DIR"] = os.path.join(self.temp.name, "run")
        spec = importlib.util.spec_from_file_location("runtime", RUNTIME)
        self.rt = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.rt)

    def tearDown(self):
        for path in (self.rt.ROOT / "instances").glob("*"):
            with self.subTest(cleanup=path.name):
                self.rt.stop(path.name)
        self.temp.cleanup()

    def test_stop_survives_process_exit_between_identity_and_signal(self):
        self.rt.update("race", child_pid=9999999, child_start="old", supervisor_pid=9999998, supervisor_start="old", desired="running")
        with patch.object(self.rt, "owned", return_value=True), patch.object(self.rt.time, "monotonic", side_effect=[0, 20]), patch.object(self.rt.time, "sleep"), patch.object(self.rt.os, "killpg", side_effect=ProcessLookupError), patch.object(self.rt.os, "kill", side_effect=ProcessLookupError), patch.object(self.rt, "screen_command"), patch.object(self.rt, "current_config", return_value={}):
            self.rt.stop("race")
        self.assertEqual(self.rt.state("race")["phase"], "stopped")
        self.assertIsNone(self.rt.state("race")["child_pid"])

    def test_unchanged_state_does_not_fsync(self):
        self.rt.update("stable", desired="stopped", bad_reason=None, bad_since=None)
        with patch.object(self.rt, "atomic") as write:
            self.rt.update("stable", desired="stopped", bad_reason=None, bad_since=None)
            write.assert_not_called()

    def test_gc_preserves_current_rollback_and_replay_tombstone(self):
        name="gc"; now=time.time()
        self.rt.update(name, version="current", committed_version="current", desired="stopped")
        for i, version in enumerate(("old", "rollback", "current")):
            d=self.rt.instance(name)/"versions"/version;d.mkdir(parents=True)
            self.rt.atomic(d/"config.json", {"sha256": str(i)*64})
            os.utime(d,(now-10*86400+i,now-10*86400+i))
        op=str(uuid.uuid4());payload={"kind":"command","script":"do not repeat"}
        d=self.rt.opdir(op);self.rt.atomic(d/"state.json",dict(status="succeeded",payload=payload,finished_at=now-31*86400))
        (d/"output.log").write_text("large output")
        self.rt.cleanup_runtime(now)
        self.assertFalse((self.rt.instance(name)/"versions"/"old").exists())
        self.assertTrue((self.rt.instance(name)/"versions"/"rollback").exists())
        self.assertFalse((d/"output.log").exists())
        with patch.object(self.rt.subprocess,"Popen") as spawn:
            self.assertEqual(self.rt.operation_start(op,payload)["status"],"succeeded")
            spawn.assert_not_called()
        with self.assertRaisesRegex(ValueError,"reused"):
            self.rt.operation_start(op,{"kind":"command","script":"different"})

    def test_system_does_not_repeat_inventory(self):
        sample=self.rt.collect_system()
        self.assertNotIn("topology",sample)
        self.assertNotIn("board",sample)
        self.assertIn("cpu_temperatures",sample)
        self.assertIn("topology",self.rt.collect_inventory())

    def test_tracker_indexer_does_not_block_flight_sheet(self):
        for executable in ("/usr/libexec/tracker-miner-fs-3", "/usr/libexec/tracker-miner-fs-3 (deleted)",
                           "/usr/lib/tracker/tracker-miner-fs", "/usr/libexec/tracker-miner-rss-3"):
            self.assertFalse(self.rt.is_miner_executable(executable))
        for executable in ("/opt/xmrig", "/opt/cpuminer", "/opt/SRBMiner-MULTI",
                           "/tmp/tracker-miner-fs-3", "/usr/libexec/tracker-miner-fs-3-custom"):
            self.assertTrue(self.rt.is_miner_executable(executable))
        original_glob = Path.glob
        def fake_glob(path, pattern):
            return iter([Path("/proc/4784")]) if str(path) == "/proc" else original_glob(path, pattern)
        with patch.object(Path, "glob", fake_glob), patch.object(self.rt.os, "readlink", return_value="/usr/libexec/tracker-miner-fs-3"):
            self.assertEqual(self.rt.collect_software()["processes"], [])
        with patch.object(Path, "glob", fake_glob), patch.object(self.rt.os, "readlink", return_value="/opt/xmrig"), patch.object(self.rt.os, "getpgid", return_value=4784), patch.object(self.rt, "identity", return_value="123"):
            self.assertTrue(self.rt.collect_software()["processes"][0]["adoption_required"])
            with self.assertRaisesRegex(ValueError, "Unmanaged miner"):
                self.rt.apply_deployment({"instances": []}, "test")

    def test_xmrig_proxy_is_not_an_unmanaged_miner(self):
        for executable in ("/home/avalon/xmrig-proxy/xmrig-proxy", "/opt/xmrig-proxy",
                           "/opt/xmrig-proxy (deleted)"):
            self.assertFalse(self.rt.is_miner_executable(executable))
        for executable in ("/opt/xmrig", "/opt/xmrig-proxy/xmrig", "/opt/xmrig-proxy-miner"):
            self.assertTrue(self.rt.is_miner_executable(executable))
        original_glob = Path.glob
        def fake_glob(path, pattern):
            return iter([Path("/proc/10827")]) if str(path) == "/proc" else original_glob(path, pattern)
        with patch.object(Path, "glob", fake_glob), patch.object(self.rt.os, "readlink",
                return_value="/home/avalon/xmrig-proxy/xmrig-proxy"):
            self.assertEqual(self.rt.collect_software()["processes"], [])

    def test_optional_hardware_missing_and_cpu_sensor_filter(self):
        base = Path(self.temp.name) / "hardware"
        base.mkdir()
        missing = self.rt.collect_hardware(base)
        self.assertIsNone(missing["cpu_temperature"])
        self.assertIsNone(missing["board"])
        facts = {"etc/os-release": 'PRETTY_NAME="Ubuntu test"',
                 "proc/sys/kernel/osrelease": "test-kernel",
                 "sys/class/dmi/id/board_name": "Dual EPYC fixture",
                 "sys/class/hwmon/hwmon0/name": "k10temp",
                 "sys/class/hwmon/hwmon0/temp1_input": "65000",
                 "sys/class/hwmon/hwmon0/temp1_label": "Tctl",
                 "sys/class/hwmon/hwmon0/temp2_input": "broken",
                 "sys/class/hwmon/hwmon1/name": "nvme",
                 "sys/class/hwmon/hwmon1/temp1_input": "90000"}
        for name, value in facts.items():
            target = base / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(value)
        actual = self.rt.collect_hardware(base)
        self.assertEqual(actual["cpu_temperature"], 65)
        self.assertEqual(actual["cpu_temperatures"][0]["label"], "Tctl")
        self.assertEqual(actual["board"], "Dual EPYC fixture")
        self.assertEqual(actual["os"], "Ubuntu test")
        self.assertEqual(actual["kernel"], "test-kernel")

    def test_gpu_inventory_excludes_bmc_display_and_keeps_missing_temperature_unknown(self):
        base = Path(self.temp.name) / "gpu-fixture"
        cards = {
            "0000:00:01.0": ("0x1a03", "0x2000", "0x030000", "45000"),
            "0000:01:00.0": ("0x10de", "0x2684", "0x030200", "87000"),
            "0000:02:00.0": ("0x1002", "0x744c", "0x030000", None),
            "0000:03:00.0": ("0x8086", "0x1234", "0x020000", None),
        }
        for card, (vendor, device, device_class, temperature) in cards.items():
            directory = base / "sys/bus/pci/devices" / card
            directory.mkdir(parents=True)
            (directory / "vendor").write_text(vendor)
            (directory / "device").write_text(device)
            (directory / "class").write_text(device_class)
            if temperature:
                sensor = directory / "hwmon/hwmon0/temp1_input"
                sensor.parent.mkdir(parents=True)
                sensor.write_text(temperature)
        gpus = self.rt.collect_gpus(base)
        self.assertEqual([gpu["id"] for gpu in gpus], ["0000:01:00.0", "0000:02:00.0"])
        self.assertEqual(gpus[0]["temperature_c"], 87)
        self.assertIsNone(gpus[1]["temperature_c"])

    def package(self, files):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as tar:
            for name, data in files.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                info.mode = 0o755
                tar.addfile(info, io.BytesIO(data))
        data = buffer.getvalue()
        digest = hashlib.sha256(data).hexdigest()
        (self.rt.ROOT / "cache").mkdir(exist_ok=True)
        (self.rt.ROOT / "cache" / f"{digest}.tar").write_bytes(data)
        return digest

    def config(self, name="cpu1", mode="positive"):
        digest = self.package({"mock-miner.py": MOCK})
        return {"instance": name, "adapter": "xmrig", "sha256": digest,
                "api_port": port(), "argv": ["mock-miner.py", "%API_PORT%", mode],
                "algorithm": "rx/0", "coin": "CUSTOM", "warmup_seconds": 0,
                "verification_seconds": 3, "policy": {"cooldown_seconds": 0.2, "max_restarts": 3}}

    def start(self, cfg):
        version = self.rt.prepare(cfg)
        self.rt.start(cfg["instance"], version)
        wait(lambda: self.rt.state(cfg["instance"]).get("child_pid"))
        wait(lambda: self.rt.sample(cfg["instance"]).get("stats"))
        return version

    def test_screen_survives_caller_and_manual_stop_is_final(self):
        cfg = self.config()
        self.start(cfg)
        self.assertTrue(self.rt.screen_alive("cpu1"))
        self.assertEqual(self.rt.sample("cpu1")["stats"]["hashrate_hs"], 12500)
        self.rt.stop("cpu1")
        self.rt.watchdog_once()
        self.assertEqual(self.rt.state("cpu1")["desired"], "stopped")
        self.assertFalse(self.rt.screen_alive("cpu1"))
        self.assertFalse(self.rt.sample("cpu1")["process_alive"])

    def test_crashed_child_recovers_without_controller(self):
        self.start(self.config())
        before = self.rt.state("cpu1")
        os.kill(before["child_pid"], signal.SIGKILL)
        wait(lambda: self.rt.state("cpu1").get("child_pid") not in (None, before["child_pid"]))
        self.assertTrue(self.rt.state("cpu1")["restarts"])

    def test_stop_marker_suppresses_exit_loop(self):
        self.start(self.config())
        self.rt.update("cpu1", desired="stopped")
        (self.rt.instance("cpu1") / "STOP").touch()
        os.kill(self.rt.state("cpu1")["child_pid"], signal.SIGKILL)
        wait(lambda: not self.rt.screen_alive("cpu1"))
        self.rt.watchdog_once()
        self.assertFalse(self.rt.screen_alive("cpu1"))

    def test_watchdog_low_hashrate_and_recovery_budget(self):
        cfg = self.config(mode="zero")
        cfg["policy"].update(failure_seconds=0, cooldown_seconds=0, max_restarts=1)
        self.start(cfg)
        first = self.rt.state("cpu1")["child_pid"]
        self.rt.watchdog_once()
        wait(lambda: self.rt.state("cpu1").get("child_pid") not in (None, first))
        wait(lambda: self.rt.sample("cpu1").get("stats"))
        self.rt.watchdog_once()
        self.assertEqual(self.rt.state("cpu1")["phase"], "faulted")
        self.assertFalse((self.rt.ROOT / "last-reboot.json").exists())

    def test_apply_failure_restores_previous_version(self):
        cfg = self.config()
        version = self.start(cfg)
        bad = {**cfg, "argv": ["mock-miner.py", "%API_PORT%", "zero"], "verification_seconds": 1}
        operation = str(uuid.uuid4())
        self.rt.atomic(self.rt.opdir(operation) / "state.json", {"status": "running"})
        with self.assertRaisesRegex(RuntimeError, "verification timed out"):
            self.rt.apply_deployment({"instances": [bad], "sheet_id": str(uuid.uuid4()), "sheet_name": "test", "sheet_version": 1}, operation)
        self.assertEqual(self.rt.state("cpu1")["version"], version)
        wait(lambda: (self.rt.sample("cpu1").get("stats") or {}).get("hashrate_hs") == 12500)

    def test_duplicate_remote_command_runs_once_and_cancel_terminates_owned_group(self):
        operation = str(uuid.uuid4())
        marker = self.rt.ROOT / "counter"
        payload = {"kind": "command", "script": f"echo one >> '{marker}'; sleep 1", "timeout_seconds": 10}
        self.rt.operation_start(operation, payload)
        self.rt.operation_start(operation, payload)
        wait(lambda: self.rt.operation_status(operation)["status"] == "succeeded")
        self.assertEqual(marker.read_text().splitlines(), ["one"])
        second = str(uuid.uuid4())
        self.rt.operation_start(second, {"kind": "command", "script": "sleep 30", "timeout_seconds": 60})
        wait(lambda: self.rt.read(self.rt.opdir(second) / "state.json").get("child_pid"))
        (self.rt.opdir(second) / "CANCEL").touch()
        wait(lambda: self.rt.operation_status(second)["status"] == "cancelled")
        record = self.rt.read(self.rt.opdir(second) / "state.json")
        self.assertFalse(self.rt.owned(record["child_pid"], record["child_start"]))

    def test_archive_traversal_rejected_before_extraction(self):
        digest = self.package({"../escape": b"bad"})
        with self.assertRaisesRegex(ValueError, "escapes"):
            self.rt.safe_extract(self.rt.ROOT / "cache" / f"{digest}.tar", self.rt.ROOT / "unpack")
        self.assertFalse((self.rt.ROOT / "escape").exists())

    def test_custom_package_sources_stats_and_isolates_absolute_paths(self):
        Path("/hive").mkdir(exist_ok=True)
        Path("/var/log/miner").mkdir(parents=True, exist_ok=True)
        digest = self.package({
            "example/h-manifest.conf": b'CUSTOM_NAME="example"\nCUSTOM_VERSION="test-1"\n',
            "example/h-config.sh": b'printf "%s" "$CUSTOM_TEMPLATE" > /hive/miners/custom/example/wallet.txt\n',
            "example/h-run.sh": b'exec python3 /hive/miners/custom/example/mock-miner.py "$CUSTOM_API_PORT"\n',
            "example/h-stats.sh": b'local sample=12.5; khs=$sample; stats=\'{"algo":"rx/0","ver":"test-1","ar":[1,0],"connected":true}\'\n',
            "example/h-stop.sh": b'touch /hive/miners/custom/example/stopped\n',
            "example/mock-miner.py": MOCK,
        })
        for name in ("custom1", "custom2"):
            cfg = {**self.config(name), "adapter": "hive-custom", "sha256": digest,
                   "environment": {"CUSTOM_TEMPLATE": f"wallet-{name}"},
                   "capabilities": {"instance_api_port": True}}
            version = self.rt.prepare(cfg)
            (self.rt.instance(name) / "logs/custom").mkdir(parents=True)
            self.rt.run_in_instance(name, "config", version=version)
            self.rt.start(name, version)
            wait(lambda: self.rt.state(name).get("child_pid"))
            cache = self.rt.sample(name)
            self.assertEqual(cache["stats"]["hashrate_hs"], 12500)
            wallet = self.rt.instance(name) / "versions" / version / "hive/miners/custom/example/wallet.txt"
            self.assertEqual(wallet.read_text(), f"wallet-{name}")
        self.rt.stop("custom1")
        self.assertTrue(self.rt.sample("custom2")["process_alive"])

    def test_standard_custom_package_needs_no_user_version_algorithm_or_port_declaration(self):
        digest = self.package({
            "example/h-manifest.conf": b'CUSTOM_NAME="example"\nCUSTOM_VERSION="1.2"\n',
            "example/h-config.sh": b'true\n',
            "example/h-run.sh": b'sleep 60\n',
            "example/h-stats.sh": b'khs=1; stats=\'{"ar":[0,0]}\'\n',
        })
        cfg = {**self.config("custom1"), "adapter": "hive-custom", "sha256": digest,
               "custom_name": "display-name-from-url", "algorithm": "", "capabilities": {}}
        version = self.rt.prepare(cfg)
        metadata = self.rt.read(self.rt.instance("custom1") / "versions" / version / "package-meta.json")
        self.assertEqual(metadata["name"], "example")
        self.assertIsNone(self.rt.normalize(cfg, {"khs": "1", "stats": {}})["algorithm"])
        operation = str(uuid.uuid4())
        self.rt.atomic(self.rt.opdir(operation) / "state.json", {"status": "running"})
        result = self.rt.apply_deployment({"instances": [cfg], "sheet_id": str(uuid.uuid4()),
                                         "sheet_name": "optional-fields", "sheet_version": 1}, operation)
        self.assertTrue(result["validation"]["custom1"]["positive_hashrate"])
        self.assertIsNone(result["validation"]["custom1"]["algorithm_match"])
        self.assertIsNone(result["validation"]["custom1"]["pool_connected"])
        self.assertEqual(result["validation"]["custom1"]["accepted_shares"], 0)
        self.assertTrue(self.rt.screen_alive("custom1"))
        self.assertEqual(self.rt.state("custom1")["desired"], "running")

    def test_hashrate_units_not_guessed(self):
        cfg = {"adapter": "hive-custom", "algorithm": "rx/0"}
        self.assertEqual(self.rt.normalize(cfg, {"khs": "2", "stats": {}})["hashrate_hs"], 2000)
        self.assertEqual(self.rt.normalize(cfg, {"stats": {"hs": [2, 3], "hs_units": "mhs"}})["hashrate_hs"], 5e6)
        with self.assertRaises(ValueError):
            self.rt.normalize(cfg, {"stats": {"hs": [2], "hs_units": "unknown"}})

    def test_native_stats_contracts_use_observed_algorithm_and_version(self):
        srb = self.rt.normalize({"adapter": "srbminer", "algorithm": "randomx"}, {
            "miner_version": "3.0.0", "mining_time": 120,
            "algorithms": [{"name": "randomx", "hashrate": {"now": 15000},
                            "shares": {"accepted": 3, "rejected": 1}}]})
        self.assertEqual(srb["hashrate_hs"], 15000)
        self.assertEqual(srb["version"], "3.0.0")
        cpu = self.rt.normalize({"adapter": "cpuminer-opt", "algorithm": "sha256d"},
                                {"VER": "25.3", "ALGO": "sha256d", "KHS": "320.5", "ACC": "2"})
        self.assertEqual(cpu["hashrate_hs"], 320500)
        self.assertEqual(cpu["algorithm"], "sha256d")
        custom = self.rt.normalize({"adapter": "hive-custom", "algorithm": "configured-only"}, {"khs": "3", "stats": {}})
        self.assertIsNone(custom["algorithm"])

    def test_boot_recovery_restores_removed_instance_intent(self):
        self.start(self.config())
        previous = self.rt.state("cpu1")
        operation = str(uuid.uuid4())
        self.rt.atomic(self.rt.opdir(operation) / "state.json", {
            "status": "running", "payload": {"kind": "apply"},
            "previous": {"cpu1": previous}, "versions": {}})
        self.rt.update("cpu1", switching=operation)
        self.rt.stop("cpu1", desired="stopped", maintenance=True)
        self.rt.recover_interrupted_deployments()
        restored = self.rt.state("cpu1")
        self.assertEqual(restored["desired"], "running")
        self.assertFalse(restored["maintenance"])
        self.assertEqual(self.rt.read(self.rt.opdir(operation) / "state.json")["status"], "failed")
        self.rt.start("cpu1", preserve=True)
        wait(lambda: self.rt.sample("cpu1")["process_alive"])

    def test_adoption_rejects_changed_process_identity(self):
        with self.assertRaisesRegex(ValueError, "identity changed"):
            self.rt.adopt({"name": "legacy", "pid": os.getpid(), "start_identity": "wrong"})


if __name__ == "__main__":
    unittest.main(verbosity=2)
