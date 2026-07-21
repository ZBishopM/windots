# Autostart the working layout at login. The apps are homed to their workspaces
# by GlazeWM window_rules; the two terminals (both wezterm, so no process rule)
# are placed by focusing their workspace right before launching.

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

# --- apps (window_rules route these to their workspaces) ---
Start-Process 'shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude'                                   # -> ws1
Start-Process 'C:\Users\obisp\AppData\Local\Programs\Zed\Zed.exe'                              # -> ws1
Start-Process 'C:\Program Files\Firefox Developer Edition\firefox.exe'                         # -> ws3
Start-Process 'C:\Users\obisp\scoop\shims\vesktop.exe'                                         # -> ws4
Start-Process 'C:\Program Files\Google\Chrome\Application\chrome.exe' -ArgumentList '--profile-directory=Profile 5'  # BubbleTea -> ws6

# --- terminals (focus workspace, then launch so the new window lands there) ---
Focus 2
Start-Process $wez -ArgumentList 'start', '--', 'pwsh', '-NoExit', '-Command', 'claude'
Start-Sleep -Seconds 3
Focus 5
Start-Process $wez -ArgumentList 'start', '--', 'pwsh', '-NoExit', '-Command', 'btop'
Start-Sleep -Seconds 2

Focus 1  # end on the primary workspace
