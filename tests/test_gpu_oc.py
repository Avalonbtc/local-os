"""GPU overclocking on the rig: HiveOS field semantics against a fake sysfs tree and a fake NVML."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

RUNTIME = Path(__file__).resolve().parents[1] / "runtime/rig-runtime.py"

POLARIS = """OD_SCLK:
0:        300MHz        750mV
1:        600MHz        769mV
7:       1340MHz       1150mV
OD_MCLK:
0:        300MHz        750mV
2:       1750MHz        900mV
OD_RANGE:
SCLK:     300MHz       2000MHz
"""
NAVI10 = """OD_SCLK:
0: 800Mhz
1: 1750Mhz
OD_MCLK:
1: 875MHz
OD_VDDC_CURVE:
0: 800MHz 711mV
2: 1750MHz 1006mV
"""
RDNA2 = """OD_SCLK:
0: 500Mhz
1: 2615Mhz
OD_MCLK:
0: 97Mhz
1: 1000MHz
OD_VDDGFX_OFFSET:
0mV
"""


class GpuOcTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rigdeck-oc-")
        self.addCleanup(self.temp.cleanup)
        os.environ["RIG_RUNTIME_ROOT"] = str(Path(self.temp.name) / "state")
        os.environ["RIG_SNAPSHOT_DIR"] = str(Path(self.temp.name) / "run")
        spec = importlib.util.spec_from_file_location("runtime", RUNTIME)
        self.rt = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.rt)
        self.rt.ROOT.mkdir(parents=True)
        self.base = Path(self.temp.name) / "root"
        self.writes = {}

    def card(self, address, vendor, table=None, cap_max=250):
        device = self.base / "sys/bus/pci/devices" / address
        (device / "hwmon/hwmon0").mkdir(parents=True)
        (device / "vendor").write_text(vendor + "\n")
        (device / "class").write_text("0x030000\n")
        if table is not None:
            (device / "pp_od_clk_voltage").write_text(table)
            (device / "power_dpm_force_performance_level").write_text("auto\n")
            (device / "pp_dpm_sclk").write_text("0: 300Mhz *\n")
            (device / "pp_dpm_mclk").write_text("0: 300Mhz *\n")
            hwmon = device / "hwmon/hwmon0"
            (hwmon / "power1_cap").write_text("150000000\n")
            (hwmon / "power1_cap_max").write_text(f"{cap_max * 1000000}\n")
            (hwmon / "power1_cap_default").write_text("150000000\n")
            (hwmon / "pwm1_enable").write_text("2\n")
            (hwmon / "pwm1").write_text("0\n")
        return device

    def record_writes(self):
        """sysfs writes append instead of replace, so the command sequence can be asserted."""
        def write(path, value):
            self.writes.setdefault(Path(path).name, []).append(str(value))
            if Path(path).name != "pp_od_clk_voltage":
                Path(path).write_text(f"{value}\n")
        return patch.object(self.rt, "_write", side_effect=write)

    def test_hiveos_padding_reuses_the_last_value(self):
        self.assertEqual(self.rt.oc_expand([1200, 1300], 4), [1200, 1300, 1300, 1300])
        self.assertEqual(self.rt.oc_expand(["-200"], 2), [-200, -200])
        self.assertEqual(self.rt.oc_expand([], 3), [None, None, None])

    def test_cards_are_ordered_by_bus_and_filtered_by_vendor(self):
        self.card("0000:0b:00.0", "0x1002", POLARIS)
        self.card("0000:03:00.0", "0x1002", POLARIS)
        self.card("0000:05:00.0", "0x10de")
        bmc = self.card("0000:02:00.0", "0x1a03")  # ASPEED BMC VGA
        self.assertEqual([d.name for d in self.rt.oc_cards("amd", self.base)], ["0000:03:00.0", "0000:0b:00.0"])
        self.assertEqual([d.name for d in self.rt.oc_cards("nvidia", self.base)], ["0000:05:00.0"])
        self.assertTrue(bmc.exists())

    def test_polaris_writes_the_selected_state_and_pins_dpm(self):
        self.card("0000:03:00.0", "0x1002", POLARIS)
        with self.record_writes():
            results = self.rt.apply_amd_oc({"core_clock": [1150], "core_vddc": [850], "core_state": [7],
                                            "mem_clock": [2000], "pl": [120], "fan": [70]}, base=self.base)
        self.assertEqual(results[0]["errors"], [])
        self.assertEqual(self.writes["pp_od_clk_voltage"], ["s 7 1150 850", "m 2 2000 900", "c"])
        self.assertEqual(self.writes["pp_dpm_sclk"], ["7"])
        self.assertEqual(self.writes["power1_cap"], ["120000000"])
        self.assertEqual(self.writes["pwm1_enable"], ["1"])
        self.assertEqual(self.writes["pwm1"], [str(70 * 255 // 100)])

    def test_navi10_sets_clock_and_voltage_curve_point(self):
        self.card("0000:03:00.0", "0x1002", NAVI10)
        with self.record_writes():
            self.rt.apply_amd_oc({"core_clock": [1400], "core_vddc": [800], "mem_clock": [910]}, base=self.base)
        self.assertEqual(self.writes["pp_od_clk_voltage"], ["s 1 1400", "vc 2 1400 800", "m 1 910", "c"])

    def test_rdna2_skips_absolute_voltage_and_pptable_fields(self):
        self.card("0000:03:00.0", "0x1002", RDNA2)
        with self.record_writes():
            result = self.rt.apply_amd_oc({"core_clock": [1300], "core_vddc": [700], "mem_clock": [1075],
                                           "mvdd": [1350], "ref": [30]}, base=self.base)[0]
        self.assertEqual(self.writes["pp_od_clk_voltage"], ["s 1 1300", "m 1 1075", "c"])
        self.assertEqual(result["errors"], [])
        self.assertEqual(len(result["skipped"]), 3)  # voltage, MVDD, REF without amdmemtweak

    def test_power_limit_above_card_maximum_is_an_error_not_a_write(self):
        self.card("0000:03:00.0", "0x1002", RDNA2, cap_max=100)
        with self.record_writes():
            result = self.rt.apply_amd_oc({"pl": [150]}, base=self.base)[0]
        self.assertNotIn("power1_cap", self.writes)
        self.assertIn("100 W", result["errors"][0])

    def test_missing_overdrive_reports_ppfeaturemask(self):
        self.card("0000:03:00.0", "0x1002")
        result = self.rt.apply_amd_oc({"core_clock": [1200]}, base=self.base)[0]
        self.assertIn("ppfeaturemask", result["errors"][0])

    def test_amd_reset_restores_defaults(self):
        self.card("0000:03:00.0", "0x1002", RDNA2)
        with self.record_writes():
            self.rt.apply_amd_oc({}, reset=True, base=self.base)
        self.assertEqual(self.writes["pp_od_clk_voltage"], ["r", "c"])
        self.assertEqual(self.writes["power_dpm_force_performance_level"], ["auto"])
        self.assertEqual(self.writes["pwm1_enable"], ["2"])
        self.assertEqual(self.writes["power1_cap"], ["150000000"])

    def nvml(self, calls, fail=()):
        rt = self.rt

        class Fake(rt.Nvml):
            def __init__(self):
                import ctypes
                self.c = ctypes

            def close(self):
                pass

            def check(self, name, *args):
                if name in fail:
                    raise rt.NvmlError(f"{name}: Not Supported (3)")
                calls.append((name, *[getattr(a, "value", a) for a in args[1:]]))

            def handle(self, address):
                return address

            def name(self, handle):
                return "NVIDIA GeForce RTX 3070 Ti"

            def uint(self, name, handle):
                return {"nvmlDeviceGetNumFans": 2, "nvmlDeviceGetPowerManagementDefaultLimit": 290000,
                        "nvmlDeviceGetPowerManagementLimit": 220000}[name]

            def int(self, name, handle):
                return 0

            def power_constraints(self, handle):
                return 100000, 350000
        return patch.object(self.rt, "Nvml", Fake)

    def test_nvidia_offsets_power_and_fans(self):
        self.card("0000:01:00.0", "0x10de")
        calls = []
        with self.nvml(calls):
            result = self.rt.apply_nvidia_oc({"clock": [-200], "mem": [2400], "plimit": [220], "fan": [65]}, base=self.base)[0]
        self.assertEqual(result["errors"], [])
        self.assertIn(("nvmlDeviceSetPowerManagementLimit", 220000), calls)
        self.assertIn(("nvmlDeviceSetGpcClkVfOffset", -200), calls)
        self.assertIn(("nvmlDeviceSetMemClkVfOffset", 2400), calls)  # HiveOS value is already NVML's scale
        self.assertIn(("nvmlDeviceSetFanSpeed_v2", 0, 65), calls)
        self.assertIn(("nvmlDeviceSetFanSpeed_v2", 1, 65), calls)

    def test_nvidia_clock_above_500_locks_the_core(self):
        self.card("0000:01:00.0", "0x10de")
        calls = []
        with self.nvml(calls):
            self.rt.apply_nvidia_oc({"clock": [1500], "fan": [0]}, base=self.base)
        self.assertIn(("nvmlDeviceSetGpuLockedClocks", 1500, 1500), calls)
        self.assertIn(("nvmlDeviceSetGpcClkVfOffset", 0), calls)
        self.assertIn(("nvmlDeviceSetDefaultFanSpeed_v2", 0), calls)

    def test_nvidia_out_of_range_power_and_unsupported_calls_are_reported_per_step(self):
        self.card("0000:01:00.0", "0x10de")
        calls = []
        with self.nvml(calls, fail=("nvmlDeviceSetMemClkVfOffset",)):
            result = self.rt.apply_nvidia_oc({"plimit": [500], "clock": [100], "mem": [1000]}, base=self.base)[0]
        self.assertEqual(len(result["errors"]), 2)
        self.assertIn("核心偏移 +100", result["applied"])
        self.assertFalse(any(c[0] == "nvmlDeviceSetPowerManagementLimit" for c in calls))

    def test_nvidia_reset_restores_default_limit_offsets_and_auto_fan(self):
        self.card("0000:01:00.0", "0x10de")
        calls = []
        with self.nvml(calls):
            self.rt.apply_nvidia_oc({}, reset=True, base=self.base)
        self.assertIn(("nvmlDeviceSetPowerManagementLimit", 290000), calls)
        self.assertIn(("nvmlDeviceSetGpcClkVfOffset", 0), calls)
        self.assertIn(("nvmlDeviceSetMemClkVfOffset", 0), calls)
        self.assertIn(("nvmlDeviceSetDefaultFanSpeed_v2", 1), calls)

    def test_operation_saves_profile_for_boot_and_reset_forgets_it(self):
        cards = [{"vendor": "NVIDIA", "index": 0, "applied": ["x"], "errors": []}]
        with patch.object(self.rt, "gpu_oc", return_value=cards):
            self.rt.oc_apply_operation({"kind": "gpu_oc", "profile_name": "eth", "config": {"nvidia": {"clock": [1500]}}})
            self.assertEqual(self.rt.oc_summary()["profile_name"], "eth")
            self.rt.oc_reset_operation()
        self.assertIsNone(self.rt.oc_summary())

    def test_operation_fails_when_every_card_fails_or_there_are_none(self):
        with patch.object(self.rt, "gpu_oc", return_value=[]):
            with self.assertRaises(RuntimeError):
                self.rt.oc_apply_operation({"config": {"nvidia": {"clock": [1]}}})
        with patch.object(self.rt, "gpu_oc", return_value=[{"vendor": "AMD", "errors": ["x"], "applied": []}]):
            with self.assertRaises(RuntimeError):
                self.rt.oc_apply_operation({"config": {"amd": {"fan": [50]}}})

    def test_boot_reapplies_saved_profile(self):
        self.rt.atomic(self.rt.ROOT / self.rt.OC_FILE, {"config": {"amd": {"fan": [60]}}, "profile_name": "p"})
        with patch.object(self.rt, "gpu_oc", return_value=[]) as applied:
            self.rt.oc_reapply()
        applied.assert_called_once_with({"amd": {"fan": [60]}})


    def test_nvidia_readings_come_from_nvml_without_spawning_nvidia_smi(self):
        rt = self.rt
        values = {"nvmlDeviceGetTemperature": 61, "nvmlDeviceGetFanSpeed": 55, "nvmlDeviceGetPowerUsage": 181500,
                  "nvmlDeviceGetPowerManagementLimit": 220000}

        class Fake(rt.Nvml):
            inits = 0

            def __init__(self):
                import ctypes
                self.c = ctypes
                Fake.inits += 1

            def handle(self, address):
                if address != "0000:01:00.0":
                    raise rt.NvmlError("not found")
                return address

            def name(self, handle):
                return "NVIDIA GeForce RTX 3070 Ti"

            def check(self, name, *args):
                target = args[-1]._obj
                if name == "nvmlDeviceGetUtilizationRates":
                    target.gpu = 99
                elif name == "nvmlDeviceGetMemoryInfo":
                    target.total = 8192 * 1048576
                elif name == "nvmlDeviceGetClockInfo":
                    target.value = {0: 1500, 2: 9751}[args[1].value]
                elif name in values:
                    target.value = values[name]
                else:
                    raise rt.NvmlError(name)

        with patch.object(rt, "Nvml", Fake), patch.object(rt.subprocess, "run") as spawned:
            readings = rt._nvidia_readings(["0000:01:00.0", "0000:02:00.0"])
            rt._nvidia_readings(["0000:01:00.0"])  # same tick: cached
            rt._NVIDIA_CACHE["at"] = -1e9
            rt._nvidia_readings(["0000:01:00.0"])  # next tick: NVML stays open
        spawned.assert_not_called()
        self.assertEqual(Fake.inits, 1)
        self.assertEqual(readings, {"0000:01:00.0": {
            "model": "NVIDIA GeForce RTX 3070 Ti", "temperature_c": 61.0, "fan_pct": 55.0, "power_w": 181.5,
            "util_pct": 99.0, "core_mhz": 1500.0, "mem_mhz": 9751.0, "vram_mb": 8192.0, "power_limit_w": 220.0}})

    def test_nvidia_readings_fall_back_to_nvidia_smi_without_nvml(self):
        rt = self.rt

        def broken():
            raise rt.NvmlError("no library")
        output = "00000000:01:00.0, 60, NVIDIA GeForce RTX 3070 Ti, 50, 180.2, 98, 1500, 9751, 8192, 220.00\n"
        with patch.object(rt, "Nvml", side_effect=broken), \
                patch.object(rt.shutil, "which", return_value="/usr/bin/nvidia-smi"), \
                patch.object(rt.subprocess, "run", return_value=type("r", (), {"stdout": output})()):
            readings = rt._nvidia_readings(["0000:01:00.0"])
        self.assertEqual(readings["0000:01:00.0"]["power_limit_w"], 220.0)
        self.assertTrue(rt._NVML_READER["failed"])

if __name__ == "__main__":
    unittest.main()
