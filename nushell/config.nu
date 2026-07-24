# Nushell config for the rice. This is the fast *interactive* shell (~67ms warm
# vs pwsh ~350ms). PowerShell still runs all the infra scripts (rice-supervisor,
# glazewm-dwindle, shadowplay-wgc-save) -- those stay .ps1. Try nu: run `nu`.

# Hide nushell's own startup banner -- we show fastfetch instead.
$env.config.show_banner = false

# --- fastfetch on the top-level WezTerm interactive shell (cached, same trick as
#     the pwsh profile: print the instant cache, refresh it in the background so
#     next launch is current). Guarded so nested shells stay quiet. ---
if ('WEZTERM_PANE' in $env) and ('FASTFETCH_SHOWN' not-in $env) {
    $env.FASTFETCH_SHOWN = '1'
    let ff_cache = $'($env.LOCALAPPDATA)/ff-cache.txt'
    if ($ff_cache | path exists) {
        print -n (open --raw $ff_cache)
    } else {
        ^fastfetch
    }
    # refresh the cache for next time without holding up the prompt
    job spawn { ^fastfetch --pipe false | save -f $'($env.LOCALAPPDATA)/ff-cache.txt' } | ignore
}

# --- warm prompt: cwd in amber, a lime arrow. Matches the bar/island palette. ---
$env.PROMPT_COMMAND = {||
    let dir = ($env.PWD | str replace $env.USERPROFILE '~')
    $'(ansi { fg: "#e0a35c" })($dir)(ansi reset)'
}
$env.PROMPT_COMMAND_RIGHT = {|| '' }
$env.PROMPT_INDICATOR = {|| $'(ansi { fg: "#a9b56a" }) ❯ (ansi reset)' }
$env.PROMPT_INDICATOR_VI_INSERT = {|| $'(ansi { fg: "#a9b56a" }) ❯ (ansi reset)' }
$env.PROMPT_MULTILINE_INDICATOR = {|| '::: ' }

# --- rice tools (call the same exes the pwsh profile does) ---
def cava [...rest] {
    ^$'($env.USERPROFILE)/dev/target/release/cava.exe' ...$rest
}

def notify-test [
    title: string = 'Prueba de notificación'
    body: string = 'cuerpo de ejemplo'
    icon: string = 'info'      # mic | replay | check | rec | info | warn | term
    accent: string = '#e0a35c'
] {
    ^$'($env.USERPROFILE)/dev/target/release/shadowplay-notify.exe' --title $title --body $body --icon $icon --accent $accent --hold 6
}

# mic: cycle HyperX <-> Snowball, feed the island + toast the new one.
def mic [] {
    let name = (^$'($env.USERPROFILE)/dev/target/release/micswitch.exe' | str trim)
    if ($name | is-empty) { print 'No hay HyperX/Snowball activo'; return }
    let short = ($name | str replace -r '^Micr[oó]fono \(' '' | str replace -r '\)$' '')
    {icon: 'mic', title: 'Micrófono', body: $short, accent: '#e0a35c'}
        | to json -r | save -f $'($env.USERPROFILE)/.config/island.json'
    ^$'($env.USERPROFILE)/dev/target/release/shadowplay-notify.exe' --title 'Micrófono' --body $short --icon mic --accent '#e0a35c' --hold 4
}

# handy aliases
alias ll = ls -l
alias la = ls -a
alias gs = git status
