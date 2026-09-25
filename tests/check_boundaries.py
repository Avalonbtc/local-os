"""Enforce dependency direction, API use of application services and feature exports."""
import json
from pathlib import Path
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version=1"], cwd=root))
allowed = {"rig-domain": set(), "rig-application": {"rig-domain"},
           "rig-infrastructure": {"rig-domain", "rig-application"},
           "rig-api": {"rig-domain", "rig-application"},
           "rigdeck": {"rig-domain", "rig-application", "rig-infrastructure", "rig-api"}}
errors = []
for module in ("identity", "fleet", "catalog", "flights", "mining", "jobs", "telemetry", "bmc", "audit"):
    if not (root / "crates/application/src" / f"{module}.rs").is_file():
        errors.append(f"missing application module: {module}")
for package in metadata["packages"]:
    for dep in package["dependencies"]:
        name = dep["name"]
        if name.startswith("rig-") and name not in allowed[package["name"]]:
            errors.append(f"{package['name']} -> {name}: forbidden layer dependency")
        if package["name"] == "rig-domain" and name in {"axum", "sqlx", "russh", "reqwest"}:
            errors.append(f"domain depends on infrastructure: {name}")
for source in (root / "crates/api/src").rglob("*.rs"):
    if ".repository" in source.read_text(encoding="utf-8"):
        errors.append(f"{source}: API bypasses application services")
for source in (root / "frontend/src/features").rglob("*.tsx"):
    own = source.relative_to(root / "frontend/src/features").parts[0]
    for target in re.findall(r"from ['\"]([^'\"]+)['\"]", source.read_text(encoding="utf-8")):
        if target.startswith("../") and not target.startswith("../../shared"):
            parts = target.split("/")
            if len(parts) > 2 and parts[1] != own and parts[-1] != "index":
                errors.append(f"{source}: imports another feature's internal file {target}")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print("Backend dependency direction, application entry points and frontend feature boundaries passed")
