#Requires -Version 7.0
<#
  Builds and signs notifyd's sparse package -- the MSIX that grants
  dev\target\release\notifyd.exe package identity so UserNotificationListener
  actually returns something. See AppxManifest.xml for why this exists at all.

  Everything here runs UNELEVATED on purpose:
    - New-SelfSignedCertificate into Cert:\CurrentUser\My  : no admin
    - makeappx pack / signtool sign                        : no admin
    - Add-AppxPackage -ExternalLocation                    : no admin (per user)

  Exactly one step needs admin, and this script only PRINTS it: trusting the
  self-signed certificate, which means writing to LocalMachine\TrustedPeople.
  install.ps1 folds that into the single UAC prompt it already asks for.

    pwsh -File .\build.ps1                 # build + sign, print the next steps
    pwsh -File .\build.ps1 -Register       # ...and register it too (needs the
                                           # cert to be trusted already)
#>
param(
    # Where notifyd.exe lives. This is the folder that gains identity.
    [string]$ExternalLocation = "$env:USERPROFILE\dev\target\release",
    # Where the .msix, the .cer and the staging tree are produced.
    [string]$OutDir = "$env:USERPROFILE\.config\notifyd-package",
    [switch]$Register
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Say($m, $c = 'Cyan') { Write-Host "==> $m" -ForegroundColor $c }
function Ok($m) { Write-Host "    $m" -ForegroundColor DarkGray }

$manifest = Join-Path $PSScriptRoot 'AppxManifest.xml'
if (-not (Test-Path $manifest)) { throw "AppxManifest.xml not found next to build.ps1 ($PSScriptRoot)" }

# ---------------------------------------------------------------- SDK tools
# makeappx/signtool ship with the Windows SDK, under bin\<version>\<arch>\.
#
# Two traps here. Most of the version folders on a machine with a long SDK
# history contain neither tool (this box has six version folders and only two of
# them have makeappx.exe), so the newest DIRECTORY is not the newest TOOL --
# probe for the file itself. And the version folders must be sorted as [version],
# not as strings, or 10.0.9.0 sorts above 10.0.22621.0.
function Get-SdkTool([string]$Name) {
    $root = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $hit = Get-ChildItem $root -Directory -EA SilentlyContinue |
        Where-Object { $_.Name -match '^10\.' } |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    # The App Certification Kit carries unversioned copies of both.
    if (-not $hit) {
        $ack = "C:\Program Files (x86)\Windows Kits\10\App Certification Kit\$Name"
        if (Test-Path $ack) { $hit = $ack }
    }
    if (-not $hit) { throw "$Name not found under Windows Kits 10 -- install the Windows SDK (winget install Microsoft.WindowsSDK) and re-run" }
    $hit
}
$makeappx = Get-SdkTool 'makeappx.exe'
$signtool = Get-SdkTool 'signtool.exe'
Ok "makeappx $makeappx"
Ok "signtool $signtool"

# ---------------------------------------------------------------- staging
# The package carries the manifest and the logos and NOTHING else; notifyd.exe
# is the external content.
$stage = Join-Path $OutDir 'stage'
Remove-Item $stage -Recurse -Force -EA SilentlyContinue
New-Item -ItemType Directory -Force "$stage\Images" | Out-Null
Copy-Item $manifest "$stage\AppxManifest.xml" -Force

# The logos are never actually shown (AppListEntry="none"), but the manifest
# schema requires them and registration validates that they exist. Generated
# rather than committed so the repo stays free of binaries.
Add-Type -AssemblyName System.Drawing
function New-Logo([string]$Path, [int]$Size) {
    $bmp = New-Object System.Drawing.Bitmap $Size, $Size
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::FromArgb(255, 40, 33, 28))      # theme::SURFACE
    $brush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 224, 163, 92))  # theme::ACCENT
    $pad = [Math]::Max(1, [int]($Size * 0.28))
    $g.FillEllipse($brush, $pad, $pad, ($Size - 2 * $pad), ($Size - 2 * $pad))
    $brush.Dispose(); $g.Dispose()
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}
New-Logo "$stage\Images\Square44x44Logo.png"   44
New-Logo "$stage\Images\Square150x150Logo.png" 150
New-Logo "$stage\Images\StoreLogo.png"         50
Ok "staged -> $stage"

# ---------------------------------------------------------------- pack
# /nv (no validation) is REQUIRED, not a shortcut: semantic validation rejects a
# manifest whose <Application Executable=...> is not inside the package, which is
# precisely what a sparse package is.
$msix = Join-Path $OutDir 'notifyd-sparse.msix'
Remove-Item $msix -Force -EA SilentlyContinue
Say 'Packing'
& $makeappx pack /d $stage /p $msix /nv /o
Ok "-> $msix"

# ---------------------------------------------------------------- sign
# The certificate Subject must equal the manifest's Publisher byte for byte, so
# read it from the manifest instead of repeating it here.
[xml]$mx = Get-Content $manifest -Raw
$publisher = $mx.Package.Identity.Publisher
Ok "publisher $publisher"

$cert = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object { $_.Subject -eq $publisher -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date) } |
    Sort-Object NotAfter | Select-Object -Last 1
if (-not $cert) {
    Say 'Creating self-signed code-signing certificate (CurrentUser\My, no admin)'
    # 2.5.29.37 = Enhanced Key Usage, 1.3.6.1.5.5.7.3.3 = Code Signing.
    # 2.5.29.19={text} = Basic Constraints, end-entity. Both are what MSIX
    # signature validation looks for; a plain New-SelfSignedCertificate without
    # them produces a cert signtool will happily use and Windows will reject.
    $cert = New-SelfSignedCertificate -Type Custom -Subject $publisher `
        -KeyUsage DigitalSignature -FriendlyName 'rice notifyd sparse package' `
        -CertStoreLocation 'Cert:\CurrentUser\My' -NotAfter (Get-Date).AddYears(10) `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
}
Ok "cert $($cert.Thumbprint)"

$cer = Join-Path $OutDir 'notifyd-sparse.cer'
Export-Certificate -Cert $cert -FilePath $cer -Force | Out-Null

Say 'Signing'
# No /t timestamp: the cert is valid for ten years and a timestamp server would
# make this step fail on a machine with no network.
& $signtool sign /fd SHA256 /sha1 $cert.Thumbprint $msix
Ok "signed"

# ---------------------------------------------------------------- next steps
$trust = "Import-Certificate -FilePath '$cer' -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
$reg = "Add-AppxPackage -Path '$msix' -ExternalLocation '$ExternalLocation'"

if ($Register) {
    if (-not (Test-Path $ExternalLocation)) { throw "external location does not exist: $ExternalLocation (build the workspace first)" }
    if (-not (Test-Path (Join-Path $ExternalLocation 'notifyd.exe'))) {
        throw "notifyd.exe is not in $ExternalLocation -- build it first (cargo build --release -p notifyd)"
    }
    Say 'Registering'
    # Re-registering the same Version over itself fails with 0x80073CF9, so an
    # install re-run has to unregister first. Only ever touches our own package
    # name, which is read from the manifest.
    $pkgName = $mx.Package.Identity.Name
    Get-AppxPackage -Name $pkgName -EA SilentlyContinue | ForEach-Object {
        Ok "removing existing $($_.PackageFullName)"
        Remove-AppxPackage -Package $_.PackageFullName -EA SilentlyContinue
    }
    try {
        Add-AppxPackage -Path $msix -ExternalLocation $ExternalLocation
        Ok 'registered'
    } catch {
        # Overwhelmingly the cause: the certificate is not in TrustedPeople yet,
        # which surfaces as CERT_E_UNTRUSTEDROOT (0x800B0109).
        Write-Host "    registration failed: $_" -ForegroundColor Yellow
        Write-Host "    trust the certificate first, ELEVATED:" -ForegroundColor Yellow
        Write-Host "      $trust" -ForegroundColor Yellow
        Write-Host "    full deployment errors: Event Viewer > Applications and Services Logs >" -ForegroundColor Yellow
        Write-Host "      Microsoft > Windows > AppxDeployment-Server" -ForegroundColor Yellow
        # throw, not `exit`: install.ps1 calls this and needs to catch it rather
        # than read $LASTEXITCODE out of a PowerShell script.
        throw 'notifyd sparse package registration failed (see the hints above)'
    }
} else {
    Write-Host ''
    Say 'Next steps' Green
    Write-Host @"
    1. ELEVATED, once (this is the only step that needs admin):
         $trust

    2. Not elevated:
         $reg

    3. Turn Do Not Disturb ON (Settings > System > Notifications > Do not disturb,
       or Win+N and toggle the bell). DND suppresses the blue banner but still
       delivers every notification to notifyd -- that is what makes the swap work.

    Verify:  notifyd.exe --check      then read ~\.config\logs\notifyd.log
"@ -ForegroundColor DarkGray
}
