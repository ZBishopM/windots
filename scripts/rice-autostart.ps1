# Autostart the working layout ONCE at login. No persistent window_rules -- each
# app is placed only now, by focusing its workspace and waiting for its window to
# appear before moving on (so it lands there). Open apps later go wherever you are.

Start-Sleep -Seconds 12  # let GlazeWM + dwindle + the bars settle after login

$wez = 'C:\Program Files\WezTerm\wezterm-gui.exe'

# The IPC client lives in lib/rice-ipc.ps1 now. This script's own copy was the
# last one still using 'localhost', paired with the shortest connect budget in
# the tree -- so it usually lost the race to the ~2.1s IPv6 timeout, the catch{}
# swallowed it, and the next app opened on whatever workspace happened to be
# focused. That was the "my apps came up on the wrong workspace" symptom.
. "$env:USERPROFILE\.config\lib\rice-paths.ps1"
. "$env:USERPROFILE\.config\lib\rice-ipc.ps1"

function Focus($n) {
    if (-not (Set-GlazeWorkspace -Index $n)) { Write-Host "Focus ${n}: no IPC" }
}

# Launch then block until the app's window exists (login is a clean slate, so the
# process isn't already running), so focus stays on the target ws until it lands.
function WaitWin($proc, $sec = 15) {
    $end = (Get-Date).AddSeconds($sec)
    while (-not (Get-Process $proc -EA SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 }) -and (Get-Date) -lt $end) {
        Start-Sleep -Milliseconds 400
    }
    Start-Sleep -Milliseconds 700  # settle in the workspace
}

# ws1: Claude (app) + Zed
Focus 1
Start-Process 'shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude'; WaitWin 'Claude'
Start-Process 'C:\Users\obisp\AppData\Local\Programs\Zed\Zed.exe'; WaitWin 'Zed'

# ws3: Firefox
Focus 3
Start-Process 'C:\Program Files\Firefox Developer Edition\firefox.exe'; WaitWin 'firefox'

# ws4: Vesktop
Focus 4
Start-Process 'C:\Users\obisp\scoop\shims\vesktop.exe'; WaitWin 'vesktop'

# ws6: Chrome (BubbleTea profile)
Focus 6
Start-Process 'C:\Program Files\Google\Chrome\Application\chrome.exe' -ArgumentList '--profile-directory=Profile 5'; WaitWin 'chrome'

# ws2 + ws5: terminals (wezterm windows appear fast; fixed settle is enough)
Focus 2
Start-Process $wez -ArgumentList 'start', '--', 'pwsh', '-NoExit', '-Command', 'claude'
Start-Sleep -Seconds 3
Focus 5
Start-Process $wez -ArgumentList 'start', '--', 'pwsh', '-NoExit', '-Command', 'btop'
Start-Sleep -Seconds 3

Focus 1  # end on the primary workspace

# Hide the Windows taskbar. Explorer restores it on every login, so this has to
# run each time; Win+Shift+B toggles it back when something needs the tray.
Start-Process (Join-Path $Rice.Dev 'taskbar.exe') -ArgumentList '--hide' -WindowStyle Hidden

# Once at login: has GlazeWM's animation PR #1392 merged yet? Toast if so (fail-silent).
Start-Process pwsh -ArgumentList '-NoProfile', '-WindowStyle', 'Hidden', '-File', "$env:USERPROFILE\.config\glazewm-animcheck.ps1" -WindowStyle Hidden
