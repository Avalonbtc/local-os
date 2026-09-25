"""Exercise bootstrap's actual shell script without network or real apt writes."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'runtime/dependencies.sh'

class DependenciesTest(unittest.TestCase):
    def run_fixture(self, missing=False, fail=False, hang=False):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            def executable(name, body):
                file = root / name
                file.write_text('#!/bin/sh\n' + body + '\n')
                file.chmod(0o755)
            for binary in ['screen', 'python3', 'bash', 'unshare', 'mount', 'jq', 'curl']:
                if binary != 'jq' or not missing:
                    executable(binary, 'exit 0')
            executable('timeout', 'shift; shift; shift; exec "$@"')
            executable('env', 'shift; exec "$@"')
            if hang:
                executable('timeout', 'exec /usr/bin/timeout -k 0.1s 0.1s /bin/sh -c \'trap "" TERM; /bin/sleep 10\'')
            executable('apt-get', 'echo "$*" >> "$FIXTURE/apt.log"\n' +
                ('exit 42' if fail else
                 'if [ "$1" = install ]; then /bin/cp "$FIXTURE/screen" "$FIXTURE/jq"; fi'))
            result = subprocess.run(['/bin/sh', str(SCRIPT)], text=True, capture_output=True,
                env={**os.environ, 'PATH': tmp, 'FIXTURE': tmp})
            log = (root / 'apt.log').read_text() if (root / 'apt.log').exists() else ''
            return result, log

    def test_complete_host_never_calls_apt(self):
        result, log = self.run_fixture()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(log, '')

    def test_screen_present_but_jq_missing_installs_jq(self):
        result, log = self.run_fixture(missing=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('update ', log)
        self.assertIn(' jq\n', log)
        self.assertNotIn(' screen', log)

    def test_apt_failure_stops_install(self):
        result, log = self.run_fixture(missing=True, fail=True)
        self.assertEqual(result.returncode, 42)
        self.assertNotIn('install ', log)

    def test_hung_installer_is_killed_after_grace_period(self):
        started = time.monotonic()
        result, _ = self.run_fixture(missing=True, hang=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertLess(time.monotonic() - started, 3)

if __name__ == '__main__':
    unittest.main()
