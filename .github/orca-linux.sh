#!/bin/bash
# A real screen reader reading TuplePad: Orca runs on a virtual display and logs everything it
# says while TuplePad is used with real key presses. Fails unless Orca speaks the editor's name,
# the line the caret is on, and the new line after moving down. Run inside a D-Bus session:
#
#   dbus-run-session -- .github/orca-linux.sh core/target/debug/tuplepad-app
set -uo pipefail

APP="$(realpath "$1")"
WORK="$(mktemp -d)"
printf '# Notes\n\nSecond line here\n' > "$WORK/notes.md"

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

kill "$APP_PID" "$ORCA" 2>/dev/null
sleep 1
SPOKEN="$(grep -a "SPEECH OUTPUT" "$WORK/orca.txt" || true)"
echo "What Orca said:"
echo "$SPOKEN" | tail -40
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
