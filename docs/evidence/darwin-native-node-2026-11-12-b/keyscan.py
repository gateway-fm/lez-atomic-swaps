# keyscan.py <seconds> <key file>...: samples every process's arguments every
# 20 ms and reports how many samples showed any of the keys (never prints them).
import subprocess, sys, time
until = time.time() + float(sys.argv[1])
keys = [open(p).read().strip() for p in sys.argv[2:]]
samples = hits = 0
while time.time() < until:
    out = subprocess.run(["ps", "-axww", "-o", "pid=,command="], capture_output=True, text=True).stdout
    samples += 1
    for line in out.splitlines():
        if any(k in line for k in keys):
            hits += 1
            print("HIT in pid", line.split()[0], flush=True)
    time.sleep(0.02)
print(f"{samples} samples of every process's arguments, {hits} showed a role key")
