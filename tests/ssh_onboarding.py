"""Disposable Linux SSH/PG integration check; --keep permits browser validation."""
import argparse
import base64
import http.cookiejar
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

p = argparse.ArgumentParser()
p.add_argument('--work', type=Path, required=True)
p.add_argument('--keep', action='store_true')
args = p.parse_args()
root = Path(__file__).resolve().parents[1]
work = args.work.resolve()
work.mkdir(parents=True, exist_ok=True)
prefix = 'rigdeck-onboarding'
api_image = os.environ.get('RIGDECK_TEST_API_IMAGE', 'rigdeck-build:ssh')
names = [prefix + '-api', prefix + '-node', prefix + '-db']
created = []
network_created = False
env_file = work / 'test.env'

def docker(*cmd):
    return subprocess.check_output(['docker', *cmd], text=True, encoding='utf-8').strip()

def cleanup():
    for name in reversed(created):
        subprocess.run(['docker', 'rm', '-f', '-v', name], check=False, stdout=subprocess.DEVNULL)
    if network_created:
        subprocess.run(['docker', 'network', 'rm', prefix], check=False, stdout=subprocess.DEVNULL)
    env_file.unlink(missing_ok=True)

try:
    docker('network', 'create', prefix)
    network_created = True
    key = work / 'test_encrypted_key'
    if not key.exists():
        subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', 'fixture-passphrase-only', '-f', str(key)], check=True)
    docker('run', '-d', '--name', names[2], '--network', prefix,
           '-e', 'POSTGRES_USER=rigdeck', '-e', 'POSTGRES_DB=rigdeck',
           '-e', 'POSTGRES_PASSWORD=fixture-db-only', 'postgres:18')
    created.append(names[2])
    for _ in range(60):
        ready = subprocess.run(['docker', 'exec', names[2], 'pg_isready', '-U', 'rigdeck'], capture_output=True)
        if ready.returncode == 0:
            break
        time.sleep(.5)
    else:
        raise RuntimeError('fixture database did not start')
    docker('run', '-d', '--name', names[1], '--network', prefix, '--network-alias', 'node',
           '-v', f'{key}.pub:/run/lab-key.pub:ro', 'rigdeck-test-node:local')
    created.append(names[1])
    env_file.write_text('\n'.join([
        'DATABASE_URL=postgres://rigdeck:fixture-db-only@' + names[2] + ':5432/rigdeck',
        'RIGDECK_MASTER_KEY=' + base64.b64encode(os.urandom(32)).decode(),
        'RIGDECK_ORIGIN=http://localhost:18082', 'RIGDECK_BIND=0.0.0.0:8080',
        'RIGDECK_STATIC_DIR=/web', 'RIGDECK_ADMIN_PASSWORD=RigDeck-fixture-2026',
    ]) + '\n')
    docker('run', '--rm', '--network', prefix, '--env-file', str(env_file),
           api_image, '/src/target/release/rigdeck', 'create-admin', 'admin')
    docker('run', '-d', '--name', names[0], '--network', prefix, '--env-file', str(env_file),
           '-p', '127.0.0.1:18082:8080', '-v', f'{root / "frontend/dist"}:/web:ro',
           api_image, '/src/target/release/rigdeck', 'serve')
    created.append(names[0])
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
    csrf = ''
    cookie = ''
    def api(method, path, body=None, expected=200, csrf_header=True):
        global cookie
        request = urllib.request.Request('http://localhost:18082/api/v1' + path,
            data=json.dumps(body).encode() if body is not None else None, method=method,
            headers={'Content-Type': 'application/json', 'Origin': 'http://localhost:18082',
                     **({'Cookie': cookie} if cookie else {}),
                     **({'X-CSRF-Token': csrf} if csrf_header else {})})
        try:
            response = opener.open(request, timeout=25)
        except urllib.error.HTTPError as e:
            response = e
        assert response.status == expected, (path, response.status, expected, response.read().decode())
        # Python's CookieJar lacks browsers' Secure-cookie localhost exception.
        # This fixture targets a fixed loopback URL; never forward to another host.
        if response.headers.get('Set-Cookie'):
            cookie = response.headers['Set-Cookie'].split(';', 1)[0]
        payload = response.read()
        return json.loads(payload) if payload else None
    for _ in range(40):
        try:
            api('POST', '/ssh/host-key', {'host': 'node', 'port': 22}, expected=401)
            break
        except urllib.error.URLError:
            time.sleep(.25)
    csrf = api('POST', '/login', {'username': 'admin', 'password': 'RigDeck-fixture-2026'})['csrf']
    api('POST', '/ssh/host-key', {'host': 'node', 'port': 22}, expected=403, csrf_header=False)
    api('POST', '/ssh/host-key', {'host': 'bad host', 'port': 22}, expected=400)
    observed = api('POST', '/ssh/host-key', {'host': 'node', 'port': 22})
    expected = docker('exec', names[1], 'ssh-keygen', '-lf', '/etc/ssh/ssh_host_ed25519_key.pub').split()[1]
    assert observed['fingerprint'] == expected
    assert observed['algorithm'] == 'ssh-ed25519'
    record = {'name': 'API encrypted-key fixture', 'host': 'node', 'port': 22, 'username': 'root',
        'host_key': expected, 'group': 'isolated-test', 'tags': [], 'is_controller': False, 'policy': {},
        'credential': {'kind': 'private_key', 'private_key': key.read_text(), 'passphrase': 'fixture-passphrase-only'}}
    machine = api('POST', '/machines', record)
    assert 'PRIVATE KEY' not in json.dumps(machine)
    assert api('POST', '/machines/' + machine['id'] + '/test')['exit_code'] == 0
    record['host_key'] = 'SHA256:' + 'A' * 43
    api('PUT', '/machines/' + machine['id'], record)
    api('POST', '/machines/' + machine['id'] + '/test', expected=503)
    record['host_key'] = expected
    api('PUT', '/machines/' + machine['id'], record)
    assert api('POST', '/machines/' + machine['id'] + '/test')['exit_code'] == 0
    # A disposable non-root account exercises key login plus password-protected sudo.
    docker('exec', names[1], 'useradd', '-m', '-s', '/bin/bash', '-G', 'sudo', 'rigtest')
    subprocess.run(['docker', 'exec', '-i', names[1], 'chpasswd'],
                   input=b'rigtest:fixture-sudo-only\n', check=True)
    docker('exec', names[1], 'sh', '-c',
           "printf '#!/bin/sh\\nexit 0\\n' > /usr/local/bin/systemctl && chmod 755 /usr/local/bin/systemctl")
    elevated = {**record, 'name': 'Non-root sudo fixture', 'username': 'rigtest',
        'sudo_password': 'fixture-sudo-only'}
    elevated_machine = api('POST', '/machines', elevated)
    assert 'fixture-sudo-only' not in json.dumps(elevated_machine)
    sudo_test = api('POST', '/machines/' + elevated_machine['id'] + '/test')
    assert sudo_test['exit_code'] == 0, sudo_test
    elevated['sudo_password'] = 'incorrect-fixture-only'
    api('PUT', '/machines/' + elevated_machine['id'], elevated)
    assert api('POST', '/machines/' + elevated_machine['id'] + '/test')['exit_code'] != 0
    elevated['sudo_password'] = 'fixture-sudo-only'
    api('PUT', '/machines/' + elevated_machine['id'], elevated)
    # The older SSH fixture has a real directory at current; a fresh install has no current link.
    docker('exec', names[1], 'mv', '/opt/rigdeck/current', '/opt/rigdeck/fixture-runtime')
    # Existing screen must not hide a missing jq dependency; fake apt restores it offline.
    docker('exec', names[1], 'mv', '/usr/bin/jq', '/usr/bin/jq.fixture')
    docker('exec', names[1], 'sh', '-c',
           "printf '#!/bin/sh\\nif [ \"$1\" = install ]; then /bin/mv /usr/bin/jq.fixture /usr/bin/jq; fi\\n' > /usr/local/bin/apt-get && chmod 755 /usr/local/bin/apt-get")
    job = api('POST', '/jobs', {'machine_ids': [elevated_machine['id']], 'action': {'kind': 'bootstrap'},
        'idempotency_key': 'sudo-bootstrap-fixture-v1', 'concurrency': 1, 'canary': True})
    for _ in range(60):
        target = api('GET', '/jobs/' + job['job_id'])['targets'][0]
        if target['status'] in ('succeeded', 'failed', 'unknown'):
            break
        time.sleep(.5)
    assert target['status'] == 'succeeded', target
    print(json.dumps({'probe_matches_console': True, 'auth_and_csrf': True,
        'encrypted_key_login': True, 'changed_host_rejected': True, 'restored_host_login': True,
        'nonroot_sudo_password': True, 'bootstrap_with_sudo': True,
        'browser_url': 'http://localhost:18082', 'fixture_private_key': str(key)}, indent=2))
except BaseException:
    cleanup()
    raise
else:
    if not args.keep:
        cleanup()
