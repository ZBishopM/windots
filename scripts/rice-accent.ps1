# Align Windows' own accent colour with the rice palette (rice-common::theme).
#
# Windows paints title bars, focus rings, selection highlights and Explorer
# chrome from this accent, so leaving it on the stock blue-grey made every
# system surface clash with the warm bar/island/toasts.
#
#   rice-accent.ps1            apply the warm accent
#   rice-accent.ps1 -Restore   put back whatever was there before
#
# The previous values are saved next to this script the first time it runs, so
# restoring is exact rather than a guess at the Windows default.

param([switch]$Restore)

$dwm     = 'HKCU:\Software\Microsoft\Windows\DWM'
$accent  = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Accent'
$backup  = "$env:USERPROFILE\.config\.accent-backup.json"

# rice-common::theme ACCENT (#e0a35c), as a ramp dark -> light. Index 5 is the
# main accent; Windows picks lighter entries for text on accent backgrounds, so
# the top of the ramp has to stay legible.
$ramp = @(
    '3a2a18', '5c4326', '8a6438', 'b3824a',
    'cc9352', 'e0a35c', 'ebb87e', 'f5cfa5'
)
$mainIndex = 5

# Registry DWORDs are unsigned, but PowerShell's [int] is signed: 0xFF... does
# not fit and Set-ItemProperty throws. Reinterpret the bits instead of casting.
function As-Dword([int64]$v) {
    return [BitConverter]::ToInt32([BitConverter]::GetBytes([uint32]($v -band 0xFFFFFFFF)), 0)
}

function To-Abgr([string]$hex) {
    # Registry accent values are 0xAABBGGRR, i.e. byte-reversed RGB.
    $r = [Convert]::ToInt32($hex.Substring(0, 2), 16)
    $g = [Convert]::ToInt32($hex.Substring(2, 2), 16)
    $b = [Convert]::ToInt32($hex.Substring(4, 2), 16)
    return [int64](0xFF000000L -bor ($b -shl 16) -bor ($g -shl 8) -bor $r)
}
function To-Argb([string]$hex, [int]$alpha = 0xC4) {
    $r = [Convert]::ToInt32($hex.Substring(0, 2), 16)
    $g = [Convert]::ToInt32($hex.Substring(2, 2), 16)
    $b = [Convert]::ToInt32($hex.Substring(4, 2), 16)
    return [int64](([int64]$alpha -shl 24) -bor ($r -shl 16) -bor ($g -shl 8) -bor $b)
}

# Tell every running app the colours changed, so they repaint without a logoff.
Add-Type -Namespace R -Name Acc -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet=System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint msg, System.IntPtr wParam,
    string lParam, uint flags, uint timeout, out System.IntPtr result);
'@ -EA SilentlyContinue
function Broadcast {
    $HWND_BROADCAST = [IntPtr]0xffff
    $WM_SETTINGCHANGE = 0x001A
    $out = [IntPtr]::Zero
    foreach ($topic in 'ImmersiveColorSet', 'WindowsThemeElement') {
        [void][R.Acc]::SendMessageTimeout($HWND_BROADCAST, $WM_SETTINGCHANGE, [IntPtr]::Zero, $topic, 2, 1000, [ref]$out)
    }
}

if ($Restore) {
    if (-not (Test-Path $backup)) { Write-Host 'no backup to restore from'; exit 1 }
    $b = Get-Content $backup -Raw | ConvertFrom-Json
    Set-ItemProperty $dwm -Name AccentColor -Value (As-Dword ([int64]$b.AccentColor)) -Type DWord
    Set-ItemProperty $dwm -Name ColorizationColor -Value (As-Dword ([int64]$b.ColorizationColor)) -Type DWord
    Set-ItemProperty $dwm -Name ColorizationAfterglow -Value (As-Dword ([int64]$b.ColorizationAfterglow)) -Type DWord
    Set-ItemProperty $accent -Name AccentColorMenu -Value (As-Dword ([int64]$b.AccentColorMenu)) -Type DWord
    Set-ItemProperty $accent -Name StartColorMenu -Value (As-Dword ([int64]$b.StartColorMenu)) -Type DWord
    Set-ItemProperty $accent -Name AccentPalette -Value ([byte[]]$b.AccentPalette) -Type Binary
    Broadcast
    Write-Host 'accent restored'
    exit 0
}

# Save the originals once, so -Restore is exact.
if (-not (Test-Path $backup)) {
    $d = Get-ItemProperty $dwm
    $a = Get-ItemProperty $accent
    @{
        AccentColor           = [uint32]$d.AccentColor
        ColorizationColor     = [uint32]$d.ColorizationColor
        ColorizationAfterglow = [uint32]$d.ColorizationAfterglow
        AccentColorMenu       = [uint32]$a.AccentColorMenu
        StartColorMenu        = [uint32]$a.StartColorMenu
        AccentPalette         = @($a.AccentPalette)
    } | ConvertTo-Json -Compress | Set-Content $backup -Encoding utf8
    Write-Host "saved previous accent -> $backup"
}

$main = $ramp[$mainIndex]
Set-ItemProperty $dwm    -Name AccentColor           -Value (As-Dword (To-Abgr $main)) -Type DWord
Set-ItemProperty $dwm    -Name ColorizationColor     -Value (As-Dword (To-Argb $main)) -Type DWord
Set-ItemProperty $dwm    -Name ColorizationAfterglow -Value (As-Dword (To-Argb $main)) -Type DWord
# Accent on title bars (and not just the taskbar), so focused windows read warm.
Set-ItemProperty $dwm    -Name ColorPrevalence       -Value 1 -Type DWord
Set-ItemProperty $accent -Name AccentColorMenu       -Value (As-Dword (To-Abgr $main)) -Type DWord
Set-ItemProperty $accent -Name StartColorMenu        -Value (As-Dword (To-Abgr $ramp[3])) -Type DWord

# AccentPalette: 8 entries x 4 bytes (B,G,R,A), darkest first.
$bytes = New-Object System.Collections.Generic.List[byte]
foreach ($h in $ramp) {
    $bytes.Add([Convert]::ToByte($h.Substring(4, 2), 16))  # B
    $bytes.Add([Convert]::ToByte($h.Substring(2, 2), 16))  # G
    $bytes.Add([Convert]::ToByte($h.Substring(0, 2), 16))  # R
    $bytes.Add(0xFF)
}
Set-ItemProperty $accent -Name AccentPalette -Value ($bytes.ToArray()) -Type Binary
Set-ItemProperty $accent -Name AccentColorIndex -Value $mainIndex -Type DWord -EA SilentlyContinue

Broadcast
Write-Host "accent set to #$main (rice amber)"
