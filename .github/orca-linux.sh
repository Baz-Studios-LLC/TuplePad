#!/bin/bash
# A real screen reader reading TuplePad: Orca runs on a virtual display and logs everything it
# says while TuplePad is used with real key presses. Fails unless Orca speaks the editor's name,
# the line the caret is on, and the new line after moving down. Run inside a D-Bus session:
#
#   dbus-run-session -- .github/orca-linux.sh core/target/debug/tuplepad-app
set -uo pipefail

APP="$(realpath "$1")"
WORK="${RUNNER_TEMP:-/tmp}/orca-check"
rm -rf "$WORK"
mkdir -p "$WORK"
printf '# Notes\n\nSecond line here\n' > "$WORK/notes.md"

# A silent sound device: the speech server waits for audio to finish playing, and CI has none.
pulseaudio --start --exit-idle-time=-1 > /dev/null 2>&1 || true
pactl load-module module-null-sink sink_name=silent > /dev/null 2>&1 || true
pactl set-default-sink silent > /dev/null 2>&1 || true

Xvfb :99 -screen 0 1600x1000x24 &
export DISPLAY=:99
sleep 2
# Accessibility on, as it is with a screen reader running.
for KEY in IsEnabled ScreenReaderEnabled; do
    gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.freedesktop.DBus.Properties.Set org.a11y.Status "$KEY" "<true>" >/dev/null
done

orca --replace --debug-file="$WORK/orca.txt" > "$WORK/orca-out.txt" 2>&1 &
ORCA=$!
sleep 8

# The real app (not a scripted run), with a data folder of its own.
TUPLEPAD_DATA_DIR="$WORK/data" "$APP" "$WORK/notes.md" > "$WORK/app.txt" 2>&1 &
APP_PID=$!
sleep 8
WINDOW="$(xdotool search --pid "$APP_PID" | tail -1)"
xdotool windowfocus --sync "$WINDOW" || true
sleep 3
xdotool key --window "$WINDOW" Down
sleep 2
xdotool key --window "$WINDOW" Down
sleep 2
xdotool key --window "$WINDOW" End
sleep 2

# Where Orca is right now (if it's waiting on something, this shows what).
sudo env "PATH=$PATH" py-spy dump --pid "$ORCA" > "$WORK/orca-stack.txt" 2>&1 || true
echo "--- Orca's stack"; grep -v "^$" "$WORK/orca-stack.txt" | head -60

# What AT-SPI says about the editor (its states decide whether Orca follows it).
/usr/bin/python3 - <<'PY'
import pyatspi
desktop = pyatspi.Registry.getDesktop(0)
def find(node, depth=0):
    if node is None or depth > 40:
        return None
    if node.name == "Editor, notes.md":
        return node
    for i in range(node.childCount):
        found = find(node.getChildAtIndex(i), depth + 1)
        if found:
            return found
for app in (desktop.getChildAtIndex(i) for i in range(desktop.childCount)):
    if app is not None and "tuplepad" in (app.name or "").lower():
        editor = find(app)
        if editor:
            print("editor role:", editor.getRoleName())
            print("editor states:", [pyatspi.stateToString(s) for s in editor.getState().getStates()])
            text = editor.queryText()
            print("editor text:", repr(text.getText(0, text.characterCount)), "caret:", text.caretOffset)
            # How screen readers ask for the line, word and character at the caret.
            for name in ["TEXT_BOUNDARY_LINE_START", "TEXT_BOUNDARY_WORD_START", "TEXT_BOUNDARY_CHAR"]:
                try:
                    print(name, repr(text.getTextAtOffset(text.caretOffset - 1, getattr(pyatspi, name))))
                except Exception as error:
                    print(name, "failed:", error)
            try:
                from gi.repository import Atspi
                for granularity in ["LINE", "WORD", "CHAR"]:
                    print("getStringAtOffset", granularity, repr(Atspi.Text.get_string_at_offset(editor, text.caretOffset - 1, getattr(Atspi.TextGranularity, granularity))))
            except Exception as error:
                print("getStringAtOffset failed:", error)
            print("extents of first char:", text.getCharacterExtents(0, pyatspi.DESKTOP_COORDS))
PY
# Orca writes its log in chunks: let it quit properly so the end isn't lost, then close TuplePad.
orca --quit > /dev/null 2>&1 || kill -INT "$ORCA" 2>/dev/null
for _ in $(seq 1 20); do kill -0 "$ORCA" 2>/dev/null || break; sleep 0.5; done
kill "$ORCA" "$APP_PID" 2>/dev/null
sleep 1
SPOKEN="$(grep -a "SPEECH OUTPUT" "$WORK/orca.txt" || true)"
echo "What Orca said:"
echo "$SPOKEN" | tail -40
echo "--- What Orca heard from TuplePad"
grep -a -E "object:state-changed:focused|window:activate|caret-moved|text-changed|LOCUS OF FOCUS|focus_manager|Setting locus" "$WORK/orca.txt" | grep -v -i "orca\b.*frame" | tail -40
if [ -z "$SPOKEN" ]; then
    echo "--- Orca's output"; tail -40 "$WORK/orca-out.txt"
    echo "--- Orca's debug log"; tail -60 "$WORK/orca.txt"
fi
FAILED=0
for EXPECTED in "Editor, notes.md" "# Notes" "Second line here"; do
    if ! grep -qF "$EXPECTED" <<< "$SPOKEN"; then
        echo "Orca never said: $EXPECTED"
        FAILED=1
    fi
done
exit $FAILED
