#!/usr/bin/env python3
"""RigDeck local runtime. No listener, no cloud dependency; Python stdlib + screen.

Every mutation records durable intent before touching a process. Only processes whose
PID AND /proc start time match an owned record may be signalled.
"""
import argparse
import base64
import csv
import contextlib
import fcntl
import hashlib
import json
import os
import pwd
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
import uuid

VERSION = "0.2.2"
ROOT = Path(os.environ.get("RIG_RUNTIME_ROOT", "/var/lib/rigdeck"))
# Controller reads this with a plain `cat` (no Python start-up, no sudo). /run is tmpfs.
SNAPSHOT_DIR = Path(os.environ.get("RIG_SNAPSHOT_DIR", "/run/rigdeck"))
EVENTS = ROOT / "events.jsonl"
SELF = str(Path(__file__).resolve())
MAX_OUTPUT = 1024 * 1024
NAME = re.compile(r"^[A-Za-z0-9_-]{1,40}$")


def read(path, default=None):
    try:
        return json.loads(Path(path).read_text())
    except FileNotFoundError:
        return {} if default is None else default


def atomic(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as f:
        os.chmod(f.name, 0o600)
        json.dump(value, f, separators=(",", ":"))
        f.flush()
        os.fsync(f.fileno())
        temp = f.name
    os.replace(temp, path)
    fd = os.open(str(path.parent), os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_volatile(path, value, mode=0o600, owner=None):
    """Replace a frequently rewritten cache file without fsync (disk wear on every 10 s tick)."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as f:
        os.chmod(f.name, mode)
        if owner is not None:
            os.chown(f.name, owner, -1)
        json.dump(value, f, separators=(",", ":"))
        temp = f.name
    os.replace(temp, path)


def uptime():
    return float(Path("/proc/uptime").read_text().split()[0])


def boot_id():
    return Path("/proc/sys/kernel/random/boot_id").read_text().strip()


def emit(level, kind, message, instance_name=None, **extra):
    """Append a HiveOS-style worker message. Never raises: messages must not break control flow."""
    try:
        record = {"id": str(uuid.uuid4()), "seq": time.time_ns(), "at": time.time(), "uptime": uptime(),
                  "boot_id": boot_id(), "level": level, "kind": kind, "instance": instance_name,
                  "message": str(message)[:2000], **({"detail": extra} if extra else {})}
        EVENTS.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with lock(ROOT / "events.lock"):
            with EVENTS.open("a") as stream:
                stream.write(json.dumps(record, separators=(",", ":")) + "\n")
            if EVENTS.stat().st_size > 1024 * 1024:
                with EVENTS.open("rb") as stream:
                    stream.seek(-256 * 1024, os.SEEK_END)
                    tail = stream.read().split(b"\n", 1)[-1]
                temp = EVENTS.with_suffix(".tmp")
                temp.write_bytes(tail)
                os.replace(temp, EVENTS)
    except Exception as error:  # noqa: BLE001 - diagnostics only
        print(f"event not recorded: {error}", file=sys.stderr, flush=True)


def recent_events(limit=40):
    try:
        with EVENTS.open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            stream.seek(max(0, stream.tell() - 32 * 1024))
            lines = stream.read().splitlines()
    except FileNotFoundError:
        return []
    events = []
    for line in lines[-limit:]:
        with contextlib.suppress(ValueError):
            events.append(json.loads(line))
    return events


def effective_policy(config):
    """Machine-level policy pushed by the controller overrides the policy frozen in a flight snapshot."""
    return {**(config.get("policy") or {}), **(read(ROOT / "policy.json").get("policy") or {})}


@contextlib.contextmanager
def lock(path, blocking=True):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as file:
        fcntl.flock(file, fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB))
        yield


def instance(name):
    if not NAME.fullmatch(name):
        raise ValueError("Invalid instance name")
    return ROOT / "instances" / name


def state(name):
    return read(instance(name) / "state.json")


def update(name, **fields):
    directory = instance(name)
    with lock(directory / "state.lock"):
        current = state(name)
        if all(current.get(key) == value for key, value in fields.items()):
            return current
        current.update(fields)
        current.update(instance=name, updated_at=time.time())
        atomic(directory / "state.json", current)
        return current


def identity(pid):
    try:
        # The comm field can contain spaces and parentheses.
        fields = Path(f"/proc/{int(pid)}/stat").read_text().rsplit(")", 1)[1].split()
        return None if fields[0] == "Z" else fields[19]
    except (FileNotFoundError, ValueError, ProcessLookupError):
        return None


def owned(pid, start):
    return bool(pid and start and identity(pid) == str(start))


def session(name):
    suffix = hashlib.sha256(str(ROOT).encode()).hexdigest()[:8]
    return f"rd-{suffix}-{name}"


def screen_alive(name):
    return subprocess.run(["screen", "-S", session(name), "-Q", "windows"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def screen_command(name, *args):
    return subprocess.run(["screen", "-S", session(name), "-p", "miner", "-X", *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def current_config(name, version=None):
    version = version or os.environ.get("RIG_RUNTIME_VERSION") or state(name).get("version")
    if not version or not re.fullmatch(r"[a-f0-9]{64}", version):
        raise ValueError("No valid installed version for instance")
    return read(instance(name) / "versions" / version / "config.json")


def start(name, version=None, preserve=False):
    directory = instance(name)
    with lock(directory / "control.lock"):
        old = state(name)
        version = version or old.get("version")
        config = current_config(name, version)
        if not config:
            raise ValueError("Configuration is missing")
        if owned(old.get("supervisor_pid"), old.get("supervisor_start")) and screen_alive(name):
            if old.get("version") != version:
                raise RuntimeError("Stop old version before switching")
            update(name, desired="running", maintenance=False)
            return
        if screen_alive(name):
            raise RuntimeError("Existing screen session has unverified ownership; refusing to replace")
        (directory / "STOP").unlink(missing_ok=True)
        update(name, desired="running", version=version, maintenance=False,
               phase="starting", warm_until=time.time() + config.get("warmup_seconds", 60),
               restarts=old.get("restarts", []) if preserve else [], last_error=None)
        log = directory / "console.log"
        subprocess.run(["screen", "-dmS", session(name), "-t", "miner", "-L", "-Logfile", str(log), sys.executable, SELF, "supervise", name], check=True)


def stop(name, desired="stopped", maintenance=False):
    directory = instance(name)
    with lock(directory / "control.lock"):
        old = state(name)
        if not old:
            return
        update(name, desired=desired, maintenance=maintenance, phase="stopping")
        (directory / "STOP").touch(mode=0o600)
        screen_command(name, "stuff", "\003")
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            s = state(name)
            if not owned(s.get("child_pid"), s.get("child_start")):
                break
            time.sleep(0.1)
        s = state(name)
        if owned(s.get("child_pid"), s.get("child_start")):
            with contextlib.suppress(ProcessLookupError):
                os.killpg(s["child_pid"], signal.SIGTERM)
            time.sleep(0.4)
            if owned(s.get("child_pid"), s.get("child_start")):
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(s["child_pid"], signal.SIGKILL)
        if owned(s.get("supervisor_pid"), s.get("supervisor_start")):
            with contextlib.suppress(ProcessLookupError):
                os.kill(s["supervisor_pid"], signal.SIGTERM)
        screen_command(name, "quit")
        try:
            cfg = current_config(name)
            if cfg.get("adapter") == "hive-custom":
                run_in_instance(name, "stop", timeout=20)
        except (ValueError, subprocess.SubprocessError) as error:
            update(name, stop_hook_error=str(error))
        update(name, phase="stopped", child_pid=None, child_start=None, supervisor_pid=None, supervisor_start=None)


def restart(name, reason="manual"):
    if state(name).get("desired") != "running" and reason != "manual":
        return
    stop(name, desired="running", maintenance=True)
    update(name, last_restart_reason=reason)
    start(name, preserve=reason != "manual")


def supervise(name):
    # This process lives inside screen; neither the SSH channel nor browser owns it.
    update(name, supervisor_pid=os.getpid(), supervisor_start=identity(os.getpid()))
    child = None

    def interrupt(signum, _frame):
        if child and child.poll() is None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child.pid, signum)
        if signum == signal.SIGTERM:
            raise SystemExit(0)

    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    while True:
        s = state(name)
        if s.get("desired") != "running" or s.get("maintenance") or (instance(name) / "STOP").exists():
            return
        config = current_config(name)
        child = subprocess.Popen(namespace_command(name, "run"), cwd=version_dir(name), start_new_session=True)
        update(name, phase="running", child_pid=child.pid, child_start=identity(child.pid), process_started_at=time.time())
        code = child.wait()
        update(name, child_pid=None, child_start=None, last_exit_code=code)
        if state(name).get("desired") != "running" or (instance(name) / "STOP").exists():
            return
        emit("warning", "miner_exited", f"矿工进程意外退出，退出码 {code}", name, exit_code=code)
        if not budget(name, config, "process_exit"):
            update(name, phase="faulted")
            return
        update(name, phase="cooldown")
        wait_until = time.monotonic() + effective_policy(config).get("cooldown_seconds", 30)
        while time.monotonic() < wait_until:
            if state(name).get("desired") != "running" or (instance(name) / "STOP").exists():
                return
            time.sleep(0.5)


def budget(name, config, reason):
    policy = effective_policy(config)
    now = time.time()
    recent = [t for t in state(name).get("restarts", []) if t > now - policy.get("restart_window_seconds", 3600)]
    if len(recent) >= policy.get("max_restarts", 5):
        if state(name).get("phase") != "faulted":
            emit("error", "recovery_exhausted", f"{len(recent)} 次自动恢复后仍异常（{REASONS.get(reason, reason)}），已停止自动重启", name, reason=reason)
        update(name, last_error=f"Recovery limit reached: {reason}", phase="faulted")
        return False
    recent.append(now)
    update(name, restarts=recent, last_restart_at=now, last_restart_reason=reason)
    return True


def version_dir(name):
    return instance(name) / "versions" / (os.environ.get("RIG_RUNTIME_VERSION") or state(name)["version"])


def namespace_command(name, action):
    cfg = current_config(name)
    if cfg.get("adapter") == "hive-custom":
        return ["unshare", "--mount", "--propagation", "private", sys.executable, SELF, "namespace", name, action]
    return [sys.executable, SELF, "native", name, action]


def run_in_instance(name, action, timeout=8, version=None):
    env = os.environ.copy()
    if version:
        env["RIG_RUNTIME_VERSION"] = version
        command = ["unshare", "--mount", "--propagation", "private", sys.executable, SELF, "namespace", name, action]
    else:
        command = namespace_command(name, action)
    return subprocess.run(command, env=env, capture_output=True, text=True, timeout=timeout, check=True).stdout


def custom_environment(name):
    config = current_config(name)
    package_name = read(version_dir(name) / "package-meta.json")["name"]
    env = os.environ.copy()
    env.update({str(k): str(v).replace("%API_PORT%", str(config["api_port"])) for k, v in config.get("environment", {}).items()})
    env.update(MINER_NAME="custom", MINER_NAME_CUSTOM=package_name,
               CUSTOM_MINER=package_name, MINER_DIR="/hive/miners/custom",
               CUSTOM_DIR=f"/hive/miners/custom/{package_name}", RIGDECK_INSTANCE=name,
               CUSTOM_API_PORT=str(config["api_port"]), API_PORT=str(config["api_port"]),
               MINER_API_PORT=str(config["api_port"]), MINER_LOG_BASENAME="/var/log/miner/custom/miner")
    return env


def namespace(name, action):
    directory = version_dir(name)
    hive = directory / "hive"
    logs = instance(name) / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    # Targets are created during bootstrap. All mounts here are private to this child.
    subprocess.run(["mount", "--bind", str(hive), "/hive"], check=True)
    subprocess.run(["mount", "--bind", str(logs), "/var/log/miner"], check=True)
    env = custom_environment(name)
    if action == "run" and current_config(name).get("cpus"):
        os.sched_setaffinity(0, {int(c) for c in current_config(name)["cpus"]})
    common = 'set -e; cd "$MINER_DIR/$CUSTOM_MINER"; source /hive/bin/rigdeck-helpers; source ./h-manifest.conf; if test -n "${CUSTOM_LOG_BASENAME:-}"; then mkdir -p "$(dirname "$CUSTOM_LOG_BASENAME")"; fi; '
    actions = {
        "config": common + 'rigdeck_hook() { source ./h-config.sh; }; rigdeck_hook',
        "run": common + 'rigdeck_hook() { source ./h-run.sh; }; rigdeck_hook',
        "stats": common + 'rigdeck_stats() { local khs=; local stats=; source ./h-stats.sh >&2; python3 - "$khs" "$stats" <<\'PY\'\nimport json,sys\nprint(json.dumps({"khs":sys.argv[1],"stats":json.loads(sys.argv[2]) if sys.argv[2] else None}))\nPY\n}; rigdeck_stats',
        "stop": common + 'rigdeck_hook() { if test -f ./h-stop.sh; then source ./h-stop.sh; fi; }; rigdeck_hook',
    }
    if action not in actions:
        raise ValueError("Unknown custom action")
    os.execvpe("bash", ["bash", "--noprofile", "--norc", "-c", actions[action]], env)


def native(name, action):
    config = current_config(name)
    if action == "run":
        command = config["argv"]
        executable = (version_dir(name) / "package" / command[0]).resolve()
        if not executable.is_relative_to((version_dir(name) / "package").resolve()):
            raise ValueError("Executable escapes package")
        argv = [str(executable)] + [str(v).replace("%API_PORT%", str(config["api_port"])) for v in command[1:]]
        cpus = config.get("cpus")
        if cpus:
            os.sched_setaffinity(0, {int(c) for c in cpus})
        os.chdir(version_dir(name) / "package")
        os.execv(argv[0], argv)
    if action == "stats":
        print(json.dumps(native_stats(config)))


def native_stats(config):
    """Read a native miner's local API in-process (no interpreter start-up per sample)."""
    port = config["api_port"]
    adapter = config["adapter"]
    if adapter == "cpuminer-opt":
        with socket.create_connection(("127.0.0.1", port), timeout=3) as conn:
            conn.sendall(b"summary")
            raw = conn.recv(65536).decode().strip("\x00")
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return dict(piece.split("=", 1) for piece in raw.replace("|", ";").split(";") if "=" in piece)
    path = "/2/summary" if adapter == "xmrig" else "/"
    request = urllib.request.Request(f"http://127.0.0.1:{port}{path}")
    with urllib.request.urlopen(request, timeout=3) as response:
        return json.loads(response.read(1024 * 1024).decode())


def normalize(config, raw):
    adapter = config["adapter"]
    # `device` says what is mining (cpu / gpu / cpu+gpu); `gpu_hs` is per-card hashrate keyed by PCI
    # bus number, so the fleet list can show a HiveOS-style tile per GPU next to the CPU bar.
    base = {"algorithm": None, "expected_algorithm": config.get("algorithm"), "coin": config.get("coin"), "hashrate_hs": None,
            "connected": None, "accepted": None, "rejected": None, "version": None, "uptime": None,
            "device": "cpu", "gpu_hs": []}
    if adapter == "hive-custom":
        stats = raw.get("stats") or {}
        khs = raw.get("khs")
        values = stats.get("hs") if isinstance(stats.get("hs"), list) else None
        factor = None
        if values is not None:
            unit = str(stats.get("hs_units") or "khs").lower().replace("/", "")
            factors = {"hs": 1, "khs": 1e3, "mhs": 1e6, "ghs": 1e9, "ths": 1e12}
            if unit not in factors:
                raise ValueError(f"Unknown hashrate unit: {unit}")
            factor = factors[unit]
        if khs not in (None, "", "null"):
            base["hashrate_hs"] = float(khs) * 1000
        elif values is not None:
            base["hashrate_hs"] = sum(float(x) for x in values if x is not None) * factor
        # HiveOS packages list the PCI bus of each GPU; CPU miners leave it out.
        buses = stats.get("bus_numbers")
        if values is not None and isinstance(buses, list) and any(isinstance(b, int) and b >= 0 for b in buses):
            base["device"] = "gpu"
            base["gpu_hs"] = [{"bus": buses[i] if i < len(buses) and isinstance(buses[i], int) else None,
                               "hs": float(v) * factor if v is not None else None} for i, v in enumerate(values)]
        ar = stats.get("ar") or []
        base.update(algorithm=stats.get("algo", base["algorithm"]), version=stats.get("ver"), uptime=stats.get("uptime"), accepted=ar[0] if ar else None, rejected=ar[1] if len(ar) > 1 else None, connected=stats.get("connected"))
    elif adapter == "xmrig":
        rate = (raw.get("hashrate", {}).get("total") or [None])[0]
        results = raw.get("results", {})
        connection = raw.get("connection", {})
        base.update(hashrate_hs=rate, algorithm=raw.get("algo"), version=raw.get("version"), uptime=raw.get("uptime"), accepted=results.get("shares_good"), rejected=(results.get("shares_total", 0) - results.get("shares_good", 0)), connected=bool(connection.get("pool")) and connection.get("uptime", 0) > 0)
    elif adapter == "srbminer":
        algorithms = raw.get("algorithms", [])
        match = next((x for x in algorithms if x.get("name", x.get("algorithm")) == config.get("algorithm")), None)
        if match is None:
            raise ValueError("SRBMiner API algorithm not present; unsupported schema or wrong algorithm")
        rates = match.get("hashrate", {}) if isinstance(match.get("hashrate"), dict) else {}
        total = rates.get("now", match.get("total_hashrate"))
        # SRBMiner can mine one algorithm on CPU and GPU at once: split the two.
        parts = {kind: rates.get(kind) for kind in ("cpu", "gpu") if isinstance(rates.get(kind), dict)}
        active = [kind for kind, part in parts.items() if (part.get("total") or 0) > 0]
        if active:
            base["device"] = "+".join(active)
        if total is None and parts:
            total = sum(part.get("total") or 0 for part in parts.values())
        if "gpu" in parts:
            bus_of = {str(d.get("device", d.get("id"))): d.get("bus_id") for d in raw.get("gpu_devices", []) if isinstance(d, dict)}
            base["gpu_hs"] = [{"bus": bus_of.get(key) if isinstance(bus_of.get(key), int) else None, "hs": value}
                              for key, value in parts["gpu"].items() if key != "total" and isinstance(value, (int, float))]
        base.update(hashrate_hs=total, algorithm=match.get("name", match.get("algorithm")), version=raw.get("miner_version"), uptime=raw.get("mining_time"), accepted=match.get("shares", {}).get("accepted"), rejected=match.get("shares", {}).get("rejected"), connected=match.get("pool", {}).get("connected"))
    elif adapter == "cpuminer-opt":
        if "SUMMARY" in raw:
            summary = raw["SUMMARY"]
            raw = summary[0] if isinstance(summary, list) else summary
        rate = raw.get("KHS", raw.get("KHS av"))
        base.update(hashrate_hs=float(rate) * 1000 if rate is not None else None, algorithm=raw.get("ALGO", raw.get("Algo", base["algorithm"])), version=raw.get("VER"), uptime=raw.get("UPTIME"), accepted=raw.get("ACC", raw.get("Accepted")), rejected=raw.get("REJ", raw.get("Rejected")))
    if base["hashrate_hs"] is not None and (not isinstance(base["hashrate_hs"], (int, float)) or not 0 <= base["hashrate_hs"] < 1e30):
        raise ValueError("Invalid hashrate value")
    return base


def sample(name):
    now = time.time()
    s = state(name)
    cfg = current_config(name)
    cache = {"instance": name, "observed_at": now, "sample_uptime": uptime(), "boot_id": boot_id(), "desired": s.get("desired"), "phase": s.get("phase"),
             "configured_coin": cfg.get("coin"), "configured_algorithm": cfg.get("algorithm"), "package_version": cfg.get("package_version"), "miner": cfg.get("adapter"),
             "pool": next(iter(cfg.get("pool_urls") or []), None) if isinstance(cfg.get("pool_urls"), list) else None,
             "miner_name": cfg.get("custom_name") or {"xmrig": "XMRig", "srbminer": "SRBMiner-MULTI", "cpuminer-opt": "cpuminer-opt"}.get(cfg.get("adapter"), cfg.get("adapter")),
             "process_alive": owned(s.get("child_pid"), s.get("child_start")), "screen": screen_alive(name),
             "version_id": s.get("version"), "sheet_id": s.get("sheet_id"), "sheet_name": s.get("sheet_name"),
             "sheet_version": s.get("sheet_version"), "error": None, "stats": None}
    if cache["process_alive"]:
        try:
            if cfg.get("adapter") == "hive-custom":
                raw = json.loads(run_in_instance(name, "stats"))
            else:
                raw = native_stats(cfg)
            cache["stats"] = normalize(cfg, raw)
            cache["stats_observed_at"] = now
        except (subprocess.SubprocessError, ValueError, OSError) as error:
            cache["error"] = str(error)[:2000]
    atomic_volatile(instance(name) / "stats.json", cache)
    return cache


REASONS = {"stats_invalid": "统计接口无效", "pool_disconnected": "矿池断开", "low_hashrate": "算力过低",
           "process_exit": "进程退出", "supervisor_missing": "守护进程丢失"}


def watchdog_once():
    for directory in sorted((ROOT / "instances").glob("*")):
        name = directory.name
        try:
            # Bound screen and custom logs without renaming an actively held descriptor.
            for log in [directory / "console.log", *(directory / "logs").rglob("*.log")]:
                if log.is_file() and log.stat().st_size > 32 * 1024**2:
                    with log.open("r+b") as stream:
                        stream.seek(-8 * 1024**2, os.SEEK_END)
                        tail = stream.read()
                        stream.seek(0)
                        stream.write(b"[RigDeck: older local output truncated]\n" + tail)
                        stream.truncate()
            cache = sample(name)
            s = state(name)
            cfg = current_config(name)
            now = time.time()
            if s.get("desired") != "running" or s.get("maintenance") or s.get("switching") or s.get("phase") == "faulted" or now < s.get("warm_until", 0):
                continue
            # Supervisor owns crash recovery. Watchdog only revives a missing supervisor.
            if not owned(s.get("supervisor_pid"), s.get("supervisor_start")):
                if budget(name, cfg, "supervisor_missing"):
                    if screen_alive(name):
                        emit("error", "ownership_unverified", "screen 会话归属无法确认，需要人工检查", name)
                        update(name, phase="faulted", last_error="Unverified screen ownership; manual inspection required")
                    else:
                        emit("warning", "supervisor_restarted", "守护进程丢失，已重新启动矿工", name)
                        start(name, preserve=True)
                continue
            policy = effective_policy(cfg)
            if policy.get("watchdog_enabled") is False:
                if s.get("bad_reason"):
                    update(name, bad_reason=None, bad_since=None)
                continue
            stats = cache.get("stats") or {}
            reason = None
            minimum = (policy.get("min_hashrate_by_algorithm") or {}).get(cfg.get("algorithm") or "", policy.get("min_hashrate_hs", 0.01))
            if cache.get("error") or (cache.get("process_alive") and stats.get("hashrate_hs") is None):
                reason = "stats_invalid"
            elif stats.get("connected") is False:
                reason = "pool_disconnected"
            elif stats.get("hashrate_hs") is not None and stats["hashrate_hs"] < minimum:
                reason = "low_hashrate"
            bad_since = s.get("bad_since", now) if s.get("bad_reason") == reason else now
            update(name, bad_reason=reason, bad_since=bad_since if reason else None)
            if reason and now - bad_since >= policy.get("failure_seconds", 120) and now - s.get("last_restart_at", 0) >= policy.get("cooldown_seconds", 30):
                if budget(name, cfg, reason):
                    emit("warning", "watchdog_restart", f"看门狗重启矿工：{REASONS.get(reason, reason)}持续 {int(now - bad_since)} 秒", name, reason=reason)
                    restart(name, reason)
                elif policy.get("allow_host_reboot") is True:
                    # Reboot opt-in is explicit, persisted and rate limited across boots.
                    reboot = read(ROOT / "last-reboot.json")
                    if now - reboot.get("at", 0) > policy.get("host_reboot_cooldown_seconds", 3600):
                        atomic(ROOT / "last-reboot.json", {"at": now, "instance": name, "reason": reason})
                        emit("error", "host_reboot", f"矿工多次恢复失败（{REASONS.get(reason, reason)}），看门狗重启整机", name, reason=reason)
                        subprocess.run(["systemctl", "reboot"], check=True)
        except Exception as error:
            print(f"watchdog {name}: {error}", file=sys.stderr, flush=True)


def cleanup_runtime(now=None):
    """Keep current/rollback versions and uncertain operations; compact old results.

    Small completed-operation tombstones are permanent replay protection.
    Never run concurrently with a deployment or delete referenced archives.
    """
    now = time.time() if now is None else now
    with lock(ROOT / "operation.lock", blocking=False):
        protected = {}
        for directory in (ROOT / "instances").glob("*"):
            record = state(directory.name)
            protected[directory.name] = {record.get("version"), record.get("committed_version")}
        for directory in (ROOT / "operations").glob("*"):
            record = read(directory / "state.json")
            if record.get("status") not in ("succeeded", "failed", "cancelled"):
                for name, value in record.get("previous", {}).items():
                    protected.setdefault(name, set()).add(value.get("version"))
                for name, version in record.get("versions", {}).items():
                    protected.setdefault(name, set()).add(version)
                # Queued runners may need any cached packages. Defer GC conservatively.
                return
            if record.get("finished_at", now) < now - 30 * 86400 and "payload" in record:
                record["payload_digest"] = hashlib.sha256(json.dumps(record.pop("payload"), sort_keys=True).encode()).hexdigest()
                record.pop("previous", None)
                record.pop("versions", None)
                record.pop("result", None)
                record["truncated"] = True
                atomic(directory / "state.json", record)
                for filename in ("output.log", "command.log"):
                    (directory / filename).unlink(missing_ok=True)
        hashes = set()
        for directory in (ROOT / "instances").glob("*"):
            versions = sorted((p for p in (directory / "versions").glob("*") if p.is_dir() and not p.is_symlink()), key=lambda p: p.stat().st_mtime, reverse=True)
            keep = protected.get(directory.name, set()) | {p.name for p in versions[:2]}
            for version in versions:
                if version.name not in keep and version.stat().st_mtime < now - 7 * 86400:
                    shutil.rmtree(version)
                else:
                    hashes.add(read(version / "config.json").get("sha256"))
        for archive in (ROOT / "cache").glob("*.tar"):
            if not archive.is_symlink() and re.fullmatch(r"[a-f0-9]{64}\.tar", archive.name) and archive.stem not in hashes and archive.stat().st_mtime < now - 7 * 86400:
                archive.unlink()


def safe_extract(archive, destination):
    destination = Path(destination).resolve()
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:*") as tar:
        members = tar.getmembers()
        if len(members) > 50000 or sum(m.size for m in members) > 8 * 1024**3:
            raise ValueError("Package exceeds extraction budget")
        for member in members:
            target = (destination / member.name).resolve()
            if not target.is_relative_to(destination) or member.name.startswith("/"):
                raise ValueError("Archive path escapes destination")
            if member.issym() or member.islnk() or member.isdev() or member.isfifo():
                raise ValueError("Package links/devices are not allowed; supply a regular-file archive")
        # Validation above works on Ubuntu Python 3.10; no extract filter dependency.
        for member in members:
            member.mode &= 0o777
            tar.extract(member, destination, set_attrs=False)
            target = destination / member.name
            if target.exists():
                target.chmod(member.mode & 0o755 if member.isdir() else member.mode & 0o755 | 0o600)


HELPERS = '''#!/bin/bash
export GPU_COUNT=0
function mkfile_from_symlink() { if [[ -L "$1" ]]; then cp --remove-destination "$(readlink -f "$1")" "$1"; fi; }
function gpu-detect() { printf '[]\\n'; }
function miner_ver() { echo "${CUSTOM_VERSION:-unknown}"; }
function message() { printf '%s\\n' "$*" >&2; }
export -f mkfile_from_symlink gpu-detect miner_ver message
'''


def prepare(config):
    name = config["instance"]
    directory = instance(name)
    capabilities = config.get("capabilities") or {}
    architectures = capabilities.get("architectures", [])
    if architectures and os.uname().machine not in architectures:
        raise ValueError(f"Unsupported hardware architecture: {os.uname().machine}; requires {architectures}")
    flags = set()
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith(("flags", "Features")):
            flags.update(line.split(":", 1)[1].split())
    missing = set(capabilities.get("cpu_flags", [])) - flags
    if missing:
        raise ValueError(f"Missing CPU features: {sorted(missing)}")
    for binary in capabilities.get("commands", []):
        if not shutil.which(binary):
            raise ValueError(f"Missing dependency: {binary}")
    cpus = config.get("cpus")
    if cpus and not set(cpus).issubset(os.sched_getaffinity(0)):
        raise ValueError("CPU allocation contains CPUs unavailable on this host")
    digest = config["sha256"].lower()
    if not re.fullmatch("[a-f0-9]{64}", digest):
        raise ValueError("Invalid SHA256")
    archive = ROOT / "cache" / f"{digest}.tar"
    if not archive.is_file():
        raise ValueError("Controller has not transferred verified package")
    version = hashlib.sha256(json.dumps(config, sort_keys=True).encode()).hexdigest()
    target = directory / "versions" / version
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() and read(target / "config.json") == config:
        return version
    with archive.open("rb") as stream:
        hasher = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    if hasher.hexdigest() != digest:
        raise ValueError("Package checksum mismatch")
    if shutil.disk_usage(ROOT).free < max(archive.stat().st_size * 4, 128 * 1024**2):
        raise ValueError("Insufficient free disk space")
    stage = Path(tempfile.mkdtemp(prefix="stage-", dir=directory.mkdir(parents=True, exist_ok=True) or directory))
    try:
        unpack = stage / "unpack"
        safe_extract(archive, unpack)
        if config["adapter"] == "hive-custom":
            manifests = list(unpack.rglob("h-manifest.conf"))
            if len(manifests) != 1:
                raise ValueError("Custom package must contain exactly one h-manifest.conf")
            package = manifests[0].parent
            for required in ("h-config.sh", "h-run.sh", "h-stats.sh"):
                if not (package / required).is_file():
                    raise ValueError(f"Missing custom package file: {required}")
            # Standard HiveOS packages can use fixed ports. A single instance does
            # not need a port capability declaration; avoid conflicting instances.
            if not (config.get("capabilities") or {}).get("instance_api_port"):
                for other in (ROOT / "instances").glob("*"):
                    if other.name != config["instance"] and state(other.name).get("desired") == "running":
                        other_cfg = read(other / "versions" / str(state(other.name).get("version", "")) / "config.json")
                        if other_cfg.get("adapter") == "hive-custom":
                            raise ValueError("另一个自定义矿工正在运行；该安装包尚未声明独立统计端口，请先停止原任务")
            for binary in ["unshare", "mount", "bash", "jq", "curl"]:
                if not shutil.which(binary):
                    raise ValueError(f"Missing dependency: {binary}")
            if not NAME.fullmatch(package.name):
                raise ValueError("Custom package directory must be a valid miner name")
            # The archive directory is authoritative; the UI name is a display name.
            (stage / "hive/miners/custom").mkdir(parents=True)
            shutil.copytree(package, stage / "hive/miners/custom" / package.name)
            atomic(stage / "package-meta.json", {"name": package.name})
            (stage / "hive/bin").mkdir()
            (stage / "hive/bin/rigdeck-helpers").write_text(HELPERS)
            (stage / "hive/etc").mkdir()
        else:
            roots = [p for p in unpack.iterdir()]
            package = roots[0] if len(roots) == 1 and roots[0].is_dir() else unpack
            shutil.copytree(package, stage / "package")
            executable = (stage / "package" / config["argv"][0]).resolve()
            if not executable.is_relative_to((stage / "package").resolve()) or not executable.is_file():
                raise ValueError("Executable not found within package")
            executable.chmod(executable.stat().st_mode | 0o100)
        atomic(stage / "config.json", config)
        if target.exists():
            raise ValueError("Existing version is incomplete; inspect it before retrying")
        stage.rename(target)
    finally:
        if stage.exists():
            shutil.rmtree(stage)
    return version


def check_port(port, previous=None):
    with socket.socket() as sock:
        try:
            sock.bind(("127.0.0.1", port))
        except OSError:
            if not previous or previous.get("api_port") != port:
                raise ValueError(f"API port {port} is occupied")


def apply_deployment(deployment, operation):
    configs = deployment["instances"]
    adopted = read(ROOT / "adopted.json", {})
    discovered = collect_software()["processes"]
    unknown = [p for p in discovered if p["adoption_required"] and not any(
        a.get("pid") == p["pid"] and str(a.get("start_identity")) == str(p["start_identity"])
        for a in adopted.values())]
    if unknown:
        raise ValueError("Unmanaged miner processes must be stopped or explicitly adopted first: " +
                         ", ".join(f"PID {p['pid']} {p['executable']}" for p in unknown))
    before = {path.name: state(path.name) for path in (ROOT / "instances").glob("*")}
    for cfg in configs:
        before.setdefault(cfg["instance"], {})
    versions = {}
    changed = []
    for cfg in configs:
        name = cfg["instance"]
        old = before[name]
        previous = current_config(name) if old.get("version") else None
        check_port(cfg["api_port"], previous)
        versions[name] = prepare(cfg)
        if cfg["adapter"] == "hive-custom":
            (instance(name) / "logs/custom").mkdir(parents=True, exist_ok=True)
            run_in_instance(name, "config", timeout=30, version=versions[name])
    operation_update(operation, phase="prepared", previous=before, versions=versions)
    legacy_stopped = []
    try:
        for legacy_name, legacy in adopted.items():
            if owned(legacy.get("pid"), legacy.get("start_identity")):
                check_cancel(operation)
                legacy_stopped.append(legacy_name)
                operation_update(operation, legacy_stopped=legacy_stopped)
                legacy_control(legacy_name, legacy, "stop")
        # Applying a flight sheet replaces the managed set, including removed instances.
        for name, old in before.items():
            if name not in versions and old.get("desired") == "running":
                check_cancel(operation)
                changed.append(name)
                update(name, switching=operation)
                stop(name, desired="stopped", maintenance=True)
        for cfg in configs:
            check_cancel(operation)
            name = cfg["instance"]
            if before[name]:
                update(name, switching=operation)
                stop(name, desired="running", maintenance=True)
            changed.append(name)
            update(name, version=versions[name], desired="running", maintenance=True,
                   switching=operation,
                   sheet_id=deployment["sheet_id"], sheet_name=deployment["sheet_name"], sheet_version=deployment["sheet_version"])
            start(name)
        operation_update(operation, phase="verifying")
        deadline = time.monotonic() + max(c.get("verification_seconds", 300) for c in configs)
        verified = {}
        while time.monotonic() < deadline:
            check_cancel(operation)
            for cfg in configs:
                name = cfg["instance"]
                sample_result = sample(name)
                stats = sample_result.get("stats") or {}
                algorithm = stats.get("algorithm")
                expected = cfg.get("algorithm")
                version_reported = bool(stats.get("version"))
                expected_version = cfg.get("package_version")
                version_match = str(stats.get("version", "")).removeprefix("v") == str(expected_version).removeprefix("v") if expected_version else None
                version_required = bool(expected_version) or cfg.get("adapter") != "hive-custom"
                valid = sample_result["process_alive"] and (version_reported or not version_required) and stats.get("hashrate_hs") is not None and stats["hashrate_hs"] > 0
                if version_match is False:
                    valid = False
                if expected and algorithm != expected:
                    valid = False
                if stats.get("connected") is False:
                    valid = False
                # Deployment completion is independent of the pool share interval.
                # Unknown connection/share evidence stays unknown in validation.
                verified[name] = {"config_delivered": True, "process_running": sample_result["process_alive"],
                                  "version_reported": version_reported, "running_version_id": sample_result["version_id"],
                                  "version_match": version_match,
                                  "algorithm_match": algorithm == expected if expected else None,
                                  "positive_hashrate": (stats.get("hashrate_hs") or 0) > 0,
                                  "pool_connected": stats.get("connected"), "accepted_shares": stats.get("accepted"),
                                  "valid": valid, "stats": stats}
            operation_update(operation, validation=verified)
            if all(v["valid"] for v in verified.values()):
                for name in changed:
                    update(name, committed_version=versions.get(name, before[name].get("version")), maintenance=False, switching=None)
                atomic(ROOT / "deployment.json", {**deployment, "operation_id": operation, "committed_at": time.time()})
                emit("success", "flight_applied", f"飞行表「{deployment.get('sheet_name')}」v{deployment.get('sheet_version')} 已应用并通过启动验证",
                     sheet_id=deployment.get("sheet_id"), operation=operation, instances=sorted(versions))
                return {"versions": versions, "validation": verified}
            time.sleep(2)
        raise RuntimeError("Startup verification timed out; see per-instance validation")
    except BaseException:
        rollback = {}
        for name in reversed(changed):
            try:
                stop(name)
                old = before[name]
                if old:
                    atomic(instance(name) / "state.json", {**old, "supervisor_pid": None, "child_pid": None})
                    if old.get("desired") == "running":
                        start(name)
                rollback[name] = "restored" if old else "new_instance_stopped"
            except Exception as error:
                rollback[name] = str(error)
        operation_update(operation, rollback=rollback)
        emit("error", "flight_rolled_back", f"飞行表「{deployment.get('sheet_name')}」应用失败，已回退：{sys.exc_info()[1]}",
             operation=operation, rollback=rollback)
        for legacy_name in legacy_stopped:
            try:
                legacy_control(legacy_name, adopted[legacy_name], "start")
            except Exception as error:
                rollback[f"legacy:{legacy_name}"] = str(error)
        operation_update(operation, rollback=rollback)
        raise


def adopt(payload):
    name = payload["name"]
    if not NAME.fullmatch(name) or int(payload["pid"]) <= 1:
        raise ValueError("Invalid adoption identity")
    if not owned(payload["pid"], payload["start_identity"]):
        raise ValueError("Process identity changed; rediscover before adopting")
    for field in ("start_script", "stop_script", "pid_script"):
        if not payload.get(field, "").strip():
            raise ValueError(f"Explicit {field} is required")
    current = read(ROOT / "adopted.json", {})
    current[name] = {**payload, "adopted_at": time.time()}
    atomic(ROOT / "adopted.json", current)
    return {"adopted": name, "pid": payload["pid"], "start_identity": payload["start_identity"]}


def legacy_control(name, record, action):
    if action == "start" and owned(record.get("pid"), record.get("start_identity")):
        return
    if action == "stop" and not owned(record.get("pid"), record.get("start_identity")):
        raise ValueError("Adopted process identity changed; refusing to run stop hook")
    subprocess.run(["bash", "-c", record[f"{action}_script"]], check=True, timeout=30)
    if action == "stop":
        deadline = time.monotonic() + 15
        while owned(record["pid"], record["start_identity"]) and time.monotonic() < deadline:
            time.sleep(.2)
        if owned(record["pid"], record["start_identity"]):
            raise RuntimeError("Adopted stop hook did not stop the identified process; no broad kill attempted")
    else:
        result = subprocess.run(["bash", "-c", record["pid_script"]], check=True, capture_output=True, text=True, timeout=10)
        pid = int(result.stdout.strip())
        started = identity(pid)
        if pid <= 1 or not started:
            raise RuntimeError("Adopted restart did not return a live process identity")
        records = read(ROOT / "adopted.json", {})
        records[name].update(pid=pid, start_identity=started)
        atomic(ROOT / "adopted.json", records)


def opdir(operation):
    uuid.UUID(operation)
    return ROOT / "operations" / operation


def operation_update(operation, **fields):
    path = opdir(operation) / "state.json"
    with lock(opdir(operation) / "state.lock"):
        current = read(path)
        current.update(fields, updated_at=time.time())
        atomic(path, current)


def check_cancel(operation):
    if (opdir(operation) / "CANCEL").exists():
        raise InterruptedError("Operation cancelled")


def operation_start(operation, payload):
    directory = opdir(operation)
    with lock(ROOT / "operation-start.lock"):
        current = read(directory / "state.json")
        if current:
            if current.get("payload_digest", hashlib.sha256(json.dumps(current.get("payload"), sort_keys=True).encode()).hexdigest()) != hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest():
                raise ValueError("Operation ID reused for different payload")
            return operation_status(operation)
        atomic(directory / "state.json", {"status": "queued", "payload": payload, "created_at": time.time()})
        with (directory / "output.log").open("ab", buffering=0) as output:
            process = subprocess.Popen([sys.executable, SELF, "operation-run", operation], stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        operation_update(operation, runner_pid=process.pid, runner_start=identity(process.pid))

    return {"status": "queued"}


def operation_run(operation):
    with lock(ROOT / "operation.lock"):
        record = read(opdir(operation) / "state.json")
        if record.get("status") not in ("queued",):
            return
        operation_update(operation, status="running", runner_pid=os.getpid(), runner_start=identity(os.getpid()))
        payload = record["payload"]
        try:
            check_cancel(operation)
            kind = payload["kind"]
            if kind == "apply":
                result = apply_deployment(payload["deployment"], operation)
            elif kind == "miner":
                labels = {"start": "启动", "stop": "停止", "restart": "重启"}
                for name in payload["instances"]:
                    check_cancel(operation)
                    {"start": start, "stop": stop, "restart": restart}[payload["operation"]](name)
                    emit("info", f"miner_{payload['operation']}", f"已{labels[payload['operation']]}矿工", name, operation=operation)
                result = {name: state(name) for name in payload["instances"]}
            elif kind == "command":
                result = run_command(operation, payload)
                emit("info", "command", "远程命令执行完成", operation=operation, exit_code=result.get("exit_code"))
            elif kind == "adopt":
                result = adopt(payload)
            else:
                raise ValueError("Unknown operation kind")
            operation_update(operation, status="succeeded", result=result, finished_at=time.time())
        except InterruptedError as error:
            operation_update(operation, status="cancelled", error=str(error), finished_at=time.time())
        except Exception as error:
            operation_update(operation, status="failed", error=str(error), finished_at=time.time())
            if payload.get("kind") != "apply":
                emit("error", f"{payload.get('kind')}_failed", f"远程操作失败：{error}", operation=operation)


def run_command(operation, payload):
    # Pipe is continuously drained even after output cap is reached.
    proc = subprocess.Popen(["bash", "-c", payload["script"]], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
    operation_update(operation, child_pid=proc.pid, child_start=identity(proc.pid))
    os.set_blocking(proc.stdout.fileno(), False)
    output = opdir(operation) / "command.log"
    deadline = time.monotonic() + payload.get("timeout_seconds", 300)
    total = 0
    interrupted = None
    with output.open("wb") as stream:
        while True:
            block = proc.stdout.read(65536)
            if block:
                remaining = max(0, MAX_OUTPUT - total)
                stream.write(block[:remaining])
                stream.flush()
                total += len(block)
            if proc.poll() is not None and not block:
                break
            if interrupted is None and ((opdir(operation) / "CANCEL").exists() or time.monotonic() >= deadline):
                interrupted = "cancelled" if (opdir(operation) / "CANCEL").exists() else "timeout"
                if owned(proc.pid, read(opdir(operation) / "state.json").get("child_start")):
                    os.killpg(proc.pid, signal.SIGTERM)
                deadline = time.monotonic() + 5
            elif interrupted and time.monotonic() >= deadline:
                if owned(proc.pid, read(opdir(operation) / "state.json").get("child_start")):
                    os.killpg(proc.pid, signal.SIGKILL)
            time.sleep(0.05)
    operation_update(operation, truncated=total > MAX_OUTPUT, exit_code=proc.returncode)
    if interrupted == "cancelled":
        raise InterruptedError("Command cancelled; owned process group terminated")
    if interrupted:
        raise RuntimeError("Command timed out; owned process group terminated")
    if proc.returncode:
        raise RuntimeError(f"Command exited with code {proc.returncode}")
    return {"exit_code": proc.returncode}


def operation_status(operation):
    directory = opdir(operation)
    result = read(directory / "state.json")
    if not result:
        return {"status": "missing", "output": "", "error": "No remote record; do not automatically replay"}
    if result["status"] in ("queued", "running") and not owned(result.get("runner_pid"), result.get("runner_start")) and time.time() - result.get("created_at", 0) > 5:
        result["status"] = "unknown"
        result["error"] = "Remote runner exited without recording an outcome; inspect validation/rollback and process state"
    log = directory / ("command.log" if result.get("payload", {}).get("kind") == "command" else "output.log")
    output = ""
    if log.exists():
        with log.open("rb") as stream:
            output = stream.read(MAX_OUTPUT).decode(errors="replace")
    details = {k: v for k, v in result.items() if k not in ("payload", "previous")}
    return {"status": result["status"], "output": output, "truncated": result.get("truncated", False), "result": details, "error": result.get("error")}


def recover_interrupted_deployments():
    committed = read(ROOT / "deployment.json")
    for directory in (ROOT / "operations").glob("*"):
        record = read(directory / "state.json")
        if record.get("status") not in ("running", "queued") or record.get("payload", {}).get("kind") != "apply":
            continue
        if owned(record.get("runner_pid"), record.get("runner_start")):
            continue
        operation = directory.name
        if committed.get("operation_id") == operation:
            operation_update(operation, status="succeeded", result={"reconciled_commit": True})
            continue
        previous = record.get("previous")
        if previous is None:
            # The preparation phase had not begun; no miner state was touched.
            operation_update(operation, status="failed", error="Interrupted before activation; previous miners preserved")
            continue
        versions = record.get("versions", {})
        conflict = any(state(name).get("version") not in (old.get("version"), versions.get(name), None)
                       for name, old in previous.items())
        if conflict:
            operation_update(operation, status="unknown", error="Runtime version changed outside interrupted operation; manual reconciliation required")
            continue
        try:
            for name, old in previous.items():
                current = state(name)
                if current.get("switching") == operation or current.get("version") != old.get("version"):
                    stop(name)
                    if old:
                        atomic(instance(name) / "state.json", {**old, "supervisor_pid": None, "child_pid": None, "switching": None})
            adopted = read(ROOT / "adopted.json", {})
            for name in record.get("legacy_stopped", []):
                legacy_control(name, adopted[name], "start")
            operation_update(operation, status="failed", error="Interrupted activation rolled back during boot recovery", rollback="restored_previous_intent")
        except Exception as error:
            operation_update(operation, status="unknown", error=f"Boot rollback failed: {error}")


def collect_hardware(base=Path("/"), dynamic_only=False):
    """Read optional hardware facts without making missing sensors look like zero."""
    def text(relative):
        try:
            return (base / relative).read_text(errors="replace").strip() or None
        except OSError:
            return None
    def joined(paths):
        values = [text(p) for p in paths]
        return " · ".join(v for v in values if v) or None
    os_release = text("etc/os-release") or ""
    os_name = next((line.partition("=")[2].strip().strip('"') for line in os_release.splitlines()
                    if line.startswith("PRETTY_NAME=")), None)
    temperatures = []
    for sensor in (base / "sys/class/hwmon").glob("hwmon*"):
        driver = text(str(sensor.relative_to(base) / "name"))
        if driver not in ("k10temp", "coretemp", "zenpower"):
            continue
        for channel in sensor.glob("temp*_input"):
            try:
                reading = float(channel.read_text()) / 1000
                if -50 <= reading <= 150:
                    temperatures.append({"driver": driver, "label": text(str(channel.relative_to(base)).replace("_input", "_label")) or channel.stem,
                                         "celsius": reading})
            except (OSError, ValueError):
                continue
    if dynamic_only:
        return {"cpu_temperature": max((t["celsius"] for t in temperatures), default=None), "cpu_temperatures": temperatures}
    return {"board": joined(["sys/class/dmi/id/board_vendor", "sys/class/dmi/id/board_name"]),
            "bios": joined(["sys/class/dmi/id/bios_version", "sys/class/dmi/id/bios_date"]),
            "os": os_name, "kernel": text("proc/sys/kernel/osrelease"),
            "cpu_temperature": max((t["celsius"] for t in temperatures), default=None),
            "cpu_temperatures": temperatures}


_NVIDIA_CACHE = {"at": -1e9, "data": {}}  # per-tick nvidia-smi readings
VIRTUAL_INTERFACES = re.compile(r"^(lo|veth|docker|br-|virbr|vnet|cni|flannel|cali|vxlan|tunl|kube|weave|lxc|tap|ifb)")


def _number(value, scale=1.0, low=None, high=None):
    try:
        result = float(str(value).strip()) * scale
    except (TypeError, ValueError):
        return None
    if (low is not None and result < low) or (high is not None and result > high):
        return None
    return round(result, 1)


def _nvidia_readings():
    """One nvidia-smi call per watchdog tick (~100 ms), only on rigs that have NVIDIA cards."""
    if time.monotonic() - _NVIDIA_CACHE["at"] < 9:
        return _NVIDIA_CACHE["data"]
    readings = {}
    if shutil.which("nvidia-smi"):
        try:
            output = subprocess.run(
                ["nvidia-smi", "--query-gpu=pci.bus_id,temperature.gpu,name,fan.speed,power.draw,utilization.gpu,"
                 "clocks.current.graphics,clocks.current.memory,memory.total,power.limit",
                 "--format=csv,noheader,nounits"],
                capture_output=True, text=True, timeout=3, check=True,
            ).stdout
            for row in csv.reader(output.splitlines()):
                if len(row) < 3:
                    continue
                bus, temperature, model, *rest = [cell.strip() for cell in row]
                fan, power, util, core, memory, vram, limit = (rest + [None] * 7)[:7]
                readings[bus.lower()[-12:]] = {
                    "model": model, "temperature_c": _number(temperature, low=-50, high=150),
                    "fan_pct": _number(fan, low=0, high=100), "power_w": _number(power, low=0, high=2000),
                    "util_pct": _number(util, low=0, high=100), "core_mhz": _number(core, low=0, high=10000),
                    "mem_mhz": _number(memory, low=0, high=30000), "vram_mb": _number(vram, low=0, high=1e7),
                    "power_limit_w": _number(limit, low=0, high=2000)}
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
    _NVIDIA_CACHE.update(at=time.monotonic(), data=readings)
    return readings


def collect_gpus(base=Path("/")):
    """Real PCI GPUs with temperature, fan, power and load, like HiveOS's GPU tiles.

    The ASPEED BMC display adapter is not a mining GPU. Missing readings stay None (never 0).
    """
    vendors = {"0x10de": "NVIDIA", "0x1002": "AMD", "0x8086": "Intel"}
    found = []
    for device in sorted((base / "sys/bus/pci/devices").glob("*")):
        try:
            vendor = (device / "vendor").read_text().strip().lower()
            device_class = (device / "class").read_text().strip().lower()
            if vendor not in vendors or not device_class.startswith("0x03"):
                continue
            device_id = (device / "device").read_text().strip().lower()
        except OSError:
            continue
        found.append((device, vendor, device_id))
    nvidia = _nvidia_readings() if base == Path("/") and any(v == "0x10de" for _, v, _ in found) else {}
    gpus = []
    for device, vendor, device_id in found:
        def sysfs(relative, scale=1.0, low=None, high=None):
            for path in sorted(device.glob(relative)):
                try:
                    return _number(path.read_text(), scale, low, high)
                except OSError:
                    continue
            return None
        temperature = sysfs("hwmon/hwmon*/temp1_input", 0.001, -50, 150) or sysfs("hwmon/hwmon*/temp*_input", 0.001, -50, 150)
        pwm = sysfs("hwmon/hwmon*/pwm1", 100 / 255, 0, 100)
        power = sysfs("hwmon/hwmon*/power1_average", 1e-6, 0, 2000) or sysfs("hwmon/hwmon*/power1_input", 1e-6, 0, 2000)
        def current_clock(name):
            # amdgpu pp_dpm_* lists states; the active one is marked "*", e.g. "1: 2400Mhz *".
            try:
                for line in (device / name).read_text().splitlines():
                    if line.rstrip().endswith("*"):
                        return _number(re.sub(r"[^0-9.]", "", line.split(":", 1)[1].split("M")[0]), low=0, high=30000)
            except (OSError, IndexError):
                pass
            return None
        memory_temperature = None
        for label in sorted(device.glob("hwmon/hwmon*/temp*_label")):
            try:
                if label.read_text().strip().lower() == "mem":
                    memory_temperature = _number((label.parent / label.name.replace("_label", "_input")).read_text(), 0.001, -50, 150)
            except OSError:
                continue
        vram = sysfs("mem_info_vram_total", 1 / 1048576, 0, 1e7)
        gpu = {"id": device.name, "vendor": vendors[vendor], "model": f"{vendors[vendor]} {device_id}",
               "temperature_c": temperature, "memory_temperature_c": memory_temperature, "fan_pct": pwm, "power_w": power,
               "util_pct": sysfs("gpu_busy_percent", 1, 0, 100), "core_mhz": current_clock("pp_dpm_sclk"),
               "mem_mhz": current_clock("pp_dpm_mclk"), "core_mv": sysfs("hwmon/hwmon*/in0_input", 1, 0, 2000),
               "vram_mb": round(vram) if vram is not None else None,
               "power_limit_w": sysfs("hwmon/hwmon*/power1_cap", 1e-6, 0, 2000)}
        try:
            gpu["bus"] = int(device.name.split(":")[1], 16)
        except (IndexError, ValueError):
            gpu["bus"] = None
        reading = nvidia.get(device.name.lower())
        if reading:
            gpu.update({key: value for key, value in reading.items() if value is not None})
        gpus.append(gpu)
    return gpus


def collect_inventory():
    topology = []
    for cpu_dir in Path("/sys/devices/system/cpu").glob("cpu[0-9]*"):
        try:
                numa = next(cpu_dir.glob("node[0-9]*"), None)
                topology.append({"cpu": int(cpu_dir.name[3:]), "socket": int((cpu_dir / "topology/physical_package_id").read_text()), "core": int((cpu_dir / "topology/core_id").read_text()), "numa_node": int(numa.name[4:]) if numa else None})
        except FileNotFoundError:
            pass
    model = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), None)
    return {key: value for key, value in {**collect_hardware(), "cpu_model": model, "topology": topology}.items() if key not in ("cpu_temperature", "cpu_temperatures")}


def collect_cpu_power(base=Path("/"), now=None):
    """Package energy deltas only: never sum overlapping core/DRAM subdomains."""
    now = time.monotonic() if now is None else now
    cache = ROOT / "power-sample.json"
    previous = read(cache)
    boot = (base / "proc/sys/kernel/random/boot_id").read_text().strip()
    samples, packages = {}, []
    seen = set()
    failed = False
    for zone in sorted((base / "sys/class/powercap").glob("*")):
        try:
            name = (zone / "name").read_text().strip()
            if not re.fullmatch(r"package-\d+", name) or name in seen:
                continue
            seen.add(name)
            energy = int((zone / "energy_uj").read_text())
            maximum = int((zone / "max_energy_range_uj").read_text())
            samples[name] = energy
            old = previous.get("samples", {}).get(name)
            elapsed = now - previous.get("time", now)
            watts = None
            if previous.get("boot") == boot and old is not None and 0 < elapsed <= 60:
                delta = energy - old
                if delta < 0:
                    delta += maximum
                value = delta / elapsed / 1e6
                if 0 <= value <= 2000:
                    watts = round(value, 2)
            packages.append({"name": name, "power_w": watts})
        except (OSError, ValueError):
            if (zone / "energy_uj").exists():
                failed = True
            continue
    atomic_volatile(cache, {"time": now, "boot": boot, "samples": samples})
    total = round(sum(p["power_w"] for p in packages), 2) if not failed and packages and all(p["power_w"] is not None for p in packages) else None
    return {"cpu_power_w": total, "cpu_power_packages": packages,
            "cpu_power_source": "Linux powercap / RAPL" if packages else None}


def collect_system():
    def meminfo():
        return {line.split(":")[0]: int(line.split()[1]) * 1024 for line in Path("/proc/meminfo").read_text().splitlines()}
    cpu = [int(v) for v in Path("/proc/stat").read_text().splitlines()[0].split()[1:]]
    previous = read(ROOT / "cpu-sample.json")
    total, idle = sum(cpu[:8]), cpu[3] + cpu[4]
    percent = None
    if previous and total > previous["total"]:
        percent = 100 * (1 - (idle - previous["idle"]) / (total - previous["total"]))
    atomic_volatile(ROOT / "cpu-sample.json", {"total": total, "idle": idle})
    memory = meminfo()
    # Disks and network interfaces are not uploaded: a mining rig's list only needs CPU, memory,
    # temperature, power and its primary address.
    gpus = collect_gpus()
    drivers = {}
    for module in (("nvidia", "NVIDIA"), ("amdgpu", "AMD")) if gpus else ():
        try:
            drivers[module[1]] = Path(f"/sys/module/{module[0]}/version").read_text().strip()
        except OSError:
            continue
    return {**collect_hardware(dynamic_only=True), **collect_cpu_power(), "gpus": gpus, "gpu_drivers": drivers, "cpu_pct": percent, "logical_cpus": os.cpu_count(),
            "memory_total": memory["MemTotal"], "memory_used": memory["MemTotal"] - memory["MemAvailable"],
            "memory_pct": 100 * (1 - memory["MemAvailable"] / memory["MemTotal"]),
            "swap_total": memory["SwapTotal"], "swap_used": memory["SwapTotal"] - memory["SwapFree"],
            "load": list(os.getloadavg()), "uptime": uptime(), "addresses": local_addresses(),
            "runtime_version": VERSION}


def is_miner_executable(executable):
    # GNOME Tracker calls its file indexers "miners"; these are not mining workloads.
    # Limit the exception to the system installation paths and exact program names.
    path = Path(executable.removesuffix(" (deleted)"))
    # XMRig Proxy forwards pool traffic; it does not run mining worker threads.
    # Match only the executable basename, never a directory or arbitrary "proxy" suffix.
    if path.name.lower() == "xmrig-proxy":
        return False
    if str(path.parent) in ("/usr/libexec", "/usr/lib/tracker", "/usr/lib/tracker-miners-2.0",
                            "/usr/lib/tracker-miners-3.0") and re.fullmatch(
            r"tracker-miner-(?:fs|rss)(?:-[23])?", path.name):
        return False
    return any(word in path.name.lower() for word in ("miner", "xmrig"))


def collect_software():
    managed = {s.get("child_pid"): s for s in [state(p.name) for p in (ROOT / "instances").glob("*")]}
    processes = []
    for path in Path("/proc").glob("[0-9]*"):
        try:
            exe = os.readlink(path / "exe")
            if is_miner_executable(exe):
                pid = int(path.name)
                record = managed.get(pid) or managed.get(os.getpgid(pid))
                processes.append({"pid": pid, "start_identity": identity(pid), "executable": exe,
                                  "managed_instance": record.get("instance") if record else None,
                                  "coin": None, "algorithm": None, "adoption_required": record is None})
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            pass
    return {"processes": processes, "coin_detection": "unknown unless provided by a managed snapshot"}


def collect_mining():
    now = time.time()
    uptime = float(Path("/proc/uptime").read_text().split()[0])
    boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    items = []
    for p in (ROOT / "instances").glob("*"):
        item = read(p / "stats.json")
        if item.get("boot_id") == boot and isinstance(item.get("sample_uptime"), (int, float)):
            at = now - max(0, uptime - item["sample_uptime"])
            item["observed_at"] = at
            if item.get("stats_observed_at") is not None:
                item["stats_observed_at"] = at
        elif item.get("boot_id"):
            item.update(process_alive=False, stats=None, stats_observed_at=None)
        items.append(item)
    return {"instances": items, "collected_at": now}


_ADDRESS_CACHE = {"at": -1e9, "data": []}


def local_addresses():
    """Primary IPv4 per physical interface (HiveOS shows the worker's LAN IP in the list).

    Cached for 60 s so the 10 s watchdog tick does not fork `ip` every time.
    """
    if time.monotonic() - _ADDRESS_CACHE["at"] < 60:
        return _ADDRESS_CACHE["data"]
    try:
        output = subprocess.run(["ip", "-4", "-o", "addr", "show", "scope", "global"], capture_output=True, text=True, timeout=2).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    addresses = []
    for line in output.splitlines():
        parts = line.split()
        if len(parts) >= 4 and not VIRTUAL_INTERFACES.match(parts[1]):
            addresses.append({"interface": parts[1], "address": parts[3].split("/")[0]})
    _ADDRESS_CACHE.update(at=time.monotonic(), data=addresses)
    return addresses


def snapshot_reader():
    """Owner uid for the published snapshot: the controller's SSH user (root keeps 0600)."""
    name = read(ROOT / "reader.json").get("user")
    if not name or name == "root":
        return None
    try:
        return pwd.getpwnam(name).pw_uid
    except KeyError:
        return None


def publish_snapshot():
    """Everything the controller polls every 10 s, in one file readable with `cat`."""
    boot = boot_id()
    now_uptime = uptime()
    instances = []
    for directory in sorted((ROOT / "instances").glob("*")):
        item = read(directory / "stats.json")
        if item and item.get("boot_id") not in (None, boot):
            item.update(process_alive=False, stats=None, stats_observed_at=None)
        if item:
            instances.append(item)
    policy = read(ROOT / "policy.json")
    snapshot = {"schema": 1, "runtime_version": VERSION, "boot_id": boot, "uptime": now_uptime, "written_at": time.time(),
                "system": collect_system(), "mining": {"instances": instances}, "events": recent_events(),
                "policy_digest": policy.get("digest")}
    SNAPSHOT_DIR.mkdir(parents=True, exist_ok=True)
    os.chmod(SNAPSHOT_DIR, 0o755)
    owner = snapshot_reader()
    atomic_volatile(SNAPSHOT_DIR / "snapshot.json", snapshot, mode=0o400 if owner is not None else 0o600, owner=owner)
    return snapshot


def log_tail(name, lines=200):
    lines = max(1, min(int(lines), 2000))
    path = instance(name) / "console.log"
    try:
        with path.open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            size = stream.tell()
            stream.seek(max(0, size - 256 * 1024))
            data = stream.read()
    except FileNotFoundError:
        return {"instance": name, "text": "", "size": 0}
    text = b"\n".join(data.splitlines()[-lines:]).decode(errors="replace")
    # Screen logs carry terminal control sequences; strip the common ones for a plain viewer.
    text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07|\r", "", text)
    return {"instance": name, "text": text, "size": size}


MAX_PACKAGE = 512 * 1024 * 1024


def sha256_file(path):
    hasher = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def fetch_package(digest, url):
    """Download a miner straight to this rig's cache, like HiveOS rigs pulling miners.

    `digest` is the flight sheet's pinned SHA256, or "-" when none was given (official GitHub
    builds): then HTTPS is the trust anchor, and the digest of what was stored is returned so
    the deployment addresses exactly these bytes. Returns the digest in the cache.
    """
    pinned = None if digest == "-" else digest.lower()
    if pinned is not None and not re.fullmatch(r"[a-f0-9]{64}", pinned):
        raise ValueError("Invalid SHA256")
    if not re.match(r"^https?://", url):
        raise ValueError("Only HTTP(S) package sources are supported")
    if pinned is None and not url.startswith("https://"):
        raise ValueError("Plain HTTP downloads need a pinned SHA256")
    cache = ROOT / "cache"
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    index_path = cache / "url-index.json"
    try:
        index = read(index_path)
    except ValueError:
        index = {}
    if not isinstance(index, dict):
        index = {}
    known = pinned or index.get(url)
    if known and (cache / f"{known}.tar").is_file() and sha256_file(cache / f"{known}.tar") == known:
        return known
    temp = cache / f".download.{uuid.uuid4().hex}.part"
    command = ["curl", "--fail", "--location", "--silent", "--show-error", "--connect-timeout", "15",
               "--max-time", "1800", "--retry", "2", "--retry-delay", "3", "--max-filesize", str(MAX_PACKAGE),
               "--proto", "=https,http", "--proto-redir", "=https" if pinned is None else "=https,http",
               "-o", str(temp), url]
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=1900)
        if result.returncode != 0:
            raise RuntimeError(f"下载失败 (curl {result.returncode}): {result.stderr.strip()[-300:]}")
        actual = sha256_file(temp)
        if pinned is not None and actual != pinned:
            raise ValueError(f"安装包 SHA256 不匹配（实际 {actual[:12]}…）")
        os.chmod(temp, 0o600)
        os.replace(temp, cache / f"{actual}.tar")
    finally:
        temp.unlink(missing_ok=True)
    if pinned is None:
        index[url] = actual
        # Bounded: keep the most recent URLs only.
        atomic(index_path, dict(list(index.items())[-64:]))
    emit("info", "package_downloaded", f"已下载安装包 {url.rsplit('/', 1)[-1][:80]}", sha256=actual)
    return actual


def set_policy(payload):
    if not isinstance(payload.get("policy"), dict) or not re.fullmatch(r"[a-f0-9]{64}", str(payload.get("digest", ""))):
        raise ValueError("Invalid policy payload")
    atomic(ROOT / "policy.json", {"digest": payload["digest"], "policy": payload["policy"], "updated_at": time.time()})
    emit("info", "policy_updated", "看门狗与恢复策略已更新")
    return {"digest": payload["digest"]}


def main():
    ROOT.mkdir(parents=True, exist_ok=True, mode=0o700)
    parser = argparse.ArgumentParser()
    parser.add_argument("command")
    parser.add_argument("arguments", nargs="*")
    args = parser.parse_args()
    cmd, params = args.command, args.arguments
    if cmd in ("start", "stop", "restart"):
        {"start": start, "stop": stop, "restart": restart}[cmd](params[0])
        print(json.dumps(state(params[0])))
    elif cmd == "status":
        print(json.dumps(state(params[0]) if params else [state(p.name) for p in (ROOT / "instances").glob("*")]))
    elif cmd == "log-tail":
        print(json.dumps(log_tail(params[0], params[1] if len(params) > 1 else 200)))
    elif cmd == "fetch-package":
        try:
            print(fetch_package(params[0], params[1]))
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            # One readable line for the controller's message feed, not a traceback.
            print(str(error), file=sys.stderr)
            sys.exit(1)
    elif cmd == "set-policy":
        print(json.dumps(set_policy(json.loads(base64.b64decode(params[0])))))
    elif cmd == "set-reader":
        if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}", params[0]):
            raise ValueError("Invalid reader account")
        atomic(ROOT / "reader.json", {"user": params[0]})
        print(json.dumps({"reader": params[0]}))
    elif cmd == "events":
        print(json.dumps(recent_events(int(params[0]) if params else 40)))
    elif cmd == "log":
        os.execvp("tail", ["tail", "-n", "200", "-f", str(instance(params[0]) / "console.log")])
    elif cmd == "attach":
        os.execvp("screen", ["screen", "-x", session(params[0]), "-p", "miner"])
    elif cmd == "supervise":
        supervise(params[0])
    elif cmd == "native":
        native(*params)
    elif cmd == "namespace":
        namespace(*params)
    elif cmd == "watchdog":
        with lock(ROOT / "watchdog.lock", blocking=False):
            next_cleanup = 0
            emit("info", "watchdog_started", f"运行层 {VERSION} 看门狗已启动")
            while True:
                started = time.monotonic()
                watchdog_once()
                try:
                    publish_snapshot()
                except Exception as error:  # noqa: BLE001 - controller falls back to legacy collection
                    print(f"snapshot not published: {error}", file=sys.stderr, flush=True)
                if started >= next_cleanup:
                    try:
                        cleanup_runtime()
                    except (OSError, ValueError) as error:
                        print(f"runtime cleanup deferred: {error}", file=sys.stderr)
                    next_cleanup = started + 3600
                if "--once" in params or "once" in params:
                    return
                time.sleep(max(0, 10 - (time.monotonic() - started)))
    elif cmd == "recover":
        emit("info", "boot", "系统启动，恢复期望的矿工状态")
        recover_interrupted_deployments()
        for path in (ROOT / "instances").glob("*"):
            s = state(path.name)
            if s.get("desired") == "running" and not s.get("maintenance") and s.get("phase") != "faulted":
                start(path.name, preserve=True)
    elif cmd == "operation-start":
        print(json.dumps(operation_start(params[0], json.loads(base64.b64decode(params[1])))))
    elif cmd == "operation-run":
        operation_run(params[0])
    elif cmd == "operation-status":
        print(json.dumps(operation_status(params[0])))
    elif cmd == "cancel":
        (opdir(params[0]) / "CANCEL").touch(mode=0o600)
        print(json.dumps({"cancel_requested": True}))
    elif cmd == "collect":
        kind = params[0]
        result = {"system": collect_system, "hardware": collect_inventory, "software": collect_software, "mining": collect_mining}[kind]()
        print(json.dumps(result))
    elif cmd == "check":
        print(json.dumps({"version": VERSION, "screen": bool(shutil.which("screen")), "python": sys.version, "root": str(ROOT)}))
    else:
        raise ValueError("Unknown runtime command")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(json.dumps({"error": str(error)}), file=sys.stderr)
        sys.exit(1)
