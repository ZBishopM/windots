<#
  Un solo sitio para ver y deshacer TODO lo que los scripts del rice han tocado.

      rice-deshacer.ps1                    lista los ambitos guardados
      rice-deshacer.ps1 accent             detalla los pasos de uno
      rice-deshacer.ps1 accent -Hacerlo    lo deshace
      rice-deshacer.ps1 -Todo -Hacerlo     deshace absolutamente todo

  Antes cada script tenia su propio -Restore y su propio fichero de respaldo:
  .accent-backup.json, .run-trimmed.json, .retired-run.json... y dos que ni
  guardaban nada. Habia que acordarse de cual era cual. Ahora todo vive en
  ~\.config\undo\ y esto lo enseña junto.

  Sin -Hacerlo NO toca nada: enseña que pasaria.
#>
param(
    [Parameter(Position = 0)][string]$Ambito,
    [switch]$Hacerlo,
    [switch]$Todo
)

. "$env:USERPROFILE\.config\lib\rice-undo.ps1"

$dir = Join-Path $env:USERPROFILE '.config\undo'

if ($Todo) {
    if (-not (Test-Path $dir)) { Write-Host 'no hay nada que deshacer.'; return }
    $ambitos = @(Get-ChildItem $dir -Filter *.json -EA SilentlyContinue | ForEach-Object { $_.BaseName })
    if (-not $ambitos) { Write-Host 'no hay nada que deshacer.'; return }

    Write-Host ("se desharan {0} ambito(s): {1}`n" -f $ambitos.Count, ($ambitos -join ', '))
    foreach ($a in $ambitos) {
        Write-Host "--- $a ---"
        if ($Hacerlo) { Undo-Rice $a } else { Undo-Rice $a -Simular }
    }
    if (-not $Hacerlo) { Write-Host "`nNada tocado. Anade -Hacerlo para que vaya en serio." }
    return
}

if (-not $Ambito) {
    Get-RiceUndo
    Write-Host ''
    Write-Host 'rice-deshacer.ps1 <ambito>            ver los pasos'
    Write-Host 'rice-deshacer.ps1 <ambito> -Hacerlo   deshacerlo'
    Write-Host 'rice-deshacer.ps1 -Todo -Hacerlo      deshacerlo todo'
    return
}

if (-not $Hacerlo) {
    Get-RiceUndo $Ambito
    Write-Host ''
    Undo-Rice $Ambito -Simular
    Write-Host "`nNada tocado. Anade -Hacerlo para que vaya en serio."
    return
}

Undo-Rice $Ambito
