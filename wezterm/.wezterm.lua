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

-- Render at the monitor's 165Hz (WezTerm caps at 60 by default) on the dedicated
-- GPU (RTX 4070) via WebGpu instead of the default OpenGL. This is what makes
-- cava (and everything) actually move at 165fps.
config.max_fps = 165
config.animation_fps = 165
config.front_end = 'WebGpu'
config.webgpu_power_preference = 'HighPerformance'

return config
