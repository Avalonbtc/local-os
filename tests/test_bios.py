import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
import uuid

spec = importlib.util.spec_from_file_location('bios', Path(__file__).parents[1] / 'runtime/bios-sum.py')
bios = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bios)
XML = '''<BiosCfg><Menu name="Advanced"><Menu name="ACPI Settings">
<Setting name="L3 NUMA" type="Option" selectedOption="Auto"><Information><AvailableOptions><Option value="0">Disabled</Option><Option value="255">Auto</Option></AvailableOptions></Information></Setting>
<Setting name="cTDP" type="Numeric" numericValue="150"><Information><MinValue>100</MinValue><MaxValue>200</MaxValue><StepSize>5</StepSize></Information></Setting>
<Setting name="Password" type="Option" selectedOption="secret"/>
</Menu></Menu></BiosCfg>'''

class BiosTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / 'sum'
        self.binary.touch()
        self.request = dict(root=str(self.root / 'data'), machine_id=str(uuid.uuid4()), operation_id=str(uuid.uuid4()),
            binary=str(self.binary), host='10.0.0.2', username='ADMIN', password='private-fixture', action={'kind':'bios_read'}, reconcile=False)
        self.calls = []
        self.xml = XML
        self.fail_write = False
        if sys.platform == 'win32':
            mock = types.SimpleNamespace(flock=lambda *a: None, LOCK_EX=1, LOCK_NB=2)
            self.mock_fcntl = patch.dict(sys.modules, fcntl=mock)
            self.mock_fcntl.start()
            self.addCleanup(self.mock_fcntl.stop)
        self.runner = patch.object(bios.subprocess, 'run', side_effect=self.run_sum)
        self.runner.start()
        self.addCleanup(self.runner.stop)
    def run_sum(self, args, **kwargs):
        self.assertNotIn('--reboot', args)
        self.assertNotIn('private-fixture', args)
        cmd = args[args.index('-c') + 1]
        self.calls.append(cmd)
        if cmd == 'GetCurrentBiosCfg': Path(args[-1]).write_text(self.xml)
        if cmd == 'ChangeBiosCfg':
            root = bios.ET.parse(args[-1]).getroot()
            self.assertEqual(len(list(root.iter('Setting'))), 1)
            if self.fail_write: raise subprocess.TimeoutExpired(args, 120)
        return subprocess.CompletedProcess(args, 0, b'SFT-OOB-LIC' if cmd == 'QueryProductKey' else b'OK', b'')
    def snapshot(self):
        return json.loads((Path(self.request['root']) / self.request['machine_id'] / 'snapshot.json').read_text())
    def next_operation(self, action):
        self.request.update(operation_id=str(uuid.uuid4()), action=action)
        return bios.main(self.request)
    def setup_write(self):
        self.assertEqual(bios.main(self.request)['status'], 'succeeded')
        snap = self.snapshot()
        return dict(kind='bios_write', revision=snap['revision'], changes={snap['settings'][0]['id']:'Disabled'})
    def test_minimal_write_and_reboot_pending(self):
        action = self.setup_write()
        self.assertEqual(self.next_operation(action)['status'], 'succeeded')
        self.assertEqual(self.snapshot()['pending'], action['changes'])
        self.assertEqual(self.snapshot()['settings'][0]['value'], 'Auto')
        self.assertEqual(len(self.snapshot()['settings']), 2)
        op = Path(self.request['root']) / self.request['machine_id'] / self.request['operation_id']
        self.assertTrue((op/'before.xml').exists())
        self.assertEqual(next(bios.ET.parse(op/'rollback.xml').iter('Setting')).get('selectedOption'), 'Auto')
    def test_same_operation_never_replays(self):
        action = self.setup_write()
        self.next_operation(action)
        before = self.calls[:]
        self.assertEqual(bios.main(self.request)['status'], 'succeeded')
        self.assertEqual(before, self.calls)
    def test_stale_revision_cannot_write(self):
        action = self.setup_write()
        self.xml = XML.replace('numericValue="150"', 'numericValue="160"')
        self.assertEqual(self.next_operation(action)['status'], 'failed')
        self.assertNotIn('ChangeBiosCfg', self.calls)
    def test_timeout_unknown_no_replay(self):
        action = self.setup_write(); self.fail_write = True
        self.assertEqual(self.next_operation(action)['status'], 'unknown')
        self.assertTrue(self.snapshot()['pending'])
        before = self.calls[:]
        self.request['reconcile'] = True
        self.assertEqual(bios.main(self.request)['status'], 'unknown')
        self.assertEqual(before, self.calls)
    def test_reconcile_missing_never_writes(self):
        self.request['reconcile'] = True
        self.assertEqual(bios.main(self.request)['status'], 'unknown')
        self.assertEqual(self.calls, [])
    def test_pending_only_cleared_by_matching_read(self):
        action = self.setup_write(); self.next_operation(action)
        self.next_operation({'kind':'bios_read'})
        self.assertTrue(self.snapshot()['pending'])
        self.xml = XML.replace('selectedOption="Auto"','selectedOption="Disabled"')
        self.next_operation({'kind':'bios_read'})
        self.assertFalse(self.snapshot()['pending'])
    def test_invalid_values_and_paths(self):
        self.setup_write(); fields = self.snapshot()['settings']
        for changes in [{fields[0]['id']:'arbitrary'}, {fields[1]['id']:'500'}, {fields[1]['id']:'151'}, {'unknown':'Disabled'}]:
            with self.assertRaises(ValueError): bios.changes_xml(fields, changes)
    def test_interrupted_journal_not_replayed(self):
        directory = Path(self.request['root']) / self.request['machine_id'] / self.request['operation_id']
        directory.mkdir(parents=True)
        bios.atomic(directory/'result.json',dict(status='running'))
        self.assertEqual(bios.main(self.request)['status'], 'unknown')
        self.assertEqual(self.calls, [])
    def test_ambiguous_menu_and_setting_paths_are_excluded(self):
        duplicate = '<Menu name="Serial"><Setting name="Port" type="Numeric" numericValue="1"/></Menu>'
        other = '<Menu name="Serial"><Setting name="Different" type="Numeric" numericValue="2"/></Menu>'
        self.xml = XML.replace('</BiosCfg>', duplicate + other + '</BiosCfg>')
        self.assertEqual(bios.main(self.request)['status'], 'succeeded')
        self.assertEqual([f['name'] for f in self.snapshot()['settings']], ['L3 NUMA', 'cTDP'])

    def test_discard_pending_after_reboot_unblocks_writes(self):
        action = self.setup_write(); self.next_operation(action)
        # Firmware ignored the value after reboot: a plain read keeps pending and writes stay blocked.
        self.next_operation({'kind':'bios_read'})
        self.assertTrue(self.snapshot()['pending'])
        result = self.next_operation({'kind':'bios_read', 'discard_pending': True})
        self.assertEqual(result['status'], 'succeeded')
        self.assertIn('清除', result['output'])
        self.assertFalse(self.snapshot()['pending'])
        retry = dict(kind='bios_write', revision=self.snapshot()['revision'], changes=action['changes'])
        self.assertEqual(self.next_operation(retry)['status'], 'succeeded')
    def test_busy_lock_fails_fresh_but_never_resolves_reconcile(self):
        if sys.platform == 'win32': self.skipTest('flock is mocked on Windows')
        import fcntl
        directory = Path(self.request['root']) / self.request['machine_id']
        directory.mkdir(parents=True)
        with open(directory / 'lock', 'a') as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            self.assertEqual(bios.main(self.request)['status'], 'failed')
            self.request['reconcile'] = True
            self.assertEqual(bios.main(self.request)['status'], 'unknown')
        self.assertEqual(self.calls, [])
    def test_empty_operation_directory_from_crash_is_reused(self):
        directory = Path(self.request['root']) / self.request['machine_id'] / self.request['operation_id']
        directory.mkdir(parents=True)
        self.assertEqual(bios.main(self.request)['status'], 'succeeded')

if __name__ == '__main__': unittest.main()
