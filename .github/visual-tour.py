#!/usr/bin/env python3
"""CI's visual pass: opens a small sample project in the release build and saves every main
screen, in light and dark, as PNGs (Slint's own snapshot, so no window frame or native menus).

    python .github/visual-tour.py <tuplepad-app binary> <output folder>
"""

import json
import os
import subprocess
import sys
import tempfile

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
for name, text in [("src/main.rs", MAIN), ("README.md", README), ("trails.json", DATA)]:
    with open(os.path.join(demo, name), "w", newline="\n") as f:
        f.write(text)
main_rs, readme = os.path.join(demo, "src", "main.rs"), os.path.join(demo, "README.md")

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
        {"updates": "check"}, {"waitUpdates": True}, pause, snap("updates"), {"updates": "later"},
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
result = subprocess.run(command, timeout=600, capture_output=True, text=True)
shots = sorted(n for n in os.listdir(OUT) if n.endswith(".png"))
print(f"{len(shots)} screens:", ", ".join(shots))
if result.returncode != 0 or len(shots) < 30:
    print(result.stderr[-3000:])
    sys.exit("The visual tour didn't finish.")
