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

-- Render at the monitor's 165Hz. max_fps applies to the OpenGL front end too.
-- NOTE: front end is OpenGL, NOT WebGpu. On Windows + NVIDIA, WebGpu + per-pixel
-- transparency is broken (wezterm issues #3998 / #5790): the window's *initial*
-- surface region stays fully opaque after the tiling WM resizes the window,
-- leaving a grey rectangle (~initial_cols x initial_rows) floating in the
-- terminal until a manual resize recreates the surface. OpenGL composites
-- transparency correctly on resize, so the box never appears.
config.max_fps = 165
config.animation_fps = 165
config.front_end = 'OpenGL'
config.webgpu_power_preference = 'HighPerformance'

-- Clean window: drop the native Windows title bar + min/max/close buttons (the
-- tiling WM moves/sizes windows), keeping only a resize border. And hide
-- WezTerm's own tab bar when there's a single tab, so there's no second header
-- stacked on the first. A thinner tab bar when it does show.
-- Clear FASTFETCH_SHOWN for every new pane so the profile always shows fastfetch
-- in a top-level WezTerm shell, even if wezterm-gui was launched from a process
-- that already had it set. Nested shells still inherit it and stay quiet.
config.set_environment_variables = { FASTFETCH_SHOWN = '' }

config.window_decorations = 'RESIZE'
config.hide_tab_bar_if_only_one_tab = true
config.use_fancy_tab_bar = false
config.window_padding = { left = 6, right = 6, top = 4, bottom = 2 }

-- Transparency: plain per-pixel opacity, read from a file the dynamic-island
-- opacity widget writes. It's on the config reload watch list, so dragging the
-- slider (or editing the file) hot-reloads the terminal opacity live.
-- NOTE: the Windows 11 Acrylic backdrop (win32_system_backdrop) is intentionally
-- NOT used -- with the WebGpu front-end + tiling it left a moving grey rectangle
-- and appeared to wedge GlazeWM. Plain opacity is clean and see-through.
local term_op_file = (os.getenv 'USERPROFILE' or '') .. '\\.config\\term-opacity.txt'
local op = 0.85
local f = io.open(term_op_file, 'r')
if f then
  op = tonumber((f:read '*a' or ''):match '[%d.]+') or 0.85
  f:close()
end
config.window_background_opacity = op
wezterm.add_to_config_reload_watch_list(term_op_file)

return config
