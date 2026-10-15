#!/usr/bin/env python3
"""CI's visual pass: opens a small sample project in the release build and saves every main
screen, in light and dark, as PNGs (Slint's own snapshot, so no window frame or native menus).

    python .github/visual-tour.py <tuplepad-app binary> <output folder>
"""

import functools
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

APP, OUT = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
os.makedirs(OUT, exist_ok=True)
work = tempfile.mkdtemp(prefix="tuplepad-tour-")
demo = os.path.join(work, "trail-planner")
os.makedirs(os.path.join(demo, "src"))

MAIN = """//! Plans a day hike: which trail fits the daylight left.

use std::time::Duration;

/// A trail and what it asks of you.
pub struct Trail {
    pub name: String,
    pub distance_km: f32,
    pub climb_m: f32,
}

impl Trail {
    /// Walking time with Naismith's rule: 5 km an hour, plus an hour for every 600 m of climb.
    pub fn walking_time(&self) -> Duration {
        let hours = self.distance_km / 5.0 + self.climb_m / 600.0;
        Duration::from_secs_f32(hours * 3600.0)
    }
}

fn main() {
    let trails = vec![
        Trail { name: "Heather Meadows".into(), distance_km: 4.5, climb_m: 210.0 },
        Trail { name: "Ptarmigan Ridge".into(), distance_km: 14.5, climb_m: 670.0 },
    ];
    for trail in &trails {
        println!("{}: {:?}", trail.name, trail.walking_time());
    }
}
"""
README = """# Trail planner

Picks a **day hike** that fits the daylight you have left.

## Trails

- Heather Meadows: 4.5 km, 210 m of climb
- Ptarmigan Ridge: 14.5 km, 670 m of climb

```rust
let hours = distance_km / 5.0 + climb_m / 600.0;
```

| Trail | Distance | Climb |
|---|---|---|
| Heather Meadows | 4.5 km | 210 m |
| Ptarmigan Ridge | 14.5 km | 670 m |
"""
DATA = json.dumps({"trails": [{"name": "Heather Meadows", "distance_km": 4.5, "climb_m": 210}, {"name": "Ptarmigan Ridge", "distance_km": 14.5, "climb_m": 670}]}, indent=2) + "\n"
XML = """<trails>
  <trail name="Heather Meadows" km="4.5"/>
  <trail name="Ptarmigan Ridge" km="14.5"/>
</trails>
"""
CSV = "name,km,climb\nHeather Meadows,4.5,210\nPtarmigan Ridge,14.5,670\nBlue Lake,7.6,300\n"
for name, text in [("src/main.rs", MAIN), ("README.md", README), ("trails.json", DATA), ("trails.xml", XML), ("trails.csv", CSV)]:
    with open(os.path.join(demo, name), "w", newline="\n") as f:
        f.write(text)
with open(os.path.join(demo, "logo.bin"), "wb") as f:
    f.write(bytes(range(256)) * 4)
main_rs, readme = os.path.join(demo, "src", "main.rs"), os.path.join(demo, "README.md")
xml, csv, binary = os.path.join(demo, "trails.xml"), os.path.join(demo, "trails.csv"), os.path.join(demo, "logo.bin")
signals = os.path.join(work, "signals")
os.makedirs(signals)

# A stand-in for GitHub's releases, offering a newer TuplePad (it's only shown, never installed).
served = os.path.join(work, "served")
os.makedirs(served)
download = {"url": "http://127.0.0.1/none.zip", "size": 1, "signature": "unused"}
with open(os.path.join(served, "latest.json"), "w") as f:
    json.dump({"version": "v999", "notes": "## What's new\n\n- Find in Files is faster, and stopping a search inside a huge file takes effect at once.\n- A calmer sidebar\n\n## Fixed\n\n- Pasted text keeps its indentation.", "downloads": {s: download for s in ["mac", "windows-x64", "linux-x64"]}}, f)


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=served))
threading.Thread(target=server.serve_forever, daemon=True).start()

steps = [{"resize": [1280, 800]}, {"folder": demo}]
for look in ["light", "dark"]:
    snap = lambda name: {"snapshot": os.path.join(OUT, f"{look}-{name}.png")}
    pause = {"wait": 700}
    steps += [
        {"set": {"appearance": look}}, {"theme": look}, {"open": main_rs}, {"goto": 14},
        {"command": "view.outline"}, pause, snap("editor"), {"command": "view.outline"},
        {"find": {"query": "trail"}}, pause, snap("find"), {"find": {"action": "close"}},
        {"search": {"scope": "folder", "folder": demo, "query": "trail"}}, {"search": {"action": "start"}}, {"waitSearch": True}, pause,
        snap("find-in-files"), {"search": {"action": "close"}},
        {"command": "view.palette"}, {"type": "split"}, pause, snap("palette"), {"key": "escape"},
        {"command": "help.shortcuts"}, pause, snap("shortcuts"), {"shortcuts": None},
        {"open": readme}, {"command": "tools.markdown-preview"}, {"wait": 1000}, snap("markdown-preview"), {"command": "tools.markdown-preview"},
        {"open": main_rs}, {"goto": 21}, {"key": "end"}, {"type": " // the easy one"},
        {"command": "compare.saved"}, pause, snap("compare"), {"toolView": {"action": "close"}}, {"command": "file.revert"},
        {"command": "tools.hex"}, pause, snap("hex"), {"toolView": {"action": "close"}},
        {"split": "side"}, pause, snap("split"), {"split": "off"},
        {"command": "text.column"}, pause, snap("column-editor"), {"column": {"action": "cancel"}},
        {"command": "tools.hashes"}, pause, snap("hashes"), {"sheet": {"action": "cancel"}},
        {"updates": "check"}, {"waitUpdates": True}, pause, snap("update-available"), {"updates": "later"},
        {"command": "view.sidebar"}, pause, snap("sidebar-closed"), {"command": "view.sidebar"}, pause,
        {"find": {"query": "trail", "replacement": "path", "replaceBar": True}}, pause, snap("find-replace"), {"find": {"action": "close"}},
        {"goto": 28}, {"type": "    tra"}, {"wait": 900}, snap("completion"), {"key": "escape"}, {"command": "file.revert"},
        {"gutterClick": 3}, {"gutterClick": 6}, {"foldClick": 11}, pause, snap("bookmarks-folds"), {"command": "bookmark.clear"}, {"foldClick": 11},
        {"command": "file.copy-path"}, {"wait": 300}, snap("toast"),
        {"search": {"scope": "folder", "folder": demo, "query": "trail", "replace": True, "replacement": "path"}}, {"search": {"action": "start"}}, {"waitSearch": True},
        {"search": {"action": "prepareReplace"}}, {"wait": 1000}, snap("replace-plan"), {"search": {"action": "cancelPlan"}}, {"search": {"action": "close"}},
        {"command": "file.quick-open"}, {"type": "tra"}, pause, snap("go-to-file"), {"key": "escape"},
        {"open": xml}, {"command": "xml.xpath"}, {"sheet": {"xpath": "//trail/@name", "action": "xpath"}}, pause, snap("xpath"), {"sheet": {"action": "cancel"}},
        {"open": csv}, {"command": "csv.sort"}, pause, snap("csv-sort"), {"sheet": {"action": "cancel"}},
        {"command": "tools.shell"}, {"sheet": {"shell": "sort"}}, pause, snap("shell"), {"sheet": {"action": "cancel"}},
        {"command": "file.print"}, pause, snap("print"), {"key": "escape"},
        {"open": binary}, pause, snap("binary-file"), {"command": "file.close"},
        {"open": readme}, {"type": "!"}, {"signal": os.path.join(signals, f"disk-{look}")}, {"checkDisk": True}, pause, snap("disk-changed"), {"command": "file.revert"},
    ]
    for tab, name in enumerate(["general", "editor", "appearance", "snippets", "ai"]):
        steps += [{"settings": tab}, pause, snap(f"settings-{name}")]
    steps += [{"closeSettings": True}]
steps += [{"quit": True}]

script = os.path.join(work, "steps.json")
with open(script, "w") as f:
    json.dump(steps, f)
command = [APP, "--script", script, "--session-dir", os.path.join(work, "session"), main_rs, readme]
if sys.platform.startswith("linux"):
    command = ["xvfb-run", "-a", "-s", "-screen 0 1600x1000x24"] + command
env = {**os.environ, "TUPLEPAD_UPDATE_URL": f"http://127.0.0.1:{server.server_address[1]}/latest.json"}
app = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, env=env)
# At each signal, README.md changes on disk under the edited document.
started = time.time()
while app.poll() is None and time.time() - started < 900:
    for name in os.listdir(signals):
        path = os.path.join(signals, name)
        if name.endswith(".done") or os.path.exists(path + ".done"):
            continue
        with open(readme, "a") as f:
            f.write("\nChanged by another app.\n")
        open(path + ".done", "w").close()
    time.sleep(0.1)
if app.poll() is None:
    app.kill()
stderr = app.stderr.read()
server.shutdown()
shots = sorted(n for n in os.listdir(OUT) if n.endswith(".png"))
print(f"{len(shots)} screens:", ", ".join(shots))
if app.returncode != 0 or len(shots) < 60:
    print(stderr[-3000:])
    sys.exit("The visual tour didn't finish.")
