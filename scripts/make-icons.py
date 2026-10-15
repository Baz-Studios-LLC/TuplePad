#!/usr/bin/env python3
"""Renders the app icon (packaging/mac/AppIcon.icon) for Windows and Linux, whose builds run
where Icon Composer doesn't: packaging/windows/TuplePad.ico and packaging/linux/tuplepad.png.
Run it on a Mac with Xcode after changing the icon, and commit what it writes.

    scripts/make-icons.py
"""

import os
import struct
import subprocess
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICTOOL = "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def render(work):
    big = os.path.join(work, "icon-1024.png")
    subprocess.run([ICTOOL, os.path.join(ROOT, "packaging/mac/AppIcon.icon"), "--export-image", "--output-file", big,
                    "--platform", "macOS", "--rendition", "Default", "--width", "1024", "--height", "1024", "--scale", "1"],
                   check=True, capture_output=True)
    return big


def resized(source, size, work):
    out = os.path.join(work, f"icon-{size}.png")
    subprocess.run(["sips", "-z", str(size), str(size), source, "--out", out], check=True, capture_output=True)
    return out


def write_ico(pngs, path):
    """An .ico whose images are PNGs (Windows Vista and later read these)."""
    images = [open(p, "rb").read() for _, p in pngs]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries = b""
    for (size, _), data in zip(pngs, images):
        side = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    with open(path, "wb") as f:
        f.write(header + entries + b"".join(images))


def main():
    with tempfile.TemporaryDirectory() as work:
        big = render(work)
        os.makedirs(os.path.join(ROOT, "packaging/windows"), exist_ok=True)
        write_ico([(size, resized(big, size, work)) for size in ICO_SIZES], os.path.join(ROOT, "packaging/windows/TuplePad.ico"))
        subprocess.run(["sips", "-z", "512", "512", big, "--out", os.path.join(ROOT, "packaging/linux/tuplepad.png")], check=True, capture_output=True)
    print("packaging/windows/TuplePad.ico\npackaging/linux/tuplepad.png")


if __name__ == "__main__":
    main()
