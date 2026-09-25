#!/usr/bin/env python3
"""Generate first-install secrets without overwriting an existing installation."""
import argparse
import base64
import os
from pathlib import Path
import re
import secrets

parser=argparse.ArgumentParser()
parser.add_argument("--host",default="rigdeck.local")
parser.add_argument("--listen",default="0.0.0.0")
args=parser.parse_args()
if not re.fullmatch(r"[A-Za-z0-9.-]+",args.host):
    parser.error("host must be a hostname or IPv4 address without a scheme")
if not re.fullmatch(r"[0-9.]+",args.listen):
    parser.error("listen must be an IPv4 address")
root=Path(__file__).resolve().parents[1]
os.umask(0o077)
directory=root/"secrets";directory.mkdir(mode=0o700,exist_ok=True)
key=directory/"master.key"
env=root/".env"
if key.exists() or env.exists():
    parser.error("existing .env or master.key found; refusing to replace secrets")
key.write_text(base64.b64encode(secrets.token_bytes(32)).decode()+"\n")
env.write_text(f"POSTGRES_PASSWORD={secrets.token_hex(32)}\nRIGDECK_HOST={args.host}\nRIGDECK_LISTEN={args.listen}\n")
# Compose file secrets retain the host file mode. The parent stays private; only the
# dedicated application container receives the file at /run/secrets/master_key.
key.chmod(0o444)
print("Created .env and secrets/master.key. Keep the master key in a separate backup.")
