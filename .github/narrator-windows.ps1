# Real Narrator reads TuplePad: Narrator runs, TuplePad opens a note, the arrow keys move the
# caret, and Narrator's own "copy last spoken phrase" command (Narrator key + Ctrl + X) shows what
# it said. Fails unless Narrator names the editor and reads the line the caret moves to.
#
#   powershell -File .github/narrator-windows.ps1 core\target\debug\tuplepad-app.exe
param([string]$App)
$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Collections.Generic;
using System.Text;
using System.Runtime.InteropServices;
public static class Input {
    public delegate bool WindowVisitor(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] public static extern bool EnumWindows(WindowVisitor visit, IntPtr data);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
    public static string Title(IntPtr window) {
        var text = new StringBuilder(256);
        GetWindowText(window, text, 256);
        return text.ToString();
    }
    // Minimizes Narrator's own windows (its home, tutorial and "updates" windows take the focus).
    public static List<string> MinimizeNarrator() {
        var titles = new List<string>();
        EnumWindows((window, data) => {
            var title = Title(window);
            if (IsWindowVisible(window) && title.Contains("Narrator")) { ShowWindow(window, 6); titles.Add(title); }
            return true;
        }, IntPtr.Zero);
        return titles;
    }
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int x, int y, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect rect);
    public struct Rect { public int Left, Top, Right, Bottom; }
    public static string ForegroundTitle() {
        var text = new StringBuilder(256);
        GetWindowText(GetForegroundWindow(), text, 256);
        return text.ToString();
    }
}
"@

function Press([byte[]]$keys) {
    foreach ($key in $keys) { [Input]::keybd_event($key, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40 }
    [array]::Reverse($keys)
    foreach ($key in $keys) { [Input]::keybd_event($key, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40 }
}
$Insert = 0x2D; $Control = 0x11; $Alt = 0x12; $X = 0x58; $Down = 0x28; $HomeKey = 0x24; $Tab = 0x09

# What Narrator said last, by its copy-last-phrase command.
function Last-Phrase {
    Set-Clipboard -Value " "
    Press @($Insert, $Control, $X)
    Start-Sleep -Milliseconds 800
    (Get-Clipboard -Raw | Out-String).Trim()
}

# Narrator starts without its welcome window and tutorial, which would take the focus.
foreach ($key in "HKCU:\Software\Microsoft\Narrator\NarratorHome", "HKCU:\Software\Microsoft\Narrator\QuickStart") {
    New-Item -Force $key | Out-Null
}
Set-ItemProperty "HKCU:\Software\Microsoft\Narrator\NarratorHome" AutoStart 0 -Type DWord
Set-ItemProperty "HKCU:\Software\Microsoft\Narrator\NarratorHome" MinimizeType 1 -Type DWord
Set-ItemProperty "HKCU:\Software\Microsoft\Narrator\QuickStart" SkipQuickStart 1 -Type DWord

$work = Join-Path $env:RUNNER_TEMP "narrator"
New-Item -ItemType Directory -Force $work | Out-Null
$note = Join-Path $work "notes.md"
[IO.File]::WriteAllText($note, "# Notes`n`nSecond line here`n")
$env:TUPLEPAD_DATA_DIR = Join-Path $work "data"

Start-Process "$env:WINDIR\System32\Narrator.exe" | Out-Null
Start-Sleep 12
$tuplepad = Start-Process (Resolve-Path $App) -ArgumentList $note -PassThru
Start-Sleep 10
$tuplepad.Refresh()
$window = $tuplepad.MainWindowHandle
# Windows lets a background process bring a window forward after an Alt press; a click in the
# editor then gives it the keyboard focus for sure.
foreach ($attempt in 1..5) {
    $minimized = [Input]::MinimizeNarrator()
    if ($minimized.Count) { "Minimized: $($minimized -join ', ')" }
    Press @($Alt)
    [Input]::SetForegroundWindow($window) | Out-Null
    Start-Sleep 4
    if ([Input]::ForegroundTitle() -like "notes.md*") { break }
}
# Coming forward, Narrator names what has the focus.
$phrases = @()
$phrases += Last-Phrase
$rect = New-Object Input+Rect
[Input]::GetWindowRect($window, [ref]$rect) | Out-Null
[Input]::SetCursorPos([int](($rect.Left + $rect.Right) / 2), [int](($rect.Top + $rect.Bottom) / 2)) | Out-Null
[Input]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60; [Input]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep 4
"In front: $([Input]::ForegroundTitle())"

$phrases += Last-Phrase
Press @($Control, $HomeKey); Start-Sleep 3
$phrases += Last-Phrase
Press @($Down); Start-Sleep 3
$phrases += Last-Phrase
Press @($Down); Start-Sleep 3
$phrases += Last-Phrase
# Narrator + Tab reads the item with the focus again.
Press @($Insert, $Tab); Start-Sleep 4
$phrases += Last-Phrase

Stop-Process -Id $tuplepad.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Name Narrator -Force -ErrorAction SilentlyContinue

"What Narrator said:"
$phrases | ForEach-Object { "  $_" }
$said = $phrases -join " | "
$missing = @("Editor", "Second line here") | Where-Object { -not $said.Contains($_) }
if ($missing) { throw "Narrator never said: $($missing -join ', ')" }
"Narrator names the editor and reads the line the caret moves to."
