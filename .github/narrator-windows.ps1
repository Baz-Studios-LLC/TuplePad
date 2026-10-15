# Real Narrator reads TuplePad: Narrator runs, TuplePad opens a note, Down moves the caret, and
# Narrator's own "copy last spoken phrase" command (Narrator key + Ctrl + X) shows what it said.
# Fails unless Narrator names the editor and reads the line the caret moves to.
#
#   powershell -File .github/narrator-windows.ps1 core\target\debug\tuplepad-app.exe
param([string]$App)
$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Input {
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
}
"@

function Press([byte[]]$keys) {
    foreach ($key in $keys) { [Input]::keybd_event($key, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40 }
    [array]::Reverse($keys)
    foreach ($key in $keys) { [Input]::keybd_event($key, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40 }
}
$Insert = 0x2D; $Control = 0x11; $X = 0x58; $Down = 0x28

# What Narrator said last, by its copy-last-phrase command.
function Last-Phrase {
    Set-Clipboard -Value " "
    Press @($Insert, $Control, $X)
    Start-Sleep -Milliseconds 800
    (Get-Clipboard -Raw | Out-String).Trim()
}

$work = Join-Path $env:RUNNER_TEMP "narrator"
New-Item -ItemType Directory -Force $work | Out-Null
$note = Join-Path $work "notes.md"
[IO.File]::WriteAllText($note, "# Notes`n`nSecond line here`n")
$env:TUPLEPAD_DATA_DIR = Join-Path $work "data"

$narrator = Start-Process "$env:WINDIR\System32\Narrator.exe" -PassThru
Start-Sleep 12
$tuplepad = Start-Process (Resolve-Path $App) -ArgumentList $note -PassThru
Start-Sleep 10
# Narrator may open its welcome window; TuplePad comes back to the front.
$tuplepad.Refresh()
[Input]::SetForegroundWindow($tuplepad.MainWindowHandle) | Out-Null
Start-Sleep 4

$phrases = @()
$phrases += Last-Phrase
Press @($Down); Start-Sleep 3
$phrases += Last-Phrase
Press @($Down); Start-Sleep 3
$phrases += Last-Phrase

Stop-Process -Id $tuplepad.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Name Narrator -Force -ErrorAction SilentlyContinue

"What Narrator said:"
$phrases | ForEach-Object { "  $_" }
$said = $phrases -join " | "
$missing = @("Editor", "Second line here") | Where-Object { -not $said.Contains($_) }
if ($missing) { throw "Narrator never said: $($missing -join ', ')" }
"Narrator names the editor and reads the line the caret moves to."
