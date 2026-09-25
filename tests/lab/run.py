"""Provision eight disposable SSH nodes and test the real API/SSH/PG/runtime path.

The HTTP miner fixture reports synthetic statistics. It is a protocol/lifecycle test,
never a real hashrate, pool-share or EPYC capacity benchmark.
"""
import argparse
import base64
import hashlib
import http.cookiejar
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import time
import urllib.request
import uuid

parser=argparse.ArgumentParser()
parser.add_argument("--work",type=Path,required=True)
parser.add_argument("--build-only",action="store_true")
args=parser.parse_args()
root=Path(__file__).resolve().parents[2]
work=args.work.resolve();work.mkdir(parents=True,exist_ok=True)
key=work/"lab_ed25519"
if not key.exists():
    subprocess.run(["ssh-keygen","-q","-t","ed25519","-N","","-f",str(key)],check=True)
env=os.environ.copy();env["RIGDECK_LAB_PUBLIC_KEY"]=str(key.with_suffix(".pub"))
compose=["docker","compose","-p","rigdeck-lab","-f",str(root/"tests/lab/compose.yml")]
subprocess.run(compose+["up","-d","--build"],env=env,check=True)
if args.build_only:
    raise SystemExit(0)
opener=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
csrf=""
def api(method,path,payload=None):
    body=json.dumps(payload).encode() if payload is not None else None
    request=urllib.request.Request("http://127.0.0.1:8080/api/v1"+path,data=body,method=method,headers={"Content-Type":"application/json","X-CSRF-Token":csrf})
    with opener.open(request,timeout=45) as response:
        data=response.read()
        return json.loads(data) if data else None
session=api("POST","/login",{"username":"admin","password":os.environ.get("RIGDECK_TEST_PASSWORD","RigDeck-local-test-2026")});csrf=session["csrf"]
existing={m["name"]:m for m in api("GET","/machines")}
hosts=[]
for n in range(1,9):
    container=subprocess.check_output(compose+["ps","-q",f"node{n}"],env=env,text=True).strip()
    fingerprint=subprocess.check_output(["docker","exec",container,"ssh-keygen","-lf","/etc/ssh/ssh_host_ed25519_key.pub"],text=True).split()[1]
    record={"name":f"LAB-ubuntu-{n:02}","host":"127.0.0.1","port":22220+n,"username":"root","host_key":fingerprint,
            "group":"隔离验收环境","tags":["lab","synthetic-miner"],"credential":{"kind":"private_key","private_key":key.read_text(),"passphrase":None},
            "policy":{"max_restarts":3,"cooldown_seconds":2,"failure_seconds":30,"allow_host_reboot":False}}
    old=existing.get(record["name"])
    host=api("PUT" if old else "POST","/machines"+("/"+old["id"] if old else ""),record)
    hosts.append(host["id"])
    output=api("POST",f"/machines/{host['id']}/test")
    assert output["exit_code"]==0,output

def job(action,concurrency=8,canary=False):
    payload={"action":action,"machine_ids":hosts,"idempotency_key":str(uuid.uuid4()),"concurrency":concurrency,"canary":canary,"include_controller":False}
    first=api("POST","/jobs",payload)
    assert api("POST","/jobs",payload)==first,"idempotency failed"
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        result=api("GET",f"/jobs/{first['job_id']}")
        if result["status"] not in ("queued","running"):
            assert result["status"]=="succeeded",json.dumps(result,ensure_ascii=False)
            return result
        time.sleep(.5)
    raise AssertionError("job deadline exceeded")

commands=job({"kind":"command","script":"printf 'LAB-ONLY '; uname -s; sleep 2; printf 'done\\n'","timeout_seconds":15})
assert len(commands["targets"])==8
assert all("LAB-ONLY Linux" in t["output"] and "done" in t["output"] for t in commands["targets"])
fixture=b'''#!/usr/bin/env python3
import http.server,json,sys,time
port=int(next(v.split('=',1)[1] for v in sys.argv if v.startswith('--http-port=')))
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  body=json.dumps({'version':'test-1','algo':'rx/0','uptime':10,'hashrate':{'total':[12500]},'results':{'shares_good':1,'shares_total':1},'connection':{'pool':'LOCAL-TEST-FIXTURE','uptime':10}}).encode();self.send_response(200);self.end_headers();self.wfile.write(body)
 def log_message(self,*args):pass
http.server.HTTPServer(('127.0.0.1',port),Handler).serve_forever()
'''
buffer=io.BytesIO()
with tarfile.open(fileobj=buffer,mode="w:gz") as archive:
    entry=tarfile.TarInfo("lab-miner.py");entry.size=len(fixture);entry.mode=0o755;archive.addfile(entry,io.BytesIO(fixture))
package=buffer.getvalue()
# Rigs download miners themselves (like HiveOS): serve the fixture over HTTP on the lab
# network's gateway. Plain HTTP is accepted only because the SHA256 is pinned.
import http.server,threading
class Package(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200);self.send_header("Content-Length",str(len(package)));self.end_headers();self.wfile.write(package)
    def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(("0.0.0.0",0),Package)
threading.Thread(target=server.serve_forever,daemon=True).start()
gateway=os.environ.get("RIGDECK_LAB_PACKAGE_HOST") or subprocess.run(["docker","network","inspect","rigdeck-lab_default","-f","{{(index .IPAM.Config 0).Gateway}}"],capture_output=True,text=True,check=True).stdout.strip()
suffix=str(uuid.uuid4())[:8]
wallet=api("POST","/catalog/wallets",{"name":f"LAB wallet {suffix}","data":{"coin_symbol":f"LAB-{suffix}","address":"local-test-no-funds"}})
miner={"adapter":"xmrig","name":"LAB synthetic miner","version":"test-1","executable":"lab-miner.py","url":f"http://{gateway}:{server.server_address[1]}/lab-miner.tar.gz","sha256":hashlib.sha256(package).hexdigest()}
sheet=api("POST","/flight-sheets",{"name":f"LAB flight {suffix}","tasks":[{"instance":"lab-cpu","wallet_id":wallet["id"],"miner":miner,"config":{"urls":["stratum+tcp://127.0.0.1:1"],"algorithm":"rx/0","warmup_seconds":1,"verification_seconds":15}}]})
deployment=job({"kind":"apply","sheet_id":sheet["id"]},concurrency=4,canary=True)
stopped=job({"kind":"miner","operation":"stop","instances":["lab-cpu"]})
report={"nodes":8,"ssh_connection_tests":8,"batch_command_job":commands["id"],"flight_sheet_job":deployment["id"],"stop_job":stopped["id"],"stats_are_synthetic":True,"real_mining_verified":False,"completed_at":time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())}
(work/"lab-report.json").write_text(json.dumps(report,indent=2))
print(json.dumps(report,indent=2))
