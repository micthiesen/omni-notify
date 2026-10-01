#!/usr/bin/env python3
"""Run on Boris after exact-SHA CI and compatibility checks. Never print credentials."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("sha", help="verified 40-character commit SHA")
parser.add_argument("--owner", required=True, help="existing Executor user ID")
parser.add_argument("--directory", default="/home/michael/compose/executor-events")
args = parser.parse_args()
if not re.fullmatch(r"[0-9a-f]{40}", args.sha):
    parser.error("An exact commit SHA is required")
if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", args.owner):
    parser.error("Invalid owner ID")
folder = Path(args.directory)
folder.mkdir(mode=0o700, parents=True, exist_ok=True)
if folder.is_symlink():
    raise SystemExit("Refusing a symlink deployment directory")
os.chmod(folder, 0o700)
# Docker provides the already-resolved existing credential; no new token or
# permissions are minted and no unrelated environment fields are written.
container = json.loads(subprocess.check_output(["docker", "inspect", "omni-notify"]))[0]
values = dict(item.split("=", 1) for item in container["Config"]["Env"] if "=" in item)
token = values.get("OMNI_MCP_TOKEN", "")
if not re.fullmatch(r"[^\s'\x00-\x1f]{32,}", token):
    raise SystemExit("Existing Omni MCP token unavailable or unsupported")

def private_write(path, text):
    temporary = path.with_suffix(path.suffix + ".new")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as stream:
        stream.write(text)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)

settings = folder / "deployment.env"
if settings.exists():
    shutil.copy2(settings, folder / "deployment.env.previous")
private_write(folder / "private.env", "OMNI_MCP_TOKEN='" + token + "'\n")
private_write(settings, f"ADAPTER_SHA={args.sha}\nEXECUTOR_ALLOWED_USER_ID={args.owner}\n")
shutil.copyfile(Path(__file__).with_name("compose.yml"), folder / "compose.yml")
command = ["docker", "compose", "--project-directory", str(folder), "--env-file", str(settings), "-f", str(folder / "compose.yml")]
subprocess.run(command + ["config", "--quiet"], check=True)
subprocess.run(command + ["pull"], check=True)
subprocess.run(command + ["up", "-d", "--wait"], check=True)
print("Adapter healthy at pinned SHA " + args.sha + "; public routing unchanged by installer")
