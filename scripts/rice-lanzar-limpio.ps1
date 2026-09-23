<#
Lanza un proceso del rice con el entorno de una sesion NORMAL, no con el de
quien lo lanza.

POR QUE EXISTE: lanzar algo del rice desde Claude Code (o desde cualquier
consola con variables propias) le pega al hijo CLAUDE_CODE_*, NO_COLOR=1 y las
variables de git no interactivo. Si el hijo es el supervisor, todo lo que el
supervisor relance hereda lo mismo: sesiones de Claude en blanco y negro y el
aviso "inherited CLAUDE_CODE_CHILD_SESSION marker". Tachar variables una a una
no basta: se probo dos veces y siempre se colaba alguna.

La receta: vaciar el entorno y reconstruirlo desde el registro (Machine y
User) MAS las variables que pone el inicio de sesion y que NO estan en el
registro. Olvidar esas dejo una vez al supervisor sin USERPROFILE: arrancaba,
no encontraba nada y no volvia a escribir en su log.

    rice-lanzar-limpio.ps1 wscript.exe "$env:USERPROFILE\.config\rice-supervisor.vbs"
    rice-lanzar-limpio.ps1 ... -Mostrar     imprime el entorno que pasaria y no lanza
#>
param(
    [Parameter(Mandatory, Position = 0)][string]$Programa,
    [Parameter(Position = 1, ValueFromRemainingArguments)][string[]]$Argumentos = @(),
    [switch]$Mostrar
)
$ErrorActionPreference = 'Stop'

$env2 = [ordered]@{}
foreach ($ambito in 'Machine', 'User') {
    $vars = [Environment]::GetEnvironmentVariables($ambito)
    foreach ($k in $vars.Keys) {
        $v = [Environment]::ExpandEnvironmentVariables([string]$vars[$k])
        # PATH se SUMA (maquina + usuario), como hace el inicio de sesion.
        if ($k -eq 'Path' -and $env2.Contains('Path')) { $env2['Path'] = $env2['Path'].TrimEnd(';') + ';' + $v }
        else { $env2[$k] = $v }
    }
}

# Las que pone el inicio de sesion y no estan en el registro. Se copian de este
# proceso: sus VALORES son los de la maquina, no los de Claude.
$DE_SESION = @('USERPROFILE', 'USERNAME', 'USERDOMAIN', 'USERDOMAIN_ROAMINGPROFILE', 'APPDATA', 'LOCALAPPDATA',
               'HOMEDRIVE', 'HOMEPATH', 'SystemRoot', 'SystemDrive', 'windir', 'ProgramData',
               'ProgramFiles', 'ProgramFiles(x86)', 'ProgramW6432', 'CommonProgramFiles',
               'CommonProgramFiles(x86)', 'CommonProgramW6432', 'ALLUSERSPROFILE', 'PUBLIC',
               'COMPUTERNAME', 'SESSIONNAME', 'LOGONSERVER', 'PATHEXT', 'ComSpec',
               'PROCESSOR_ARCHITECTURE', 'PROCESSOR_IDENTIFIER', 'PROCESSOR_LEVEL',
               'PROCESSOR_REVISION', 'NUMBER_OF_PROCESSORS', 'OS', 'OneDrive')
foreach ($k in $DE_SESION) {
    $v = [Environment]::GetEnvironmentVariable($k, 'Process')
    if ($v -and -not $env2.Contains($k)) { $env2[$k] = $v }
}

# Comprobacion: nada de lo que delata a quien lanza.
$sucias = @($env2.Keys | Where-Object { $_ -match '^(CLAUDE|NO_COLOR$|GIT_TERMINAL_PROMPT$|GIT_EDITOR$|GIT_ASKPASS$|GCM_INTERACTIVE$)' })
if ($sucias) { throw "el entorno reconstruido trae variables de la sesion: $($sucias -join ', ')" }
foreach ($k in 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'SystemRoot', 'Path', 'TEMP') {
    if (-not $env2[$k]) { throw "falta $k en el entorno reconstruido" }
}

if ($Mostrar) { $env2.GetEnumerator() | Sort-Object Key | ForEach-Object { "{0}={1}" -f $_.Key, $_.Value }; return }

$psi = New-Object Diagnostics.ProcessStartInfo $Programa, (($Argumentos | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }) -join ' ')
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.WorkingDirectory = $env2['USERPROFILE']
$psi.EnvironmentVariables.Clear()
foreach ($e in $env2.GetEnumerator()) { $psi.EnvironmentVariables[$e.Key] = $e.Value }
$p = [Diagnostics.Process]::Start($psi)
"lanzado $Programa (pid $($p.Id)) con $($env2.Count) variables, ninguna de esta sesion"
