# Autostart the working layout ONCE at login. No persistent window_rules -- each
# app is placed only now, by focusing its workspace and waiting for its window to
# appear before moving on (so it lands there). Open apps later go wherever you are.

Start-Sleep -Seconds 12  # let GlazeWM + dwindle + the bars settle after login

$wez = 'C:\Program Files\WezTerm\wezterm-gui.exe'

function Focus($n) {
    try {
        $s = New-Object System.Net.WebSockets.ClientWebSocket
        $ct = [System.Threading.CancellationToken]::None
        if ($s.ConnectAsync([Uri]'ws://localhost:6123', $ct).Wait(3000)) {
            $b = [Text.Encoding]::UTF8.GetBytes("command focus --workspace $n")
            [void]$s.SendAsync((New-Object System.ArraySegment[byte] (, $b)), 'Text', $true, $ct).Wait(2000)
            Start-Sleep -Milliseconds 400
        }
        $s.Dispose()
    } catch {}
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
