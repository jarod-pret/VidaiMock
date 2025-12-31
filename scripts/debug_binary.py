import subprocess
import os
import sys
import time

binary = os.path.abspath("vidaimock-dist/vidaimock")
env = os.environ.copy()
env["VIDAIMOCK_LOG_LEVEL"] = "debug"
print(f"Launching {binary}")
with open("debug_run.log", "w") as f:
    p = subprocess.Popen([binary, "--port", "8100"], stdout=f, stderr=subprocess.STDOUT, env=env)
    print(f"Started pid {p.pid}")
    time.sleep(5)
    p.terminate()
    p.wait()

if os.path.exists("debug_run.log"):
    print("LOG CONTENT BEGIN")
    with open("debug_run.log") as f:
        print(f.read())
    print("LOG CONTENT END")
else:
    print("NO LOG CREATED")
