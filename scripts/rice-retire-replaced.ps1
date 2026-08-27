<#
  Apaga las aplicaciones cuya función ya hace el rice.

      rice-retire-replaced.ps1            para y quita del autoarranque
      rice-retire-replaced.ps1 -Restore   las devuelve al autoarranque
      rice-retire-replaced.ps1 -Uninstall las desinstala de verdad

  Sin admin. Desactivar no borra nada: si algo del rice falla, -Restore las trae
  de vuelta en un segundo. Desinstalar es la opción final y por eso está separada.

  Qué sustituye a qué:
    EarTrumpet   -> volumen por app en la isla (+ el crate appvol)
    Twinkle Tray -> brillo de monitores en la isla (DDC/CI, dxva2.dll)
    AudioSwitch  -> cambio de altavoz en la página de dispositivos de la isla
    Pomatez      -> el temporizador de la isla
#>
param([switch]$Restore, [switch]$Uninstall)

. "$env:USERPROFILE\.config\lib\rice-undo.ps1"
$Ambito = 'retire-replaced'

if ($Restore) {
    Undo-Rice $Ambito
    Write-Host "`nListo. Vuelve a lanzarlas a mano si quieres que arranquen ya."
    return
}

# --- procesos -------------------------------------------------------------
$procs = 'EarTrumpet', 'Twinkle Tray', 'TwinkleTray', 'AudioSwitch', 'Pomatez'
if (-not $Restore) {
    Write-Host '=== parando ==='
    foreach ($p in $procs) {
        $found = Get-Process $p -ErrorAction SilentlyContinue
        if ($found) {
            $found | Stop-Process -Force -ErrorAction SilentlyContinue
            Write-Host ("  parado   {0}" -f $p)
        }
    }
}

# --- autoarranque clásico (HKCU Run) --------------------------------------
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$bak = "$env:USERPROFILE\.config\.retired-run.json"

Start-RiceUndo $Ambito
try {
    Write-Host "`n=== autoarranque clasico ==="
    $p = Get-ItemProperty $run
    foreach ($n in (Get-Item $run).Property) {
        if ($n -match 'pomatez|eartrumpet|twinkle|audioswitch' -or "$($p.$n)" -match 'pomatez|eartrumpet|twinkle|audioswitch') {
            if (Remove-RegValueTracked $run $n) { Write-Host ("  quitado  {0}" -f $n) }
        }
    }

    # --- autoarranque de apps de la Store ---------------------------------
    # Las apps empaquetadas no usan la clave Run: su interruptor es un DWORD
    # `State` bajo AppModel\SystemAppData\<familia>\<tarea>.
    #
    # El -Restore de antes escribia un 2 FIJO ("habilitada por el usuario") sin
    # mirar que valia. Si la app ya estaba desactivada antes de que el rice la
    # tocara, "restaurar" te la activaba. Ahora se apunta el valor real.
    Write-Host "`n=== autoarranque de apps de la Store ==="
    $base = 'HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData'
    foreach ($fam in Get-ChildItem $base -ErrorAction SilentlyContinue |
                     Where-Object { $_.PSChildName -match 'EarTrumpet|TwinkleTray' }) {
        foreach ($task in Get-ChildItem $fam.PSPath -ErrorAction SilentlyContinue) {
            $cur = (Get-ItemProperty $task.PSPath -ErrorAction SilentlyContinue).State
            if ($null -eq $cur) { continue }   # sin State no es una tarea de inicio
            $ruta = "HKCU:\" + ($task.Name -replace '^HKEY_CURRENT_USER\\', '')
            Set-RegValueTracked $ruta 'State' 1 -Kind DWord   # 1 = deshabilitada
            Write-Host ("  {0}\{1}  State {2} -> 1" -f $fam.PSChildName, $task.PSChildName, $cur)
        }
    }
} finally { Save-RiceUndo }

if (Test-Path $bak) { Write-Host "`n(el antiguo $bak sigue ahi; ya no se usa)" }

# --- desinstalar ----------------------------------------------------------
if ($Uninstall) {
    Write-Host "`n=== desinstalando ==="
    Get-AppxPackage -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -match 'EarTrumpet|TwinkleTray' } | ForEach-Object {
        Write-Host ("  quitando " + $_.Name)
        Remove-AppxPackage $_.PackageFullName -ErrorAction SilentlyContinue
      }
    # Las clásicas traen su propio desinstalador; se lanza interactivo a
    # propósito, para que puedas ver qué borra antes de confirmar.
    foreach ($u in 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
                    'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*') {
        Get-ItemProperty $u -ErrorAction SilentlyContinue |
          Where-Object { $_.DisplayName -match 'AudioSwitch|Pomatez' } | ForEach-Object {
            Write-Host ("  lanza el desinstalador de " + $_.DisplayName)
            Write-Host ("     " + $_.UninstallString)
          }
    }
}

Write-Host "`nListo."
