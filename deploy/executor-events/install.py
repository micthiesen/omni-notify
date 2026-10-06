#!/usr/bin/env python3
"""One-time setup of the adapter's Compose project on Boris. Never prints credentials.

After setup, Boris's deploy timer pulls and restarts the adapter whenever a new
image is published or its Compose configuration changes.
"""
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess

COMPOSE_DIR = Path("/home/michael/compose")

parser = argparse.ArgumentParser()
parser.add_argument("--owner", required=True, help="existing Executor user ID")
parser.add_argument("--directory", default=str(COMPOSE_DIR / "executor-events"))
args = parser.parse_args()
if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", args.owner):
    parser.error("Invalid owner ID")
folder = Path(args.directory)
folder.mkdir(mode=0o700, parents=True, exist_ok=True)
if folder.is_symlink():
    raise SystemExit("Refusing a symlink deployment directory")
os.chmod(folder, 0o700)


def private_write(path, text):
    temporary = path.with_suffix(path.suffix + ".new")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as stream:
        stream.write(text)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


private_write(folder / "deployment.env", f"EXECUTOR_ALLOWED_USER_ID={args.owner}\n")
# The token now comes from the main .env; remove the old copied credential.
(folder / "private.env").unlink(missing_ok=True)
shutil.copyfile(Path(__file__).with_name("compose.yml"), folder / "compose.yml")
command = [
    "docker", "compose", "--project-directory", str(folder),
    "--env-file", str(COMPOSE_DIR / ".env"), "--env-file", str(folder / "deployment.env"),
    "-f", str(folder / "compose.yml"),
]
subprocess.run(command + ["config", "--quiet"], check=True)
subprocess.run(command + ["pull"], check=True)
subprocess.run(command + ["up", "-d", "--wait"], check=True)
print("Adapter healthy; Boris's deploy timer keeps it current. Public routing unchanged.")
