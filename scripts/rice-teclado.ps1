<#
  Quita los atajos de accesibilidad que interrumpen, y acelera la repeticion de
  teclas.

      rice-teclado.ps1            aplica
      rice-teclado.ps1 -Restore   deshace (via rice-deshacer)
      rice-teclado.ps1 -Ver       enseña como estan ahora

  POR QUE, en un escritorio de tiling y juegos:

    StickyKeys       cinco Shift seguidos -> dialogo modal encima de todo. En un
                     juego eso es perder la partida; con GlazeWM, un modal que
                     roba el foco y descoloca el layout.
    FilterKeys       mantener Shift 8 segundos -> otro dialogo. Pasa solo con
                     movimiento sostenido en cualquier juego.
    ToggleKeys       mantener Bloq Num 5 segundos -> pitido.

  No se desactiva la accesibilidad "por rendimiento" -- eso seria mentira. Se
  desactiva porque son ATAJOS que se disparan solos con patrones de tecleo
  normales en juegos, y el dialogo es modal.

  Todo en HKCU, sin admin. Los valores son REG_SZ, no numeros: Windows los
  guarda como cadena y escribirlos como DWORD los ignora en silencio.

  Los "Flags" son mascaras de bits documentadas en SystemParametersInfo:
    bit 0 (1)   funcion activada
    bit 1 (2)   atajo de teclado activado
    bit 2 (4)   confirmacion visual al activarse
    bit 3 (8)   pitido al activarse
    bit 4 (16)  ...
  Los valores de abajo dejan el bit del ATAJO a cero, que es lo unico que
  molesta. La funcion sigue pudiendo activarse desde Configuracion.
#>
param([switch]$Restore, [switch]$Ver)

. "$env:USERPROFILE\.config\lib\rice-undo.ps1"
$Ambito = 'teclado'

$acc = 'HKCU:\Control Panel\Accessibility'
$ctl = 'HKCU:\Control Panel\Keyboard'

# nombre -> @{ Ruta; Valor; Kind; Por que }
$ajustes = @(
    @{ Ruta = "$acc\StickyKeys";        Nombre = 'Flags'; Valor = '506'; Kind = 'String'
       Que  = 'StickyKeys: quita el atajo de los 5 Shift (la funcion sigue disponible)' }
    @{ Ruta = "$acc\Keyboard Response"; Nombre = 'Flags'; Valor = '122'; Kind = 'String'
       Que  = 'FilterKeys: quita el atajo de mantener Shift 8s' }
    @{ Ruta = "$acc\ToggleKeys";        Nombre = 'Flags'; Valor = '58';  Kind = 'String'
       Que  = 'ToggleKeys: quita el atajo de mantener Bloq Num 5s' }
    # Repeticion de teclas al maximo. Esto SI se nota al navegar por texto y en
    # menus; no es un tweak de fe.
    @{ Ruta = $ctl; Nombre = 'KeyboardDelay'; Valor = '0';  Kind = 'String'
       Que  = 'retardo antes de repetir: el minimo (~250 ms)' }
    @{ Ruta = $ctl; Nombre = 'KeyboardSpeed'; Valor = '31'; Kind = 'String'
       Que  = 'velocidad de repeticion: la maxima' }
)

if ($Ver) {
    foreach ($a in $ajustes) {
        $actual = try { (Get-ItemProperty $a.Ruta -Name $a.Nombre -EA Stop).($a.Nombre) } catch { '(sin poner)' }
        Write-Host ("  {0,-38} {1,-12} -> {2}" -f "$($a.Ruta.Split('\')[-1])\$($a.Nombre)", $actual, $a.Valor)
        Write-Host ("     {0}" -f $a.Que) -ForegroundColor DarkGray
    }
    return
}

if ($Restore) {
    Undo-Rice $Ambito
    Write-Host 'Cierra y abre sesion para que todo vuelva a su sitio.'
    return
}

Start-RiceUndo $Ambito
try {
    foreach ($a in $ajustes) {
        Set-RegValueTracked $a.Ruta $a.Nombre $a.Valor -Kind ([Microsoft.Win32.RegistryValueKind]$a.Kind)
        Write-Host ("  {0}" -f $a.Que)
    }
} finally { Save-RiceUndo }

# Aplicar sin cerrar sesion. SPI_SETSTICKYKEYS y compania quieren la struct
# entera; para el retardo de teclado basta con SPI_SETKEYBOARDDELAY/SPEED, que
# toman el valor directo en wParam.
Add-Type -Namespace R -Name Kbd -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError=true)]
public static extern bool SystemParametersInfo(uint action, uint param, System.IntPtr vparam, uint winini);
'@ -EA SilentlyContinue

$SPI_SETKEYBOARDDELAY = 0x0017
$SPI_SETKEYBOARDSPEED = 0x000B
$SPIF_SENDCHANGE      = 0x02
[void][R.Kbd]::SystemParametersInfo($SPI_SETKEYBOARDDELAY, 0,  [IntPtr]::Zero, $SPIF_SENDCHANGE)
[void][R.Kbd]::SystemParametersInfo($SPI_SETKEYBOARDSPEED, 31, [IntPtr]::Zero, $SPIF_SENDCHANGE)

Write-Host ''
Write-Host 'Listo. La repeticion de teclas ya esta activa; los atajos de'
Write-Host 'accesibilidad se apagan del todo al volver a iniciar sesion.'
Write-Host "Deshacer:  rice-deshacer.ps1 $Ambito -Hacerlo"
