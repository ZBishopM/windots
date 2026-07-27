<#
  Turn Windows' own notification banners off per app, so notifyd's toast is the
  only thing drawn.

    rice-notif-banners.ps1            hide banners for the apps below
    rice-notif-banners.ps1 -Restore   give them back
    rice-notif-banners.ps1 -List      show every registered app and its state

  Per app rather than Do Not Disturb, deliberately. DND would suppress
  everything globally, and with notifyd drawing the replacements that means a
  dead notifyd leaves you seeing nothing at all. Clearing ShowBanner only stops
  Windows PAINTING the banner: the notification still lands in the Notification
  Center, which is both where notifyd reads it from and a fallback you can still
  open with Win+N if notifyd is not running.

  All of this is under HKCU, so no elevation.
#>
param([switch]$Restore, [switch]$List)

$Root = 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Notifications\Settings'

# Apps whose banners notifyd replaces. Matched as substrings against the
# registered identifiers, because those are AUMIDs and rarely guessable in full
# (Vesktop is 'dev.vencord.vesktop', Discord 'com.squirrel.Discord.Discord').
$Apps = @(
    'Parsec'
    'vesktop'
    'Discord'
    'NVIDIA'
    'HyperX'
    'Claude'
    'OneDrive'
    'firefox'
    'Brave'
    'Chrome'
    'Steam'
    'Epic'
    'Slack'
    'Teams'
    'WhatsApp'
)

if ($List) {
    Get-ChildItem $Root -ErrorAction SilentlyContinue | ForEach-Object {
        $v = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
        $b = if ($null -eq $v.ShowBanner) { 'default(on)' } elseif ($v.ShowBanner -eq 0) { 'OFF' } else { 'on' }
        '{0,-12} {1}' -f $b, $_.PSChildName
    }
    return
}

$hit = 0
foreach ($k in Get-ChildItem $Root -ErrorAction SilentlyContinue) {
    $name = $k.PSChildName
    if (-not ($Apps | Where-Object { $name -like "*$_*" })) { continue }
    $hit++
    if ($Restore) {
        # Remove the value rather than setting it to 1: absent means "Windows
        # default", which is what it was before we touched anything.
        Remove-ItemProperty $k.PSPath -Name ShowBanner -ErrorAction SilentlyContinue
        Write-Host "  restored $name"
    } else {
        New-ItemProperty $k.PSPath -Name ShowBanner -PropertyType DWord -Value 0 -Force | Out-Null
        Write-Host "  banner off  $name"
    }
}
Write-Host ""
Write-Host "$hit app(s) $(if ($Restore) { 'restored' } else { 'silenced' })."
Write-Host "Notifications still reach the Notification Center (Win+N); only the banner is gone."
