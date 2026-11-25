#!/usr/bin/env python3
"""record_cast.py <out.cast> <title> -- <command...>

Runs the command (stdin /dev/null, stderr merged into stdout) and writes an
asciicast v2 file of its output: one event per chunk read, timed from the
start. A shown prompt line carries the command as typed. Also tees the output
to this process's stdout. Exit status: the command's."""
import json, os, shlex, subprocess, sys, time

out, title = sys.argv[1], sys.argv[2]
assert sys.argv[3] == "--"
cmd = sys.argv[4:]
shown = os.environ.get("CAST_SHOWN_COMMAND") or " ".join(shlex.quote(c) for c in cmd)
start = time.time()
with open(out, "w") as cast:
    cast.write(json.dumps({"version": 2, "width": int(os.environ.get("CAST_COLS", "150")),
                           "height": int(os.environ.get("CAST_ROWS", "46")),
                           "timestamp": int(start), "title": title,
                           "env": {"SHELL": "/bin/zsh", "TERM": "xterm-256color"}}) + "\n")
    def emit(text):
        cast.write(json.dumps([round(time.time() - start, 3), "o", text]) + "\n"); cast.flush()
        sys.stdout.write(text); sys.stdout.flush()
    emit(f"$ {shown}\r\n")
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    for line in iter(proc.stdout.readline, b""):
        emit(line.decode("utf-8", "replace").replace("\r\n", "\n").replace("\n", "\r\n"))
    rc = proc.wait()
    emit(f"$ echo $?\r\n{rc}\r\n")
sys.exit(rc)
