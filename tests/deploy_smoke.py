"""Validate the built production image, HTTPS, PG18 and isolated dump restore."""
import argparse
import http.cookiejar
import json
import os
from pathlib import Path
import shutil
import ssl
import subprocess
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument("--work", type=Path, required=True)
args = parser.parse_args()
source = Path(__file__).resolve().parents[1]
stage = args.work.resolve() / "deploy-smoke"
stage.mkdir(parents=True, exist_ok=True)
for name in ("compose.yml", "deploy/Caddyfile", "scripts/init.py"):
    target = stage / name; target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source / name, target)
if not (stage / ".env").exists():
    subprocess.run(["python", str(stage / "scripts/init.py"), "--host", "localhost", "--listen", "127.0.0.1"], check=True)
(stage / "smoke.yml").write_text('''name: rigdeck-smoke
services:
  app:
    image: rigdeck:local
    environment:
      RIGDECK_ORIGIN: https://localhost:18443
  https:
    ports: !override ["127.0.0.1:18443:443", "127.0.0.1:18080:80"]
''')
compose = ["docker", "compose", "-p", "rigdeck-smoke", "-f", str(stage / "compose.yml"), "-f", str(stage / "smoke.yml")]
def run(*args, **kwargs):
    return subprocess.run(compose + list(args), check=True, **kwargs)
run("up", "-d", "--no-build", "--wait", "--wait-timeout", "120")
admin_env = {**os.environ, "RIGDECK_ADMIN_PASSWORD": "RigDeck-smoke-test-2026"}
admin = subprocess.run(compose + ["run", "--rm", "-e", "RIGDECK_ADMIN_PASSWORD", "app", "create-admin", "smoke-admin"], env=admin_env, capture_output=True, text=True, encoding="utf-8")
if admin.returncode and "已存在" not in admin.stderr:
    raise RuntimeError(admin.stderr)
ca = stage / "root.crt"
for attempt in range(20):
    copied = subprocess.run(compose + ["cp", "https:/data/caddy/pki/authorities/local/root.crt", str(ca)], capture_output=True)
    if copied.returncode == 0: break
    time.sleep(1)
tls = ssl.create_default_context(cafile=str(ca))
opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=tls), urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
base = "https://localhost:18443"
with opener.open(base + "/", timeout=15) as response:
    assert b"RigDeck" in response.read()
request = urllib.request.Request(base + "/api/v1/login", data=json.dumps({"username":"smoke-admin","password":admin_env["RIGDECK_ADMIN_PASSWORD"]}).encode(), headers={"Content-Type":"application/json","Origin":base})
with opener.open(request, timeout=15) as response:
    assert "Secure" in response.headers["Set-Cookie"]
    session = json.load(response)
request = urllib.request.Request(base + "/api/v1/catalog/wallets", data=json.dumps({"name":"restore-test","data":{"coin_symbol":"SMOKE","address":"test-only"}}).encode(), headers={"Content-Type":"application/json","X-CSRF-Token":session["csrf"],"Origin":base})
with opener.open(request) as response: assert json.load(response)["name"] == "restore-test"
dump = stage / "database.dump"
with dump.open("wb") as output:
    run("exec", "-T", "postgres", "pg_dump", "-U", "rigdeck", "-d", "rigdeck", "-Fc", stdout=output)
database = "rigdeck_restore_smoke"
run("exec", "-T", "postgres", "createdb", "-U", "rigdeck", database)
try:
    with dump.open("rb") as input_file:
        run("exec", "-T", "postgres", "pg_restore", "-U", "rigdeck", "--exit-on-error", "--no-owner", "-d", database, stdin=input_file)
    result = run("exec", "-T", "postgres", "psql", "-U", "rigdeck", "-d", database, "-Atc", "SELECT count(*) FROM wallets WHERE name='restore-test'", capture_output=True, text=True)
    assert int(result.stdout.strip()) > 0
finally:
    run("exec", "-T", "postgres", "dropdb", "-U", "rigdeck", database)
report = {"production_image":"rigdeck:local","https_certificate_verified":True,"secure_cookie":True,"wallet_write":True,"postgres18_dump_restore":True,"completed_at":time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())}
(args.work.resolve() / "deploy-report.json").write_text(json.dumps(report, indent=2))
print(json.dumps(report, indent=2))
