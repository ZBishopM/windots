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

-- Clean window: drop the native Windows title bar + min/max/close buttons (the
-- tiling WM moves/sizes windows), keeping only a resize border. And hide
-- WezTerm's own tab bar when there's a single tab, so there's no second header
-- stacked on the first. A thinner tab bar when it does show.
config.window_decorations = 'RESIZE'
config.hide_tab_bar_if_only_one_tab = true
config.use_fancy_tab_bar = false
config.window_padding = { left = 6, right = 6, top = 4, bottom = 2 }

return config
