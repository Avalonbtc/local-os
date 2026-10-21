"""rigdeck-agent: enrollment, machine identity, the stdlib WebSocket client and what the agent runs.

A small WebSocket server in this file plays the controller (crates/api/src/http.rs agent_session):
it sends the challenge, checks the HMAC answer and then drives exec, upload and terminal requests.
"""
import base64
import hashlib
import hmac
import http.server
import importlib.util
import json
import os
from pathlib import Path
import socket
import stat
import tempfile
import threading
import time
import unittest
import uuid

RUNTIME = Path(__file__).resolve().parents[1] / "runtime/rig-runtime.py"
KEY = "cd" * 32
GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def load(root, snapshots):
    os.environ["RIG_RUNTIME_ROOT"] = str(root)
    os.environ["RIG_SNAPSHOT_DIR"] = str(snapshots)
    spec = importlib.util.spec_from_file_location(f"rig_runtime_agent_{id(root)}", RUNTIME)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.ROOT.mkdir(parents=True, exist_ok=True)
    return module


class Controller:
    """One WebSocket connection, server side."""

    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.url = f"ws://127.0.0.1:{self.listener.getsockname()[1]}/agent/v1/connect"
        self.conn, self.buffer, self.headers = None, b"", {}

    def accept(self):
        self.conn, _ = self.listener.accept()
        self.conn.settimeout(10)
        while b"\r\n\r\n" not in self.buffer:
            self.buffer += self.conn.recv(4096)
        head, self.buffer = self.buffer.split(b"\r\n\r\n", 1)
        lines = head.decode().split("\r\n")
        assert lines[0].startswith("GET /agent/v1/connect "), lines[0]
        self.headers = {k.strip().lower(): v.strip() for k, _, v in (l.partition(":") for l in lines[1:])}
        accept = base64.b64encode(hashlib.sha1((self.headers["sec-websocket-key"] + GUID).encode()).digest()).decode()
        self.conn.sendall(f"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n".encode())

    def _exact(self, n):
        while len(self.buffer) < n:
            chunk = self.conn.recv(65536)
            if not chunk:
                raise ConnectionError("closed")
            self.buffer += chunk
        data, self.buffer = self.buffer[:n], self.buffer[n:]
        return data

    def send(self, message, opcode=0x1):
        payload = json.dumps(message).encode() if opcode == 0x1 else message
        n = len(payload)
        head = bytes([0x80 | opcode]) + (bytes([n]) if n < 126 else bytes([126]) + n.to_bytes(2, "big") if n < 65536 else bytes([127]) + n.to_bytes(8, "big"))
        self.conn.sendall(head + payload)

    def receive(self):
        """Next message from the agent; client frames must be masked."""
        while True:
            b0, b1 = self._exact(2)
            assert b1 & 0x80, "client frames are masked"
            n = b1 & 0x7F
            if n == 126:
                n = int.from_bytes(self._exact(2), "big")
            elif n == 127:
                n = int.from_bytes(self._exact(8), "big")
            mask = self._exact(4)
            payload = bytes(b ^ mask[i % 4] for i, b in enumerate(self._exact(n)))
            opcode = b0 & 0x0F
            if opcode == 0x1:
                return json.loads(payload)
            if opcode == 0xA:
                self.pong = payload
                continue
            if opcode == 0x8:
                raise ConnectionError("agent closed")

    def receive_until(self, predicate, limit=200):
        for _ in range(limit):
            message = self.receive()
            if predicate(message):
                return message
        raise AssertionError("expected message never arrived")

    def close(self):
        for s in (self.conn, self.listener):
            if s:
                s.close()


class AgentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        base = Path(self.temp.name)
        self.rt = load(base / "state", base / "run")
        self.machine = str(uuid.uuid4())
        self.controller = Controller()
        self.config = {"connect_url": self.controller.url, "machine_id": self.machine, "key": KEY}

    def tearDown(self):
        self.controller.close()
        self.temp.cleanup()

    def handshake(self, accept=True):
        result = {}

        def connect():
            try:
                result["session"] = self.rt.agent_connect(self.config)
            except Exception as error:  # noqa: BLE001
                result["error"] = error
        thread = threading.Thread(target=connect)
        thread.start()
        self.controller.accept()
        self.assertEqual(self.controller.headers["x-rig-machine"], self.machine)
        nonce = uuid.uuid4().hex
        self.controller.send({"type": "challenge", "nonce": nonce})
        auth = self.controller.receive()
        expected = hmac.new(bytes.fromhex(KEY), f"rigdeck-agent/v1|{self.machine}|{nonce}".encode(), hashlib.sha256).hexdigest()
        self.assertEqual((auth["type"], auth["machine"], auth["mac"]), ("auth", self.machine, expected))
        self.assertEqual(auth["version"], self.rt.VERSION)
        self.controller.send({"type": "ready"} if accept else {"type": "error", "error": "这台机器已删除"})
        thread.join(10)
        return result

    def run_session(self):
        session = self.handshake()["session"]
        threading.Thread(target=lambda: self.swallow(session.loop), daemon=True).start()
        return session

    @staticmethod
    def swallow(fn):
        try:
            fn()
        except Exception:  # noqa: BLE001 - the test closed the connection
            pass

    def test_rejected_machine_raises_agent_rejected(self):
        result = self.handshake(accept=False)
        self.assertIsInstance(result.get("error"), self.rt.AgentRejected)
        self.assertIn("已删除", str(result["error"]))

    def test_exec_upload_and_ping_over_the_connection(self):
        self.run_session()
        self.controller.send({"type": "exec", "id": 7, "command": "read line; echo got:$line; echo warn >&2; exit 3",
                              "stdin": base64.b64encode(b"hello\n").decode(), "timeout": 10})
        reply = self.controller.receive()
        self.assertEqual((reply["type"], reply["id"], reply["exit_code"]), ("result", 7, 3))
        self.assertEqual(base64.b64decode(reply["stdout"]), b"got:hello\n")
        self.assertEqual(base64.b64decode(reply["stderr"]), b"warn\n")
        target = Path(self.temp.name) / "uploaded.bin"
        self.controller.send({"type": "upload", "id": 8, "path": str(target), "mode": 0o640,
                              "data": base64.b64encode(b"\x00binary\xff").decode()})
        self.assertEqual(self.controller.receive(), {"type": "result", "id": 8, "ok": True})
        self.assertEqual(target.read_bytes(), b"\x00binary\xff")
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o640)
        self.controller.send({"type": "upload", "id": 9, "path": "relative/path", "mode": 0o600, "data": ""})
        self.assertIn("absolute", self.controller.receive()["error"])
        # A ping is answered with a pong carrying the same payload.
        self.controller.send(b"are-you-there", opcode=0x9)
        self.controller.send({"type": "exec", "id": 10, "command": "true", "timeout": 5})
        self.assertEqual(self.controller.receive()["id"], 10)
        self.assertEqual(self.controller.pong, b"are-you-there")

    def test_large_output_is_bounded_and_timeouts_kill_the_group(self):
        self.run_session()
        self.controller.send({"type": "exec", "id": 1, "command": "head -c 3000000 /dev/zero", "timeout": 20})
        reply = self.controller.receive()
        self.assertTrue(reply["truncated"])
        self.assertEqual(len(base64.b64decode(reply["stdout"])), self.rt.AGENT_STDOUT_LIMIT)
        marker = Path(self.temp.name) / "survived"
        started = time.monotonic()
        self.controller.send({"type": "exec", "id": 2, "command": f"(sleep 3; touch {marker}) & sleep 30", "timeout": 1})
        self.assertEqual(self.controller.receive(), {"type": "result", "id": 2, "timed_out": True})
        self.assertLess(time.monotonic() - started, 10)
        time.sleep(3.5)
        self.assertFalse(marker.exists(), "the whole process group is killed on timeout")

    def test_terminal_streams_a_pty(self):
        self.run_session()
        self.controller.send({"type": "term_open", "id": 5, "command": "stty -echo; echo tty:$(tty | cut -c1-8); cat", "cols": 100, "rows": 30})
        first = self.controller.receive_until(lambda m: m["type"] == "term_data" and b"tty:" in base64.b64decode(m["data"]))
        self.assertIn(b"tty:/dev/pts", base64.b64decode(first["data"]))
        self.controller.send({"type": "term_resize", "id": 5, "cols": 80, "rows": 20})
        self.controller.send({"type": "term_input", "id": 5, "data": base64.b64encode(b"ping\n").decode()})
        echoed = self.controller.receive_until(lambda m: m["type"] == "term_data" and b"ping" in base64.b64decode(m["data"]))
        self.assertEqual(echoed["id"], 5)
        self.controller.send({"type": "term_close", "id": 5})
        exit_message = self.controller.receive_until(lambda m: m["type"] == "term_exit")
        self.assertEqual(exit_message["id"], 5)


class EnrollTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        base = Path(self.temp.name)
        self.rt = load(base / "state", base / "run")
        self.requests, self.reply = [], None
        test = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                test.requests.append((self.path, body))
                status, reply = test.reply
                data = json.dumps(reply).encode()
                self.send_response(status)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.panel = f"http://127.0.0.1:{self.server.server_port}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.temp.cleanup()

    def test_enroll_writes_agent_and_push_configuration(self):
        machine = str(uuid.uuid4())
        self.reply = (200, {"machine_id": machine, "name": "rig-7", "created": True, "key": KEY,
                            "connect_url": "wss://panel.example.com/agent/v1/connect",
                            "push": {"url": "https://panel.example.com/ingest/v1/push", "digest": "e" * 64}})
        result = self.rt.enroll(self.panel + "/", "rgk_" + "1" * 64)
        self.assertEqual(result, {"machine_id": machine, "name": "rig-7", "created": True})
        path, body = self.requests[0]
        self.assertEqual(path, "/agent/v1/enroll")
        self.assertEqual(body["token"], "rgk_" + "1" * 64)
        self.assertRegex(body["fingerprint"], r"^[0-9a-f]{64}$")
        self.assertEqual(body["hostname"], socket.gethostname())
        agent = json.loads(self.rt.AGENT_FILE.read_text())
        self.assertEqual((agent["machine_id"], agent["key"], agent["panel"]), (machine, KEY, self.panel))
        self.assertEqual(stat.S_IMODE(self.rt.AGENT_FILE.stat().st_mode), 0o600)
        push = json.loads(self.rt.PUSH_FILE.read_text())
        self.assertEqual(push, {"url": "https://panel.example.com/ingest/v1/push", "machine_id": machine, "key": KEY, "digest": "e" * 64})
        self.assertEqual(self.rt.recent_events()[-1]["kind"], "enrolled")

    def test_rejected_key_explains_and_writes_nothing(self):
        self.reply = (401, {"error": "需要登录或令牌已失效"})
        with self.assertRaises(RuntimeError) as caught:
            self.rt.enroll(self.panel, "rgk_bad")
        self.assertIn("HTTP 401", str(caught.exception))
        self.assertFalse(self.rt.AGENT_FILE.exists())
        with self.assertRaises(ValueError):
            self.rt.enroll("ftp://example.com", "x")

    def test_fingerprint_follows_the_hardware_not_the_installed_system(self):
        def computer(mac, docker_mac="02:42:ac:11:00:02", machine_id="same-image-machine-id"):
            root = Path(tempfile.mkdtemp(dir=self.temp.name))
            (root / "etc").mkdir()
            (root / "etc/machine-id").write_text(machine_id + "\n")
            for name, address, physical in (("enp3s0", mac, True), ("docker0", docker_mac, False)):
                nic = root / "sys/class/net" / name
                nic.mkdir(parents=True)
                (nic / "address").write_text(address + "\n")
                if physical:
                    (nic / "device").mkdir()
            return root
        a = self.rt.machine_fingerprint(computer("aa:bb:cc:00:00:01"))
        self.assertEqual(a, self.rt.machine_fingerprint(computer("aa:bb:cc:00:00:01", docker_mac="02:42:ac:11:00:99")),
                         "virtual interfaces do not change the identity")
        self.assertNotEqual(a, self.rt.machine_fingerprint(computer("aa:bb:cc:00:00:02")),
                            "two rigs flashed from one image still differ")
        self.assertEqual(a, self.rt.machine_fingerprint(computer("aa:bb:cc:00:00:01", machine_id="reinstalled")),
                         "re-flashing the OS keeps the rig's identity")
        bare = Path(tempfile.mkdtemp(dir=self.temp.name))
        (bare / "etc").mkdir()
        (bare / "etc/machine-id").write_text("only-a-machine-id\n")
        self.assertRegex(self.rt.machine_fingerprint(bare), r"^[0-9a-f]{64}$", "machine-id is the fallback")
        with self.assertRaises(RuntimeError):
            self.rt.machine_fingerprint(Path(tempfile.mkdtemp(dir=self.temp.name)))


if __name__ == "__main__":
    unittest.main()
