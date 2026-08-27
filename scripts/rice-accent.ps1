# Align Windows' own accent colour with the rice palette (rice-common::theme).
#
# Windows paints title bars, focus rings, selection highlights and Explorer
# chrome from this accent, so leaving it on the stock blue-grey made every
# system surface clash with the warm bar/island/toasts.
#
#   rice-accent.ps1            apply the warm accent
#   rice-accent.ps1 -Restore   put back whatever was there before
#
# El deshacer lo lleva lib\rice-undo.ps1. Antes habia un .accent-backup.json a
# mano que guardaba SEIS valores... y el script escribia OCHO: ColorPrevalence y
# AccentColorIndex se quedaban puestos para siempre despues de -Restore. Ese es
# justo el fallo que desaparece cuando la funcion que escribe es la que apunta.

param([switch]$Restore)

. "$env:USERPROFILE\.config\lib\rice-undo.ps1"

$dwm     = 'HKCU:\Software\Microsoft\Windows\DWM'
$accent  = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Accent'
$Ambito  = 'accent'
$viejo   = "$env:USERPROFILE\.config\.accent-backup.json"

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
    Undo-Rice $Ambito
    Broadcast
    if (Test-Path $viejo) {
        Write-Host "nota: queda un $viejo del sistema antiguo de respaldo; ya no se usa."
    }
    exit 0
}

$main = $ramp[$mainIndex]

# AccentPalette: 8 entries x 4 bytes (B,G,R,A), darkest first.
$bytes = New-Object System.Collections.Generic.List[byte]
foreach ($h in $ramp) {
    $bytes.Add([Convert]::ToByte($h.Substring(4, 2), 16))  # B
    $bytes.Add([Convert]::ToByte($h.Substring(2, 2), 16))  # G
    $bytes.Add([Convert]::ToByte($h.Substring(0, 2), 16))  # R
    $bytes.Add(0xFF)
}

Start-RiceUndo $Ambito
try {
    Set-RegValueTracked $dwm    'AccentColor'           (As-Dword (To-Abgr $main))     -Kind DWord
    Set-RegValueTracked $dwm    'ColorizationColor'     (As-Dword (To-Argb $main))     -Kind DWord
    Set-RegValueTracked $dwm    'ColorizationAfterglow' (As-Dword (To-Argb $main))     -Kind DWord
    # Accent on title bars (and not just the taskbar), so focused windows read warm.
    Set-RegValueTracked $dwm    'ColorPrevalence'       1                              -Kind DWord
    Set-RegValueTracked $accent 'AccentColorMenu'       (As-Dword (To-Abgr $main))     -Kind DWord
    Set-RegValueTracked $accent 'StartColorMenu'        (As-Dword (To-Abgr $ramp[3]))  -Kind DWord
    Set-RegValueTracked $accent 'AccentPalette'         ($bytes.ToArray())             -Kind Binary
    Set-RegValueTracked $accent 'AccentColorIndex'      $mainIndex                     -Kind DWord
} finally { Save-RiceUndo }

Broadcast
Write-Host "accent set to #$main (rice amber)"
Write-Host "-Restore lo devuelve todo, incluidos ColorPrevalence y AccentColorIndex."
