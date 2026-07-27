<#
  Qué tardó el último arranque, y en qué se fue el tiempo.

      rice-boot-report.ps1        últimos arranques + desglose del actual

  NECESITA ADMIN: el canal Diagnostics-Performance no es legible como usuario.
  También necesita el servicio DPS activo -- es quien escribe estos eventos, y
  el install.ps1 del rice lo desactiva, así que si no aparece nada nuevo esa es
  la razón antes que cualquier otra.

  El evento tarda un par de minutos en escribirse tras iniciar sesión: si acabas
  de arrancar y no ves la entrada de hoy, espera y vuelve a lanzarlo.
#>

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
        ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Hay que ejecutarlo como administrador.' -ForegroundColor Red
    exit 1
}

$dps = Get-Service DPS -ErrorAction SilentlyContinue
Write-Host ("DPS (quien mide): {0}/{1}`n" -f $dps.StartType, $dps.Status)

Write-Host '=== arranques registrados ==='
try {
    Get-WinEvent -LogName 'Microsoft-Windows-Diagnostics-Performance/Operational' -MaxEvents 200 -EA Stop |
      Where-Object { $_.Id -eq 100 } | Select-Object -First 8 | ForEach-Object {
        $x = [xml]$_.ToXml(); $d = @{}
        $x.Event.EventData.Data | ForEach-Object { $d[$_.Name] = $_.'#text' }
        Write-Host ("  {0}   total {1,6:N1}s   nucleo {2,6:N1}s   post-arranque {3,6:N1}s" -f `
            $_.TimeCreated.ToString('dd/MM HH:mm'),
            ($d['BootTime']/1000), ($d['MainPathBootTime']/1000), ($d['BootPostBootTime']/1000))
      }
} catch { Write-Host ("  no legible: " + $_.Exception.Message) }

# Los eventos 101..110 nombran QUÉ tardó: aplicación, servicio, driver, etc.
Write-Host "`n=== lo que mas retraso el ultimo arranque ==="
try {
    $since = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
    Get-WinEvent -LogName 'Microsoft-Windows-Diagnostics-Performance/Operational' -MaxEvents 400 -EA Stop |
      Where-Object { $_.Id -in 101,102,103,106,109,110 -and $_.TimeCreated -gt $since } |
      ForEach-Object {
        $x = [xml]$_.ToXml(); $d = @{}
        $x.Event.EventData.Data | ForEach-Object { $d[$_.Name] = $_.'#text' }
        $ms = $d['TotalTime']; if (-not $ms) { $ms = $d['Time'] }
        [pscustomobject]@{
            Tipo = switch ($_.Id) {
                101 {'aplicacion'} 102 {'driver'} 103 {'servicio (degradado)'}
                106 {'tarea en segundo plano'} 109 {'dispositivo'} 110 {'servicio'}
            }
            Nombre = $d['Name']
            Segundos = if ($ms) { [math]::Round($ms/1000,1) } else { $null }
        }
      } | Where-Object Segundos | Sort-Object Segundos -Descending | Select-Object -First 20 |
      Format-Table -AutoSize | Out-String | Write-Host
} catch { Write-Host ("  no legible: " + $_.Exception.Message) }
