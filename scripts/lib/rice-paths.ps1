# Every path and machine constant the rice scripts need, derived from the current
# user's profile. Dot-source this instead of hardcoding C:\Users\<name>\...
#
# This is what lets the repo's scripts run in place: previously install.ps1 had to
# string-replace the author's home directory into every deployed file, so a script
# run straight from a checkout pointed at someone else's profile.

$Rice = @{
    Home      = $env:USERPROFILE
    Config    = "$env:USERPROFILE\.config"
    Lib       = "$env:USERPROFILE\.config\lib"
    Dev       = "$env:USERPROFILE\dev\target\release"
    Scoop     = "$env:USERPROFILE\scoop"
    ScoopApps = "$env:USERPROFILE\scoop\apps"

    Ffmpeg    = "$env:USERPROFILE\scoop\apps\ffmpeg\current\bin\ffmpeg.exe"
    Notify    = "$env:USERPROFILE\dev\target\release\shadowplay-notify.exe"

    WgcBuffer = "$env:USERPROFILE\ShadowPlay\wgc-buffer"
    Clips     = "$env:USERPROFILE\ShadowPlay\clips"
    Island    = "$env:USERPROFILE\.config\island.json"
    LogDir    = "$env:USERPROFILE\.config\logs"

    # 127.0.0.1, never 'localhost': localhost resolves ::1 first, which GlazeWM's
    # IPC doesn't listen on, so each connect burns a ~2.1s IPv6 timeout first.
    IpcUri    = [Uri]'ws://127.0.0.1:6123'

    # Physical monitor layout. Duplicated in the GlazeWM config's
    # startup_commands; keep them in step.
    Monitors  = @(
        @{ X = 0;    Width = 1920 },
        @{ X = 1920; Width = 2560 }
    )
}

function Get-RiceExe {
    param([Parameter(Mandatory)][string]$Name)
    Join-Path $Rice.Dev $Name
}
