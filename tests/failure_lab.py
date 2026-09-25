"""Fault injection restricted to the explicitly named disposable local lab."""
import argparse
import http.cookiejar
import json
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument("--work", type=Path, required=True)
args = parser.parse_args()
node = "rigdeck-lab-node1-1"
database = "rigdeck-dev-pg"
def docker(*args, **kwargs):
    return subprocess.check_output(["docker", *args], **kwargs)
node_info = json.loads(docker("inspect", node))[0]
db_info = json.loads(docker("inspect", database))[0]
assert node_info["Config"]["Labels"]["com.docker.compose.project"] == "rigdeck-lab"
assert db_info["Config"]["Image"].startswith("postgres:18")
assert db_info["HostConfig"]["PortBindings"]["5432/tcp"] == [{"HostIp":"127.0.0.1","HostPort":"55432"}]
def runtime(*args):
    return json.loads(docker("exec", node, "rig-miner", *args))
runtime("start", "lab-cpu")
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
request = urllib.request.Request("http://127.0.0.1:8080/api/v1/login", data=b'{"username":"admin","password":"RigDeck-local-test-2026"}', headers={"Content-Type":"application/json"})
with opener.open(request) as response: session = json.load(response)
try:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        state = runtime("status", "lab-cpu")
        if state.get("child_pid"): break
        time.sleep(.2)
    previous_pid = state["child_pid"]
    docker("stop", "--time", "2", database)
    request = urllib.request.Request("http://127.0.0.1:8080/api/v1/catalog/coins", data=b'{"name":"must-not-save","data":{"symbol":"DB-OFFLINE"}}', headers={"Content-Type":"application/json","X-CSRF-Token":session["csrf"]})
    try:
        opener.open(request, timeout=15)
        raise AssertionError("Write accepted while PostgreSQL stopped")
    except urllib.error.HTTPError as error:
        assert error.code == 503, error.code
    # Kill only the fixture miner identified by this runtime record.
    docker("exec", node, "kill", "-KILL", str(previous_pid))
    deadline = time.monotonic() + 25
    recovered = False
    while time.monotonic() < deadline:
        samples = runtime("collect", "mining")["instances"]
        recovered = any(s.get("instance") == "lab-cpu" and s.get("process_alive") and s.get("stats", {}) and time.time() - s.get("stats_observed_at", 0) < 15 for s in samples)
        state = runtime("status", "lab-cpu")
        if recovered and state.get("child_pid") not in (None, previous_pid): break
        time.sleep(.5)
    assert recovered and state["child_pid"] != previous_pid
finally:
    docker("start", database)
    runtime("stop", "lab-cpu")
deadline = time.monotonic() + 30
while time.monotonic() < deadline:
    try:
        with opener.open("http://127.0.0.1:8080/api/v1/machines", timeout=8) as response:
            assert isinstance(json.load(response), list)
        break
    except (urllib.error.URLError, TimeoutError): time.sleep(1)
else: raise AssertionError("Controller did not reconnect to PostgreSQL")
report = {"postgres_stop_rejects_write":True,"local_crash_recovery_without_database":True,"local_statistics_without_database":True,"controller_reconnected":True,"synthetic_miner":True,"completed_at":time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())}
(args.work.resolve() / "failure-report.json").write_text(json.dumps(report, indent=2))
print(json.dumps(report, indent=2))
