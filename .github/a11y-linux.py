#!/usr/bin/env python3
"""What Orca gets from TuplePad: its AT-SPI tree on the accessibility bus, read with pyatspi.
Fails if a control has no name, or the editor doesn't expose its document's line. Run inside a
D-Bus session (CI: dbus-run-session -- python3 .github/a11y-linux.py <tuplepad-app>).
"""

import json
import os
import subprocess
import sys
import tempfile
import time

APP = os.path.abspath(sys.argv[1])
work = tempfile.mkdtemp(prefix="tuplepad-atspi-")
doc = os.path.join(work, "notes.md")
with open(doc, "w") as f:
    f.write("# Notes\n\nSecond line here\n")
signal = os.path.join(work, "go")
with open(os.path.join(work, "steps.json"), "w") as f:
    json.dump([{"wait": 1500}, {"signal": {"path": signal, "timeout": 120}}, {"quit": True}], f)

# Accessibility on, as a screen reader turns it on: the bus starts, and apps see IsEnabled.
subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus", "--object-path", "/org/a11y/bus", "--method",
                "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", "IsEnabled", "<true>"], check=True)
subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus", "--object-path", "/org/a11y/bus", "--method",
                "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", "ScreenReaderEnabled", "<true>"], check=True)
app = subprocess.Popen(["xvfb-run", "-a", "-s", "-screen 0 1600x1000x24", APP, "--script", os.path.join(work, "steps.json"),
                        "--session-dir", os.path.join(work, "session"), doc])

import pyatspi  # noqa: E402 (after the bus is up)


def walk(node, depth=0, out=None):
    out = [] if out is None else out
    if node is None or depth > 60:
        return out
    try:
        role, name = node.getRoleName(), node.name or ""
        text = None
        try:
            t = node.queryText()
            text = t.getText(0, t.characterCount)
        except NotImplementedError:
            pass
        out.append({"role": role, "name": name, "text": text})
        for i in range(node.childCount):
            walk(node.getChildAtIndex(i), depth + 1, out)
    except Exception as error:  # an element that went away mid-walk
        out.append({"role": "error", "name": str(error), "text": None})
    return out


tree = []
deadline = time.time() + 60
while time.time() < deadline:
    time.sleep(2)
    desktop = pyatspi.Registry.getDesktop(0)
    apps = [desktop.getChildAtIndex(i) for i in range(desktop.childCount)]
    ours = [a for a in apps if a is not None and "tuplepad" in (a.name or "").lower()]
    if not ours:
        print("apps on the bus:", [a.name for a in apps if a is not None])
        continue
    tree = walk(ours[0])
    if len(tree) > 10:
        break
open(signal + ".done", "w").close()
app.wait(timeout=120)

for e in tree:
    print(f"{e['role']:<18} {e['name'][:50]:<50} {(e['text'] or '')[:40]}")
controls = {"push button", "check box", "radio button", "combo box", "text", "entry", "spin button", "slider", "page tab", "toggle button"}
unnamed = [e for e in tree if e["role"] in controls and not e["name"].strip() and e["role"] not in ("text",)]
editor = next((e for e in tree if e["name"] == "Editor, notes.md"), None)
problems = []
if len(tree) <= 10:
    problems.append(f"Orca would get almost nothing: {len(tree)} elements")
if unnamed:
    problems.append(f"controls without a name: {unnamed}")
if not editor:
    problems.append("no editor named 'Editor, notes.md'")
elif not (editor["text"] or "").startswith("# Notes"):
    problems.append(f"the editor exposes {editor['text']!r}, not its text starting '# Notes'")
if problems:
    sys.exit("\n".join(problems))
print(f"Orca gets {len(tree)} elements; every control is named, and the editor reads its line.")
