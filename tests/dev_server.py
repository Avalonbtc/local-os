"""Start the isolated local validation server; no real server credentials are used."""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--work", type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
binary = root / "target/debug" / ("rigdeck.exe" if os.name == "nt" else "rigdeck")
work = args.work.resolve()
work.mkdir(parents=True, exist_ok=True)
settings = work / "dev-env.json"
env = os.environ.copy()
if settings.exists():
    config = json.loads(settings.read_text())
else:
    config = {"DATABASE_URL": os.environ.get("RIGDECK_TEST_DATABASE_URL", "postgres://rigdeck:rigdeck-local-test-only@127.0.0.1:55432/rigdeck"),
              "RIGDECK_MASTER_KEY": base64.b64encode(os.urandom(32)).decode(),
              "RIGDECK_BIND": "127.0.0.1:8080", "RIGDECK_ORIGIN": "http://127.0.0.1:5173",
              "RIGDECK_INSECURE_LOCALHOST": "1", "RIGDECK_DATA_DIR": str(work / "rigdeck-data")}
    settings.write_text(json.dumps(config))
env.update(config)
env["RIGDECK_ADMIN_PASSWORD"] = "RigDeck-local-test-2026"
result = subprocess.run([str(binary), "create-admin", "admin"], env=env, cwd=root, capture_output=True, text=True, encoding="utf-8")
if result.returncode and "已存在" not in result.stderr:
    raise RuntimeError(result.stderr)
env.pop("RIGDECK_ADMIN_PASSWORD", None)
with (work / "server.log").open("w") as output, (work / "server-error.log").open("w") as error:
    child = subprocess.Popen([str(binary), "serve"], env=env, cwd=root, stdout=output, stderr=error,
                             creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
(work / "server.pid").write_text(str(child.pid))
print(f"Local validation server PID {child.pid}, API http://127.0.0.1:8080")
