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
Write-Host "`n=== autoarranque clasico ==="
if ($Restore) {
    if (Test-Path $bak) {
        (Get-Content $bak -Raw | ConvertFrom-Json).PSObject.Properties | ForEach-Object {
            New-ItemProperty $run -Name $_.Name -Value $_.Value -PropertyType String -Force | Out-Null
            Write-Host ("  restaurado {0}" -f $_.Name)
        }
    } else { Write-Host '  (no hay copia que restaurar)' }
} else {
    $saved = @{}
    $p = Get-ItemProperty $run
    foreach ($n in (Get-Item $run).Property) {
        if ($n -match 'pomatez|eartrumpet|twinkle|audioswitch' -or "$($p.$n)" -match 'pomatez|eartrumpet|twinkle|audioswitch') {
            $saved[$n] = $p.$n
            Remove-ItemProperty $run -Name $n -ErrorAction SilentlyContinue
            Write-Host ("  quitado  {0}" -f $n)
        }
    }
    if ($saved.Count) { $saved | ConvertTo-Json | Set-Content $bak -Encoding utf8 }
}

# --- autoarranque de apps de la Store -------------------------------------
# Las apps empaquetadas no usan la clave Run: su interruptor es un DWORD `State`
# bajo AppModel\SystemAppData\<familia>\<tarea>. 1 = activado, 0 = desactivado.
Write-Host "`n=== autoarranque de apps de la Store ==="
$base = 'HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData'
foreach ($fam in Get-ChildItem $base -ErrorAction SilentlyContinue |
                 Where-Object { $_.PSChildName -match 'EarTrumpet|TwinkleTray' }) {
    foreach ($task in Get-ChildItem $fam.PSPath -ErrorAction SilentlyContinue) {
        $cur = (Get-ItemProperty $task.PSPath -ErrorAction SilentlyContinue).State
        if ($null -eq $cur) { continue }   # sin State no es una tarea de inicio
        $want = if ($Restore) { 2 } else { 1 }   # 2 = habilitada por el usuario, 1 = deshabilitada
        Set-ItemProperty $task.PSPath -Name State -Value $want -Type DWord -ErrorAction SilentlyContinue
        Write-Host ("  {0}\{1}  State {2} -> {3}" -f $fam.PSChildName, $task.PSChildName, $cur, $want)
    }
}

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
