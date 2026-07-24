# Checks once at login whether GlazeWM's animation PR #1392 has merged, and
# toasts the moment it flips open -> merged so the real, official workspace-slide
# can replace the custom interim one. Fail-silent (offline = skip). A flag file
# makes it a one-shot: it never nags again once it has fired.
$flag = "$env:USERPROFILE\.config\.glaze-anim-merged"
if (Test-Path $flag) { return }

try {
    $pr = Invoke-RestMethod -Uri 'https://api.github.com/repos/glzr-io/glazewm/pulls/1392' `
        -Headers @{ 'User-Agent' = 'rice-animcheck' } -TimeoutSec 8
} catch { return }   # no network / rate-limited -> try again next login

if ($pr.merged -eq $true) {
    New-Item -ItemType File -Force $flag | Out-Null
    @{ icon = 'check'; title = 'GlazeWM #1392 mergeado'; body = 'Animaciones oficiales listas'; accent = '#a9b56a' } |
        ConvertTo-Json -Compress | Set-Content "$env:USERPROFILE\.config\island.json" -Encoding utf8
    $notify = "$env:USERPROFILE\dev\target\release\shadowplay-notify.exe"
    if (Test-Path $notify) {
        Start-Process $notify -ArgumentList "--title `"GlazeWM #1392 mergeado`" --body `"Animaciones oficiales - actualiza GlazeWM y activa el slide`" --icon check --accent `"#a9b56a`" --hold 12"
    }
}
