#!/usr/bin/env python3
"""TUI smoke harness: runs `kmttt` in a pseudo terminal, sends keys, and renders the screen
with pyte so the layout can be checked without a real terminal.

usage: tui_harness.py <kmttt-binary> <script.json> [cols rows]
script: [["wait", 1.5], ["keys", "echo hi\r"], ["dump"], ["ctrl", "b"], ["keys", "v"], ...]
"""
import json
import os
import pty
import select
import sys
import time

import pyte


def main():
    exe, script_path = sys.argv[1], sys.argv[2]
    cols = int(sys.argv[3]) if len(sys.argv) > 3 else 140
    rows = int(sys.argv[4]) if len(sys.argv) > 4 else 40
    script = json.load(open(script_path))
    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.environ["COLUMNS"], os.environ["LINES"] = str(cols), str(rows)
        os.execv(exe, [exe] + os.environ.get("KMTTT_ARGS", "").split())
    import fcntl
    import struct
    import termios

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def pump(secs):
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if fd in r:
                try:
                    data = os.read(fd, 65536)
                except OSError:
                    return False
                if not data:
                    return False
                # crossterm asks for cursor position at start; answer it.
                if b"\x1b[6n" in data:
                    os.write(fd, b"\x1b[1;1R")
                stream.feed(data)
        return True

    for step in script:
        op = step[0]
        if op == "wait":
            pump(step[1])
        elif op == "keys":
            os.write(fd, step[1].encode())
            pump(0.4)
        elif op == "ctrl":
            os.write(fd, bytes([ord(step[1]) - ord("a") + 1]))
            pump(0.2)
        elif op == "dump":
            print("=" * cols)
            for line in screen.display:
                print(line.rstrip())
            print("=" * cols)
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass


if __name__ == "__main__":
    main()
