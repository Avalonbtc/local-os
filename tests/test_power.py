import tempfile
import unittest
from pathlib import Path
from test_runtime_snapshot import load


class PowerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.rt = load(self.base / 'state', self.base / 'run')
        self.boot = self.base / 'proc/sys/kernel/random/boot_id'
        self.boot.parent.mkdir(parents=True)
        self.boot.write_text('boot-a')

    def tearDown(self):
        self.temp.cleanup()

    def zone(self, id, name, energy, maximum=10000000000):
        p = self.base / 'sys/class/powercap' / id
        p.mkdir(parents=True, exist_ok=True)
        for key, value in [('name', name), ('energy_uj', energy), ('max_energy_range_uj', maximum)]:
            (p / key).write_text(str(value))

    def test_dual_socket_ignores_core_subdomains(self):
        for i in range(2):
            self.zone(f'intel-rapl:{i}', f'package-{i}', 100)
            self.zone(f'intel-rapl:{i}:0', 'core', 100)
        self.assertIsNone(self.rt.collect_cpu_power(self.base, 10)['cpu_power_w'])
        for i in range(2):
            self.zone(f'intel-rapl:{i}', f'package-{i}', 1500000100)
        result = self.rt.collect_cpu_power(self.base, 20)
        self.assertEqual(result['cpu_power_w'], 300)
        self.assertEqual(len(result['cpu_power_packages']), 2)

    def test_counter_wrap_and_reboot_and_stale(self):
        self.zone('intel-rapl:0', 'package-0', 900000000, 1000000000)
        self.rt.collect_cpu_power(self.base, 10)
        self.zone('intel-rapl:0', 'package-0', 100000000, 1000000000)
        self.assertEqual(self.rt.collect_cpu_power(self.base, 12)['cpu_power_w'], 100)
        self.boot.write_text('boot-b')
        self.assertIsNone(self.rt.collect_cpu_power(self.base, 13)['cpu_power_w'])
        self.assertIsNone(self.rt.collect_cpu_power(self.base, 100)['cpu_power_w'])

    def test_unsupported_and_partial_failure_are_not_zero(self):
        self.assertIsNone(self.rt.collect_cpu_power(self.base, 10)['cpu_power_w'])
        self.zone('intel-rapl:0', 'package-0', 100)
        self.zone('intel-rapl:1', 'package-1', 100)
        self.rt.collect_cpu_power(self.base, 20)
        self.zone('intel-rapl:0', 'package-0', 1500000100)
        self.zone('intel-rapl:1', 'package-1', 'invalid')
        self.assertIsNone(self.rt.collect_cpu_power(self.base, 30)['cpu_power_w'])


if __name__ == '__main__':
    unittest.main()
