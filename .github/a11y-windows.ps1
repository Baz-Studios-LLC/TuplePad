# What Narrator gets from TuplePad: its UI Automation tree, read with Windows' own UIA client.
# Fails if a control has no name, or the editor doesn't expose its document's line.
#
#   powershell -File .github/a11y-windows.ps1 core\target\release\tuplepad-app.exe
param([string]$App)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$work = Join-Path $env:RUNNER_TEMP "a11y"
New-Item -ItemType Directory -Force $work | Out-Null
$doc = Join-Path $work "notes.md"
[IO.File]::WriteAllText($doc, "# Notes`n`nSecond line here`n")
# The app waits at the signal until this script has read its tree.
$signal = Join-Path $work "go"
$steps = Join-Path $work "steps.json"
ConvertTo-Json @(@{ wait = 1500 }, @{ signal = @{ path = $signal; timeout = 120 } }, @{ quit = $true }) -Depth 4 | Set-Content $steps
$process = Start-Process (Resolve-Path $App) -ArgumentList "--script", $steps, "--session-dir", (Join-Path $work "session"), $doc -PassThru

function Read-Tree {
    $condition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ProcessIdProperty, $process.Id)
    $window = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children, $condition)
    if (-not $window) { return @() }
    foreach ($element in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
        $value = $null
        $pattern = $null
        if ($element.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) { $value = $pattern.Current.Value }
        [pscustomobject]@{ type = $element.Current.ControlType.ProgrammaticName -replace "ControlType\.", ""; name = $element.Current.Name; value = $value }
    }
}

# The first request switches the tree on; it fills in a moment later.
$deadline = (Get-Date).AddSeconds(60)
$tree = @()
while ((Get-Date) -lt $deadline) {
    Start-Sleep 2
    $tree = @(Read-Tree)
    if ($tree.Count -gt 10) { break }
}
New-Item -ItemType File "$signal.done" -Force | Out-Null
$tree | Format-Table -AutoSize | Out-String -Width 200 | Write-Output

$controls = "Button", "CheckBox", "RadioButton", "ComboBox", "Edit", "Spinner", "Slider", "TabItem", "ListItem"
$unnamed = @($tree | Where-Object { $controls -contains $_.type -and [string]::IsNullOrWhiteSpace($_.name) })
$editor = $tree | Where-Object { $_.type -eq "Edit" -and $_.name -eq "Editor, notes.md" } | Select-Object -First 1
if ($tree.Count -le 10) { throw "Narrator would get almost nothing: $($tree.Count) elements" }
if ($unnamed.Count -gt 0) { throw "Controls without a name: $($unnamed | Out-String)" }
if (-not $editor) { throw "No editor named 'Editor, notes.md'" }
if ($editor.value -ne "# Notes") { throw "The editor exposes '$($editor.value)', not its line '# Notes'" }
"Narrator gets $($tree.Count) elements; every control is named, and the editor reads its line."
