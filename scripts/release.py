#!/usr/bin/env python3
"""Makes a TuplePad release, on a Mac that has the update-signing key.

    scripts/release.py <notes.md>              # builds and signs everything into dist/
    scripts/release.py <notes.md> --publish    # and publishes it as a GitHub release

The version (and the release's tag) is VERSION. The notes say what's new, in Markdown; the
release page adds how to install. Steps:

1. The Windows installer and the Linux AppImage come from the Build workflow's run for this
   commit, which must have passed (push first, and let it finish).
2. The Mac app is built here for Apple silicon and Intel: a disk image to download, and a zip
   the updater installs from.
3. The updater's downloads are signed with the key in ~/.config/tuplepad-release/
   update-signing.key (TUPLEPAD_SIGNING_KEY to use another), which must match the public key
   built into TuplePad (src/update.rs). latest.json lists them; SHA256SUMS.txt covers all.
4. --publish creates the release as a draft, uploads everything, then makes it the latest.
"""

import datetime
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = "Baz-Studios-LLC/TuplePad"
DIST = os.path.join(ROOT, "dist")
KEY = os.environ.get("TUPLEPAD_SIGNING_KEY", os.path.expanduser("~/.config/tuplepad-release/update-signing.key"))


def run(*command, capture=True, **options):
    result = subprocess.run(command, capture_output=capture, text=True, **options)
    if result.returncode != 0:
        sys.exit(f"failed: {' '.join(command)}\n{(result.stderr or '')[-2000:] if capture else ''}")
    return (result.stdout or "").strip() if capture else ""


def git(*args):
    return run("git", "-C", ROOT, *args)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    publish = "--publish" in sys.argv
    if len(args) != 1:
        sys.exit(__doc__)
    notes = open(args[0]).read().strip()
    version = open(os.path.join(ROOT, "VERSION")).read().strip()
    commit = git("rev-parse", "HEAD")

    # The release is exactly what's committed and pushed.
    if git("status", "--porcelain", "--untracked-files=no"):
        sys.exit("Commit or set aside your changes first.")
    git("fetch", "-q", "origin")
    if publish and git("rev-parse", "origin/main") != commit:
        sys.exit("Push main first: the release is made from the commit on GitHub.")
    if subprocess.run(["gh", "release", "view", version, "--repo", REPO], capture_output=True).returncode == 0:
        sys.exit(f"{version} is already released. Raise VERSION first.")

    # The signing key must be the one TuplePad trusts.
    run("cargo", "build", "-q", "--manifest-path", os.path.join(ROOT, "core/Cargo.toml"), "-p", "tuplepad-app", "--example", "update_sign")
    sign_tool = os.path.join(ROOT, "core/target/debug/examples/update_sign")
    built_in = re.search(r'const PUBLIC_KEY: &str = "([^"]+)"', open(os.path.join(ROOT, "core/tuplepad-app/src/update.rs")).read()).group(1)
    if run(sign_tool, "public", KEY) != built_in:
        sys.exit(f"{KEY} isn't the key TuplePad trusts (PUBLIC_KEY in src/update.rs).")

    shutil.rmtree(DIST, ignore_errors=True)
    os.makedirs(DIST)

    # Windows and Linux, from CI.
    runs = json.loads(run("gh", "run", "list", "--repo", REPO, "--workflow", "build.yml", "--commit", commit, "--json", "databaseId,status,conclusion", "--limit", "1"))
    if not runs or runs[0]["status"] != "completed" or runs[0]["conclusion"] != "success":
        sys.exit(f"The Build workflow hasn't passed for {commit[:7]} yet: {runs[0] if runs else 'no run'}.")
    ci = os.path.join(DIST, "ci")
    run("gh", "run", "download", str(runs[0]["databaseId"]), "--repo", REPO, "-n", "windows-x64", "-n", "linux-x64", "-D", ci)
    for folder, _, files in os.walk(ci):
        for name in files:
            if name.startswith(f"TuplePad-{version}-"):
                shutil.move(os.path.join(folder, name), os.path.join(DIST, name))
    shutil.rmtree(ci)

    # The Mac app.
    print("Building the Mac app (Apple silicon and Intel)…", flush=True)
    run(os.path.join(ROOT, "scripts/bundle-mac.sh"), "release", "universal", capture=False, stdout=subprocess.DEVNULL)
    app = os.path.join(ROOT, "build/TuplePad.app")
    run("ditto", "-c", "-k", "--keepParent", app, os.path.join(DIST, f"TuplePad-{version}-mac.zip"))
    stage = os.path.join(ROOT, "build/dmg")
    shutil.rmtree(stage, ignore_errors=True)
    os.makedirs(stage)
    run("ditto", app, os.path.join(stage, "TuplePad.app"))
    os.symlink("/Applications", os.path.join(stage, "Applications"))
    run("hdiutil", "create", "-volname", "TuplePad", "-srcfolder", stage, "-ov", "-format", "UDZO", "-fs", "HFS+", os.path.join(DIST, f"TuplePad-{version}-mac.dmg"))
    shutil.rmtree(stage)

    # What the updater installs, signed for each system.
    updater = {"mac": f"TuplePad-{version}-mac.zip", "windows-x64": f"TuplePad-{version}-windows-x64-setup.exe", "linux-x64": f"TuplePad-{version}-linux-x64.AppImage"}
    downloads = {}
    for system, name in updater.items():
        path = os.path.join(DIST, name)
        if not os.path.exists(path):
            sys.exit(f"{name} is missing.")
        signature = run(sign_tool, "sign", KEY, path, f"TuplePad {version} {system}") + "\n"
        # Checked against the built-in key before anything is published.
        signature_file = os.path.join(ROOT, "build", f"{name}.minisig")
        with open(signature_file, "w") as f:
            f.write(signature)
        if run(sign_tool, "verify", built_in, path, signature_file) != f"TuplePad {version} {system}":
            sys.exit(f"{name}'s signature didn't check out.")
        os.remove(signature_file)
        downloads[system] = {
            "url": f"https://github.com/{REPO}/releases/download/{version}/{name}",
            "size": os.path.getsize(path),
            "signature": signature,
        }
    manifest = {"version": version, "date": datetime.date.today().isoformat(), "notes": notes,
                "page": f"https://github.com/{REPO}/releases/tag/{version}", "downloads": downloads}
    with open(os.path.join(DIST, "latest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
    files = sorted(n for n in os.listdir(DIST) if not n.startswith("."))
    with open(os.path.join(DIST, "SHA256SUMS.txt"), "w") as f:
        for name in files:
            f.write(f"{hashlib.sha256(open(os.path.join(DIST, name), 'rb').read()).hexdigest()}  {name}\n")
    files.append("SHA256SUMS.txt")

    body = notes + "\n\n" + install_notes(version)
    with open(os.path.join(DIST, ".release-notes.md"), "w") as f:
        f.write(body)
    print("\n".join(f"dist/{n}  ({os.path.getsize(os.path.join(DIST, n)) / 1e6:.1f} MB)" for n in files))
    if not publish:
        print("\nNot published (add --publish).")
        return
    paths = [os.path.join(DIST, n) for n in files]
    run("gh", "release", "create", version, "--repo", REPO, "--target", commit, "--title", f"TuplePad {version}", "--notes-file", os.path.join(DIST, ".release-notes.md"), "--draft", "--latest", *paths)
    run("gh", "release", "edit", version, "--repo", REPO, "--draft=false", "--latest")
    print(f"\nPublished: https://github.com/{REPO}/releases/tag/{version}")


def install_notes(version):
    return f"""## Install

**Mac** (macOS 11 or later, Apple silicon or Intel): open `TuplePad-{version}-mac.dmg` and drag TuplePad to Applications. TuplePad isn't notarized by Apple yet, so the first time you open it macOS says it can't check it. Open System Settings ▸ Privacy & Security, scroll down, and click **Open Anyway**.

**Windows** (10 and 11, 64-bit): run `TuplePad-{version}-windows-x64-setup.exe`. It installs just for you, with no administrator password. Until TuplePad's installer is signed, Windows may say it protected your PC: click **More info**, then **Run anyway**.

**Linux** (64-bit): download `TuplePad-{version}-linux-x64.AppImage`, make it executable (`chmod +x TuplePad-{version}-linux-x64.AppImage`) and open it. The `.tar.gz` has the same app as plain files.

From then on, TuplePad tells you when there's a new version and updates itself. `SHA256SUMS.txt` lists every file's checksum."""


if __name__ == "__main__":
    main()
