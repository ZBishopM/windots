local wezterm = require 'wezterm'
local config = wezterm.config_builder()

-- Default shell: PowerShell 7+ (runs the fastfetch profile)
-- -NoLogo hides the "PowerShell 7.6.3" banner and the
-- "Loading personal and system profiles took ...ms" line at startup.
config.default_prog = { 'pwsh.exe', '-NoLogo' }

-- Font
config.font = wezterm.font 'JetBrainsMono Nerd Font'
config.font_size = 10.0

-- Open wide enough that fastfetch specs don't wrap to a second line
config.initial_cols = 110
config.initial_rows = 30

return config
