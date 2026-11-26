# End-to-end smoke test for the Windows build. Runs on a real desktop session
# (GitHub's windows runners have one) against target\release\lalia.exe:
#
#   CLI → daemon start → status over the named pipe → pause/resume with a real
#   F11 key press through the keyboard hook → dictation attempt with F12 →
#   settings window → typing into Notepad → quit.
#
# Screenshots land in smoke\ for a human (or Claude) to look at.
param([string]$Exe = "target\release\lalia.exe")

$ErrorActionPreference = "Stop"
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force smoke | Out-Null
$failures = @()

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Native {
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, UIntPtr wParam, IntPtr lParam);
}
"@
[Native]::SetProcessDPIAware() | Out-Null

function Shot($name) {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save("smoke\$name.png")
    $g.Dispose(); $bmp.Dispose()
}
function Key($vk, $holdMs = 120) {
    [Native]::keybd_event($vk, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds $holdMs
    [Native]::keybd_event($vk, 0, 2, [UIntPtr]::Zero)
}
# Lalia is a GUI program: piping makes PowerShell wait for it and capture stdout.
function Lalia { (& $Exe @args | Out-String).Trim() }
function Status { Lalia status | ConvertFrom-Json }
function Check($what, $ok) {
    if ($ok) { Write-Host "PASS  $what" }
    else { Write-Host "FAIL  $what"; $script:failures += $what }
}

$VK_F11 = 0x7A; $VK_F12 = 0x7B; $VK_CONTROL = 0x11; $VK_ESCAPE = 0x1B; $VK_RETURN = 0x0D
$VK_LWIN = 0x5B; $VK_B = 0x42
$WM_TRAY = 0x8001; $WM_LBUTTONUP = 0x0202; $WM_RBUTTONUP = 0x0205

# --- CLI ---------------------------------------------------------------
$version = Lalia version
Check "version prints ($version)" ($version -match '^lalia \d+\.\d+\.\d+$')
$config = Lalia config
Check "config lives in AppData" ($config -match 'AppData\\Roaming\\lalia\\config\.toml')
"AIzaFAKE-smoke-test-key" | & $Exe set-key | Out-Null
Check "set-key stores the key" (Test-Path "$env:APPDATA\lalia\key")
Check "status fails cleanly when not running" (-not (Lalia status))

# --- daemon ------------------------------------------------------------
# Launch it the way the Start menu does (through Explorer, with no terminal).
$viaExplorer = $true
Start-Process explorer.exe -ArgumentList "`"$Exe`""
Start-Sleep 8
if (-not (Lalia status)) {
    Write-Host "WARN  launching through Explorer did not work here; starting directly"
    $viaExplorer = $false
    Start-Process $Exe
    Start-Sleep 6
}
$s = Status
Check "daemon answers on the named pipe (phase $($s.phase))" ($s.phase -eq "idle")
Shot "1-running"

Lalia pause | Out-Null
Start-Sleep -Milliseconds 400
Shot "2-paused-overlay"
Check "pause over the pipe" ((Status).phase -eq "paused")
Lalia resume | Out-Null
Check "resume over the pipe" ((Status).phase -eq "idle")
Start-Sleep 2

# --- hotkeys through the keyboard hook -----------------------------------
Key $VK_F11
Start-Sleep -Milliseconds 500
Check "F11 pauses (keyboard hook)" ((Status).phase -eq "paused")
Start-Sleep 2
Key $VK_F11
Start-Sleep -Milliseconds 500
Check "F11 resumes (keyboard hook)" ((Status).phase -eq "idle")
Start-Sleep 2

# F12 starts dictation. The runner has no microphone (or the fake key is
# rejected), so the expected outcome is a clear error, not a hang.
[Native]::keybd_event($VK_F12, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 700
Shot "3-f12-held"
$during = Status
[Native]::keybd_event($VK_F12, 0, 2, [UIntPtr]::Zero)
Start-Sleep 6
$after = Status
Write-Host "      during F12: phase=$($during.phase) error=$($during.error)"
Write-Host "      after  F12: phase=$($after.phase) error=$($after.error)"
Check "F12 reaches the daemon (hook) and ends in a clear state" `
    (($during.phase -ne "idle") -or $during.error -or $after.error)
Check "daemon is idle again after the attempt" ($after.phase -eq "idle")

# --- tray icon -------------------------------------------------------------
$tray = [Native]::FindWindowW("LaliaTray", $null)
Check "tray window exists" ($tray -ne [IntPtr]::Zero)
try {
    # Win+B focuses the notification area's overflow arrow; Enter opens it.
    [Native]::keybd_event($VK_LWIN, 0, 0, [UIntPtr]::Zero); Key $VK_B 60
    [Native]::keybd_event($VK_LWIN, 0, 2, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 800; Key $VK_RETURN 60; Start-Sleep 2
    Shot "3b-tray-overflow"
    Key $VK_ESCAPE 60; Start-Sleep 1
    # A right-click on the icon, as the shell reports it: the menu opens.
    [Native]::PostMessageW($tray, $WM_TRAY, [UIntPtr]::Zero, [IntPtr]$WM_RBUTTONUP) | Out-Null
    Start-Sleep 2
    Shot "3c-tray-menu"
    Key $VK_ESCAPE 60; Start-Sleep 1
    Check "daemon still answers after the tray menu" ((Status).phase -eq "idle")
} catch {
    Write-Host "WARN  tray screenshots could not be taken: $_"
}

# --- settings window -----------------------------------------------------
# A left-click on the tray icon opens it.
[Native]::PostMessageW($tray, $WM_TRAY, [UIntPtr]::Zero, [IntPtr]$WM_LBUTTONUP) | Out-Null
Start-Sleep 10
Shot "4-settings-home"
$gui = Get-Process lalia | Where-Object { $_.MainWindowTitle -eq "Lalia" }
Check "settings window opens" ($null -ne $gui)
Lalia gui dictation | Out-Null   # second request switches page in the same window
Start-Sleep 3
Shot "5-settings-dictation"
Check "still one settings window" (@(Get-Process lalia | Where-Object { $_.MainWindowTitle -eq "Lalia" }).Count -eq 1)
if ($gui) { $gui | ForEach-Object { $_.CloseMainWindow() | Out-Null } }
Start-Sleep 2

# --- typing into another app ---------------------------------------------
try {
    $notepad = Start-Process notepad -PassThru
    Start-Sleep 3
    [Native]::SetForegroundWindow($notepad.MainWindowHandle) | Out-Null
    Start-Sleep 1
    $text = "Hello from Lalia: caf" + [char]0xE9 + " na" + [char]0xEF + "ve " + [char]0x03BB + [char]0x03B1 + [char]0x03BB + [char]0x03B9 + [char]0x03AC + " 123!"
    & $Exe type-text $text 300 | Out-Null
    Start-Sleep 1
    Shot "6-typed-in-notepad"
    Set-Clipboard -Value "empty"
    foreach ($letter in 0x41, 0x43) {          # Ctrl+A, Ctrl+C
        [Native]::keybd_event($VK_CONTROL, 0, 0, [UIntPtr]::Zero)
        Key $letter 60
        [Native]::keybd_event($VK_CONTROL, 0, 2, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 300
    }
    Start-Sleep 1
    $typed = (Get-Clipboard -Raw)
    if ($typed) { $typed = $typed.Trim() }
    Write-Host "      typed: $typed"
    Check "Unicode text is typed into Notepad" ($typed -eq $text)
    $notepad | Stop-Process -Force
} catch {
    Write-Host "WARN  typing test could not run: $_"
}

# --- log file --------------------------------------------------------------
$log = "$env:LOCALAPPDATA\lalia\lalia.log"
if ($viaExplorer) {
    Check "started without a terminal, the daemon writes a log file" ((Test-Path $log) -and ((Get-Content $log -Raw) -match "lalia \d"))
}
if (Test-Path $log) { Write-Host "      --- lalia.log ---"; Get-Content $log -Tail 12 | ForEach-Object { Write-Host "      $_" } }

# --- quit ----------------------------------------------------------------
$bye = Lalia quit
Start-Sleep 2
Check "quit replies and the process exits ($bye)" (($bye -eq "bye") -and -not (Get-Process lalia -ErrorAction SilentlyContinue))

if ($failures.Count) {
    Write-Host "`n$($failures.Count) check(s) failed:"; $failures | ForEach-Object { Write-Host "  - $_" }
    exit 1
}
Write-Host "`nAll smoke checks passed."
