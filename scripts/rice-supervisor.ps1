# Keeps the rice's always-on processes alive. Every 30s it checks each component
# and relaunches any that died (crash, manual kill, GlazeWM restart killing its
# child dwindle, etc.) so a dead piece self-heals within a minute instead of
# staying dead until the next login. Single-instance; trims its own RAM.

$mutex = New-Object System.Threading.Mutex($false, 'Global\rice-supervisor')
if (-not $mutex.WaitOne(0)) { exit }   # another supervisor already running

$cfg    = 'C:\Users\obisp\.config'
$scoop  = 'C:\Users\obisp\scoop\apps'
$ahk    = "$scoop\autohotkey\current\v2\AutoHotkey64.exe"
$bar    = 'C:\Users\obisp\dev\target\release\glaze-bar.exe'

# Give things a moment at login so we don't race the normal autostart.
Start-Sleep -Seconds 20

Add-Type -Namespace W -Name K -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("psapi.dll")] public static extern bool EmptyWorkingSet(System.IntPtr h);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern System.IntPtr GetCurrentProcess();
'@ -EA SilentlyContinue

function Alive($name)      { [bool](Get-Process $name -EA SilentlyContinue) }
function AliveCount($name) { (Get-Process $name -EA SilentlyContinue | Measure-Object).Count }
function AliveCmd($match)  { [bool](Get-CimInstance Win32_Process -Filter "Name='pwsh.exe'" -EA SilentlyContinue | Where-Object { $_.CommandLine -like $match }) }

while ($true) {
    if (-not (Alive 'GlazeWM'))      { Start-Process "$scoop\glazewm\current\GlazeWM.exe" }
    if (-not (Alive 'AltSnap'))      { Start-Process "$scoop\altsnap\current\AltSnap.exe" }
    if (-not (Alive 'AutoHotkey64')) { Start-Process $ahk -ArgumentList "`"$cfg\wezterm-hotkey.ahk`"" }

    # dwindle: fibonacci layout (child of GlazeWM, dies on GlazeWM restart)
    if (-not (AliveCmd '*glazewm-dwindle*')) {
        Start-Process pwsh -ArgumentList '-NoProfile','-WindowStyle','Hidden','-File',"$cfg\glazewm-dwindle.ps1" -WindowStyle Hidden
    }
    # ShadowPlay WGC recorder (only launch if none -> never duplicates)
    if (-not (Alive 'shadowplay-wgc')) {
        Start-Process wscript.exe -ArgumentList "`"$cfg\shadowplay-wgc.vbs`""
    }
    # glaze-bar: one per monitor. If both died, relaunch both with their offsets.
    if ((AliveCount 'glaze-bar') -eq 0 -and (Test-Path $bar)) {
        Start-Process $bar -ArgumentList '--x','0','--width','1920'
        Start-Process $bar -ArgumentList '--x','1920','--width','2560'
    }

    try { [W.K]::EmptyWorkingSet([W.K]::GetCurrentProcess()) | Out-Null } catch {}
    Start-Sleep -Seconds 30
}
