# Nushell config for the rice. This is the fast interactive shell (~67ms warm vs
# pwsh ~350ms). PowerShell still runs the infra scripts (rice-supervisor,
# glazewm-dwindle, shadowplay-wgc-save) -- those stay .ps1.
#
# Layout:
#   1. shell behaviour + completions
#   2. warm colour theme (same palette as the bar and the toasts)
#   3. context detection (node / pnpm / python / git / ssh) -- computed on
#      directory change, not on every prompt
#   4. prompt
#   5. rice tools + aliases

# ---------------------------------------------------------------- 1. behaviour
$env.config.show_banner = false
$env.config.edit_mode = 'emacs'
$env.config.shell_integration.osc133 = true   # let WezTerm mark prompt boundaries

# Completions. Nushell completes commands, paths, flags and subcommands out of
# the box, but defaults to prefix matching with no menu; fuzzy + a menu makes it
# behave the way the rest of the rice does.
$env.config.completions = {
    algorithm: 'fuzzy'
    case_sensitive: false
    quick: true        # auto-accept when only one match
    partial: true      # complete the common prefix
    sort: 'smart'
    external: { enable: true, max_results: 100 }
    use_ls_colors: true
}

$env.config.history = {
    file_format: 'sqlite'   # shared across concurrent shells, unlike plaintext
    max_size: 100_000
    sync_on_enter: true
    isolation: false
}

$env.config.rm.always_trash = true   # recoverable deletes

# ---------------------------------------------------------------- 2. theme
# Same palette as rice-common::theme (the bar, the island, the toasts).
let warm = {
    bg:      '#1a1613'
    surface: '#282118'
    hi:      '#3c322b'
    text:    '#e9e0d6'
    sub:     '#aa9a8c'
    amber:   '#e0a35c'
    lime:    '#a9b56a'
    terra:   '#d08770'
    rust:    '#c67b5c'
}

$env.config.color_config = {
    separator: $warm.hi
    leading_trailing_space_bg: { attr: 'n' }
    header: { fg: $warm.lime, attr: 'b' }
    empty: $warm.sub
    bool: $warm.amber
    int: $warm.text
    filesize: $warm.lime
    duration: $warm.amber
    date: $warm.sub
    range: $warm.text
    float: $warm.text
    string: $warm.text
    nothing: $warm.sub
    binary: $warm.terra
    cell-path: $warm.sub
    row_index: { fg: $warm.sub, attr: 'b' }
    record: $warm.text
    list: $warm.text
    block: $warm.text
    hints: $warm.hi
    search_result: { fg: $warm.bg, bg: $warm.amber }

    shape_and: { fg: $warm.terra, attr: 'b' }
    shape_binary: { fg: $warm.terra, attr: 'b' }
    shape_block: { fg: $warm.amber, attr: 'b' }
    shape_bool: $warm.amber
    shape_closure: { fg: $warm.lime, attr: 'b' }
    shape_custom: $warm.lime
    shape_datetime: { fg: $warm.sub, attr: 'b' }
    shape_directory: $warm.amber
    shape_external: $warm.lime
    shape_externalarg: { fg: $warm.text, attr: 'b' }
    shape_filepath: $warm.amber
    shape_flag: { fg: $warm.terra, attr: 'b' }
    shape_float: { fg: $warm.text, attr: 'b' }
    shape_garbage: { fg: '#ffffff', bg: '#c1272d', attr: 'b' }
    shape_globpattern: { fg: $warm.amber, attr: 'b' }
    shape_int: { fg: $warm.text, attr: 'b' }
    shape_internalcall: { fg: $warm.lime, attr: 'b' }
    shape_literal: $warm.text
    shape_match_pattern: $warm.lime
    shape_matching_brackets: { attr: 'u' }
    shape_nothing: $warm.sub
    shape_operator: $warm.terra
    shape_or: { fg: $warm.terra, attr: 'b' }
    shape_pipe: { fg: $warm.terra, attr: 'b' }
    shape_range: { fg: $warm.amber, attr: 'b' }
    shape_record: { fg: $warm.text, attr: 'b' }
    shape_redirection: { fg: $warm.terra, attr: 'b' }
    shape_signature: { fg: $warm.lime, attr: 'b' }
    shape_string: $warm.lime
    shape_string_interpolation: { fg: $warm.terra, attr: 'b' }
    shape_table: { fg: $warm.amber, attr: 'b' }
    shape_variable: $warm.rust
    shape_vardecl: { fg: $warm.rust, attr: 'u' }
}

# Completion / history menus, themed to match.
$env.config.menus = [
    {
        name: completion_menu
        only_buffer_difference: false
        marker: '| '
        type: { layout: columnar, columns: 4, col_padding: 2 }
        style: {
            text: $warm.text
            selected_text: { fg: $warm.bg, bg: $warm.amber, attr: 'b' }
            description_text: $warm.sub
        }
    }
    {
        name: history_menu
        only_buffer_difference: true
        marker: '? '
        type: { layout: list, page_size: 10 }
        style: {
            text: $warm.text
            selected_text: { fg: $warm.bg, bg: $warm.lime, attr: 'b' }
            description_text: $warm.sub
        }
    }
]

$env.config.keybindings = [
    { name: completion_menu, modifier: none, keycode: tab, mode: [emacs vi_normal vi_insert]
      event: { until: [ { send: menu, name: completion_menu }, { send: menunext } ] } }
    { name: history_menu, modifier: control, keycode: char_r, mode: [emacs vi_insert vi_normal]
      event: { send: menu, name: history_menu } }
    # Accept the inline (history) suggestion, fish-style. On Ctrl+E, not Ctrl+F:
    # WezTerm claims Ctrl+F for scrollback search and never forwards it to the
    # shell. (Right arrow at end-of-line also accepts the hint, natively.)
    { name: accept_hint, modifier: control, keycode: char_e, mode: [emacs vi_insert]
      event: { send: historyhintcomplete } }
]

# ---------------------------------------------------------------- 3. context
# Which toolchains are in play, shown on the right of the prompt.
#
# Version lookups are the expensive part (each is a process spawn), so they run
# ONCE per session and are cached here. Which badges to *show* is then decided
# from marker files, and only when the directory actually changes -- the prompt
# itself does no work at all.
$env.RICE_NODE_V = (if (which node | is-not-empty) { ^node --version | str trim | str replace 'v' '' } else { '' })
$env.RICE_PY_V = (if (which python | is-not-empty) {
    ^python --version | str trim | str replace 'Python ' ''
} else { '' })

def --env rice-context [] {
    mut parts = []

    # Node: any JS project marker in this directory.
    let js = (['package.json' 'pnpm-lock.yaml' 'bun.lockb' 'deno.json'] | any {|f| ($f | path exists) })
    if $js and ($env.RICE_NODE_V | is-not-empty) {
        let pm = if ('pnpm-lock.yaml' | path exists) { 'pnpm'
        } else if ('yarn.lock' | path exists) { 'yarn'
        } else if ('bun.lockb' | path exists) { 'bun'
        } else { 'npm' }
        $parts = ($parts | append $'(ansi { fg: "#a9b56a" })⬢ ($env.RICE_NODE_V) ($pm)(ansi reset)')
    }

    # Python: a project marker or an active virtualenv.
    let py = (['pyproject.toml' 'requirements.txt' '.python-version' 'setup.py'] | any {|f| ($f | path exists) })
    let venv = ('VIRTUAL_ENV' in $env)
    if ($py or $venv) and ($env.RICE_PY_V | is-not-empty) {
        let tag = if $venv { $'($env.VIRTUAL_ENV | path basename)' } else { $env.RICE_PY_V }
        $parts = ($parts | append $'(ansi { fg: "#e0a35c" })🐍 ($tag)(ansi reset)')
    }

    # Rust.
    if ('Cargo.toml' | path exists) {
        $parts = ($parts | append $'(ansi { fg: "#c67b5c" })🦀(ansi reset)')
    }

    # Git branch + dirty marker.
    let br = (do -i { ^git rev-parse --abbrev-ref HEAD } | complete)
    if $br.exit_code == 0 {
        let name = ($br.stdout | str trim)
        let dirty = (do -i { ^git status --porcelain } | complete)
        let mark = if ($dirty.exit_code == 0 and ($dirty.stdout | str trim | is-not-empty)) { '*' } else { '' }
        $parts = ($parts | append $'(ansi { fg: "#d08770" }) ($name)($mark)(ansi reset)')
    }

    # Remote session.
    if ('SSH_CONNECTION' in $env) or ('SSH_TTY' in $env) {
        $parts = ($parts | append $'(ansi { fg: "#e0a35c" })󰣀 ssh(ansi reset)')
    }

    $env.RICE_CTX = ($parts | str join '  ')
}

# Recompute only when the directory changes.
$env.config.hooks.env_change.PWD = [ {|before, after| rice-context } ]
rice-context

# ---------------------------------------------------------------- 4. prompt
$env.PROMPT_COMMAND = {||
    let dir = ($env.PWD | str replace $env.USERPROFILE '~')
    let admin = (if (is-admin) { $'(ansi { fg: "#d08770" })# ' } else { '' })
    $'($admin)(ansi { fg: "#e0a35c" })($dir)(ansi reset)'
}
$env.PROMPT_COMMAND_RIGHT = {|| $env.RICE_CTX? | default '' }
# Lime arrow normally, terracotta when the last command failed.
$env.PROMPT_INDICATOR = {||
    let c = if ($env.LAST_EXIT_CODE? | default 0) == 0 { '#a9b56a' } else { '#d08770' }
    $'(ansi { fg: $c }) ❯ (ansi reset)'
}
$env.PROMPT_INDICATOR_VI_INSERT = $env.PROMPT_INDICATOR
$env.PROMPT_INDICATOR_VI_NORMAL = $env.PROMPT_INDICATOR
$env.PROMPT_MULTILINE_INDICATOR = {|| $'(ansi { fg: "#aa9a8c" })::: (ansi reset)' }

# ---------------------------------------------------------------- 5. fastfetch
# Only for a top-level interactive WezTerm shell. Prints the instant cache, then
# refreshes it in the background for next time -- same trick as the pwsh profile.
# NOTE: test the VALUE, not key presence. .wezterm.lua sets FASTFETCH_SHOWN to an
# empty string for every new pane (so a nested shell inherits it and stays quiet
# while a top-level one still shows), and `'X' not-in $env` is false for a key
# that exists but is empty -- which silently skipped fastfetch entirely.
if ('WEZTERM_PANE' in $env) and (($env.FASTFETCH_SHOWN? | default '') | is-empty) {
    $env.FASTFETCH_SHOWN = '1'
    let ff_cache = $'($env.LOCALAPPDATA)/ff-cache.txt'
    if ($ff_cache | path exists) { print -n (open --raw $ff_cache) } else { ^fastfetch }
    job spawn { ^fastfetch --pipe false | save -f $'($env.LOCALAPPDATA)/ff-cache.txt' } | ignore
}

# ---------------------------------------------------------------- 6. rice tools
def rice-exe [name: string] { $'($env.USERPROFILE)/dev/target/release/($name)' }

def cava [...rest] { ^(rice-exe 'cava.exe') ...$rest }

def notify-test [
    title: string = 'Prueba de notificación'
    body: string = 'cuerpo de ejemplo'
    icon: string = 'info'      # mic | replay | check | rec | info | warn | term
    accent: string = '#e0a35c'
] {
    ^(rice-exe 'shadowplay-notify.exe') --title $title --body $body --icon $icon --accent $accent --hold 6
}

# mic: cycle the active mic and toast the new one.
def mic [] {
    let name = (^(rice-exe 'micswitch.exe') | str trim)
    if ($name | is-empty) { print 'No hay micrófono configurado activo'; return }
    let short = ($name | str replace -r '^Micr[oó]fono \(' '' | str replace -r '\)$' '')
    {icon: 'mic', title: 'Micrófono', body: $short, accent: '#e0a35c'}
        | to json -r | save -f $'($env.USERPROFILE)/.config/island.json'
    ^(rice-exe 'shadowplay-notify.exe') --title 'Micrófono' --body $short --icon mic --accent '#e0a35c' --hold 4
}

# speaker: switch the default PLAYBACK device -- what AudioSwitch was sitting in
# the tray for. With no argument it TOGGLES between the two you actually use
# (headset <-> monitor) rather than cycling all fourteen endpoints.
#   speaker            -> toggle HyperX <-> VG270
#   speaker hyperx     -> jump to a specific one by substring
#   speaker --list     -> all outputs, * marks the active one
#   speaker --cycle    -> step to the next endpoint
def speaker [name?: string, --list, --cycle] {
    let exe = (rice-exe 'micswitch.exe')
    if $list { return (^$exe --output --list) }

    let target = if ($name | is-not-empty) { $name } else if $cycle { null } else {
        # Which of the pair is active now? Pick the other one.
        let cur = (^$exe --output --list | lines | where { |l| $l starts-with '*' } | get -o 0 | default '')
        if ($cur | str lowercase | str contains 'hyperx') { 'VG270' } else { 'HyperX' }
    }
    let new = if ($target | is-empty) {
        (^$exe --output | str trim)
    } else {
        (^$exe --output --set $target | str trim)
    }
    if ($new | is-empty) { return }
    {icon: 'desktop', title: 'Salida de audio', body: $new, accent: '#a9b56a'}
        | to json -r | save -f $'($env.USERPROFILE)/.config/island.json'
    ^(rice-exe 'shadowplay-notify.exe') --title 'Salida de audio' --body $new --icon desktop --accent '#a9b56a' --hold 4
}

# vol: master and per-application volume -- what EarTrumpet did from the tray.
#   vol                    list master + every app playing audio
#   vol 40                 master to 40%
#   vol discord 20         Discord to 20% (across all its processes)
#   vol discord mute       (also: unmute)
def vol [...args: string] { ^(rice-exe 'appvol.exe') ...$args }

def island-test [
    title: string = 'Prueba'
    body: string = 'cuerpo de ejemplo'
    icon: string = 'info'
    accent: string = '#e0a35c'
] {
    {icon: $icon, title: $title, body: $body, accent: $accent}
        | to json -r | save -f $'($env.USERPROFILE)/.config/island.json'
}

# ---------------------------------------------------------------- 7. aliases
alias ll = ls -l
alias la = ls -a
alias lla = ls -la
alias gs = git status
alias gd = git diff
alias gl = git log --oneline -20
alias .. = cd ..
alias ... = cd ../..

# Retoma la conversacion de los proyectos. Lo usa rice-autostart.ps1 para la
# ventana de D:/2026-projects.
#
# `--resume <termino>` abre el selector de conversaciones filtrando por ese
# texto. NO es lo mismo que pasar un prompt: `claude "resume projects"` abre una
# sesion NUEVA y le manda esa frase, que es lo que hacia la primera version de
# esto y no era lo que se queria.
#
# Existe como COMANDO y no como argumento suelto en el autostart por una razon
# medida: `wezterm start -- nu -e "claude 'resume projects'"` lanzado con
# Start-Process no ejecuta nada. El argumento llega a nu entero -- se ve en la
# linea de comandos del proceso -- y aun asi el -e se queda sin correr; lanzado
# a mano desde una shell, el mismo comando si funciona. Sin espacios en ese
# argumento no pasa, que es la forma que el autostart ya usaba con `nu -e claude`
# y que lleva funcionando cada arranque.
def claude-proyectos [] { claude --resume "projects" }

# Retoma la conversacion del rice. Lo usa rice-autostart.ps1 para la ventana del
# home. Mismo motivo que claude-proyectos para vivir aqui y no en el autostart:
# `nu -e` con un argumento que lleva espacios no ejecuta nada cuando lo lanza
# Start-Process.
def claude-winrice [] { claude --resume "winrice" }
