# Para los componentes del rice que un anti-cheat puede tomar por trampas, para
# poder descartarlos por eliminacion.
#
#   rice-modo-juego.ps1           los para (y para el supervisor, o los revive)
#   rice-modo-juego.ps1 -Volver   los devuelve
#   rice-modo-juego.ps1 -Estado   dice que hay puesto ahora
#
# NO necesita administrador.
#
# POR QUE EXISTE: Apex dio "Badware Detected" y r5apex_dx12.exe se cayo con
# 0xc0000005 en modulo 'unknown' -- una violacion de acceso ejecutando en memoria
# sin modulo asociado, que es la firma de codigo inyectado. En esta maquina
# inyectan tres cosas: AltSnap (tiene hooks.dll, asi que su gancho se mete en
# cada proceso), el overlay de Steam y el de Discord. Los logs no distinguen
# cual, asi que hay que quitarlos de uno en uno.
#
# AutoHotkey se para tambien aunque NO inyecte -- usa ganchos low-level, que son
# fuera de proceso -- porque aparece por nombre en todas las guias de este error
# y descartarlo cuesta nada.
#
# EL SUPERVISOR HAY QUE PARARLO PRIMERO, o revive AltSnap y AHK en 30 segundos y
# la prueba no vale nada. Eso es lo que hace esta herramienta y no un
# Stop-Process a mano.
[CmdletBinding()]
param([switch]$Volver, [switch]$Estado)

. "$env:USERPROFILE\.config\lib\rice-paths.ps1"

# El propio PID se excluye SIEMPRE al buscar por linea de comandos: esta sesion
# lleva la cadena 'rice-supervisor' escrita en su propio comando, y filtrar sin
# excluirse mata la consola desde la que estas trabajando. Ya paso dos veces.
function Get-Supervisor {
    Get-CimInstance Win32_Process -Filter "Name='pwsh.exe'" -EA SilentlyContinue |
        Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -match '-File\s+\S*rice-supervisor\.ps1' }
}

if ($Estado) {
    Write-Host '== estado =='
    foreach ($p in 'AutoHotkey64', 'AltSnap') {
        $n = (Get-Process $p -EA SilentlyContinue | Measure-Object).Count
        Write-Host ("  {0,-14} {1}" -f $p, $(if ($n) { "corriendo ($n)" } else { 'parado' }))
    }
    Write-Host ("  {0,-14} {1}" -f 'supervisor', $(if (Get-Supervisor) { 'corriendo' } else { 'parado' }))
    # Ojo no aparece por nombre de proceso util -- son un python y un
    # llama-server entre otros iguales -- asi que se pregunta por sus puertos.
    foreach ($o in @{n='ojo-vlm'; u='http://127.0.0.1:8099/health'}, @{n='ojo-oido'; u='http://127.0.0.1:17494/salud'}) {
        $vivo = $false
        try { $null = Invoke-RestMethod $o.u -TimeoutSec 2; $vivo = $true } catch { }
        Write-Host ("  {0,-14} {1}" -f $o.n, $(if ($vivo) { 'corriendo' } else { 'parado' }))
    }
    Write-Host "`n  Overlays (se apagan en su propia app, no desde aqui):"
    $s = Get-ItemProperty 'HKCU:\SOFTWARE\Valve\Steam' -EA SilentlyContinue
    if ($s.SteamPath) {
        $ud = ($s.SteamPath -replace '/', '\') + '\userdata'
        Get-ChildItem $ud -Directory -EA SilentlyContinue | ForEach-Object {
            $f = Join-Path $_.FullName 'config\localconfig.vdf'
            if (Test-Path $f) {
                Select-String -Path $f -Pattern 'EnableGameOverlay' -EA SilentlyContinue |
                    Select-Object -First 1 | ForEach-Object { Write-Host ("    Steam: {0}" -f $_.Line.Trim()) }
            }
        }
    }
    Write-Host ("    Discord: {0} procesos" -f (Get-Process Discord -EA SilentlyContinue | Measure-Object).Count)
    exit 0
}

if ($Volver) {
    Write-Host '== devolviendo el rice =='
    # Basta con relanzar el supervisor: su tabla de componentes se encarga de
    # AltSnap y AutoHotkey en el primer tick.
    if (Get-Supervisor) { Write-Host '   el supervisor ya estaba vivo' }
    else {
        Start-Process wscript.exe -ArgumentList "$($Rice.Config)\rice-supervisor.vbs"
        Write-Host '   supervisor relanzado (revive AltSnap y AHK en su primer tick)'
    }
    Start-Sleep -Seconds 10
    foreach ($p in 'AutoHotkey64', 'AltSnap') {
        Write-Host ("   {0,-14} {1}" -f $p, $(if (Get-Process $p -EA SilentlyContinue) { 'de vuelta' } else { 'AUN NO (dale otros 30 s)' }))
    }
    exit 0
}

Write-Host '== modo juego: quitando lo que un anti-cheat puede confundir =='
$sup = Get-Supervisor
if ($sup) {
    foreach ($s in $sup) { Stop-Process -Id $s.ProcessId -Force -EA SilentlyContinue }
    Write-Host '   supervisor parado (si no, revive lo de abajo en 30 s)'
}
foreach ($p in 'AltSnap', 'AutoHotkey64') {
    $n = (Get-Process $p -EA SilentlyContinue | Measure-Object).Count
    Get-Process $p -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    Write-Host ("   {0,-14} parado ({1})" -f $p, $n)
}

# Ojo: aqui no es por anti-cheat, es por VRAM.
#
# Su modelo de vision ocupa 11,4 de los 12,28 GB de la tarjeta desde que se
# enciende el equipo, asi que dejarlo puesto mientras se juega es quedarse sin
# memoria de video. Cuando eso pasa, CUDA se desborda a memoria compartida por
# PCIe y el juego se arrastra igual que se arrastro el modelo en su dia.
#
# Va DESPUES de parar el supervisor, como todo lo de arriba: si no, lo revive
# en su siguiente vuelta de 30 s.
#
# Solo el del puerto 8099. El 35B del Win+Space, si estuviera puesto, se para
# con `rice-llm.ps1 -Stop`, que es su herramienta.
$vlm = @(Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" -EA SilentlyContinue |
         Where-Object { $_.CommandLine -match '--port\s+8099\b' })
foreach ($v in $vlm) { Stop-Process -Id $v.ProcessId -Force -EA SilentlyContinue }
Write-Host ("   {0,-14} parado ({1})  -- libera ~11,4 GB de VRAM" -f 'ojo-vlm', $vlm.Count)

# El oido son 769 MB de RAM y cero VRAM, asi que no estorba a un juego. Se para
# igualmente porque con el atajo muerto no sirve de nada, y porque tiene el
# microfono abierto mientras graba.
$oido = @(Get-CimInstance Win32_Process -Filter "Name='python.exe'" -EA SilentlyContinue |
          Where-Object { $_.CommandLine -match 'escuchar\.py' })
foreach ($o in $oido) { Stop-Process -Id $o.ProcessId -Force -EA SilentlyContinue }
Write-Host ("   {0,-14} parado ({1})" -f 'ojo-oido', $oido.Count)

Write-Host @'

Lo que NO se toca desde aqui, y hay que apagar a mano si esto no basta:

  Overlay de Steam   Biblioteca > clic derecho en Apex > Propiedades >
                     desmarcar "Habilitar la superposicion de Steam"
                     (por juego: no pierdes el overlay en el resto)

  Overlay de Discord Ajustes de usuario > Superposicion de juego > apagar

Orden sugerido, de menos molestia a mas:
  1. Overlay de Steam solo en Apex  -- es el sospechoso mas comun y el mas barato
  2. Overlay de Discord
  3. Esto (AltSnap + AutoHotkey)

Cuando funcione, ve devolviendo uno a uno hasta que vuelva a fallar: ahi esta.
Pierdes Super+arrastrar (AltSnap) y Win+Space (AHK) mientras dure la prueba.

Para devolver todo:  rice-modo-juego.ps1 -Volver
'@
