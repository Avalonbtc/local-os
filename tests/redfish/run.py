"""Local TLS Redfish fixture and provider contract tests. No real BMC is contacted."""
import base64
import http.server
import json
import os
from pathlib import Path
import ssl
import subprocess
import threading

root = Path(__file__).resolve().parent
posts = []
resources = {
    "/redfish/v1/": {"Systems": {"@odata.id": "/systems"}, "Chassis": {"@odata.id": "/chassis"}},
    "/systems": {"Members": [{"@odata.id": "/system/1"}]},
    "/system/1": {"PowerState": "On", "Model": "TEST-BMC", "Status": {"Health": "OK"},
                  "Actions": {"#ComputerSystem.Reset": {"target": "/reset", "ResetType@Redfish.AllowableValues": ["On", "GracefulShutdown"]}},
                  "LogServices": {"@odata.id": "/logs"}},
    "/chassis": {"Members": [{"@odata.id": "/chassis/1"}]},
    "/chassis/1": {"Sensors": {"@odata.id": "/sensors"}},
    "/sensors": {"Members": [{"@odata.id": "/sensor/1"}], "Members@odata.nextLink": "/sensors?page=2"},
    "/sensors?page=2": {"Members": [{"@odata.id": "/sensor/2"}]},
    "/sensor/1": {"Name": "CPU1 Temp", "Reading": 55, "ReadingUnits": "Cel", "Status": {"Health": "OK"}},
    "/sensor/2": {"Name": "Missing sensor", "Reading": None, "ReadingUnits": "W"},
    "/logs": {"Members": [{"@odata.id": "/log/1"}]},
    "/log/1": {"Entries": {"@odata.id": "/entries"}},
    "/entries": {"Members": [{"@odata.id": "/event/1"}]},
    "/event/1": {"Id": "1", "Message": "Test-only event", "Severity": "OK"},
}

class Handler(http.server.BaseHTTPRequestHandler):
    def authorized(self):
        return self.headers.get("Authorization") == "Basic " + base64.b64encode(b"test:test-only").decode()

    def do_GET(self):
        if not self.authorized():
            self.send_error(401); return
        if self.path not in resources:
            self.send_error(404); return
        body = json.dumps(resources[self.path]).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if not self.authorized():
            self.send_error(401); return
        posts.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
        self.send_response(202); self.send_header("Location", "/tasks/1"); self.send_header("Content-Length", "0"); self.end_headers()

    def log_message(self, *_):
        pass

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
tls.load_cert_chain(root / "server-cert.pem", root / "server-key.pem")
server.socket = tls.wrap_socket(server.socket, server_side=True)
threading.Thread(target=server.serve_forever, daemon=True).start()
env = {**os.environ, "TEST_REDFISH_URL": f"https://127.0.0.1:{server.server_port}", "TEST_REDFISH_CA": str(root / "test-ca.pem")}
try:
    subprocess.run(["cargo", "test", "-p", "rig-infrastructure", "--test", "redfish", "--", "--ignored"], cwd=root.parents[1], env=env, check=True)
    assert posts == [{"ResetType": "GracefulShutdown"}], posts
    print("TLS, auth, pagination, missing sensor, events and graceful-only reset verified.")
finally:
    server.shutdown()
