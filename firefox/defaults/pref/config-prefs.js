// Turns on AutoConfig so that ../../config.js runs. Files in defaults/pref/ are
// read at startup, before any profile pref, which is the only hook early enough
// to name a config file -- setting these from user.js is too late.
//
// pref(), not user_pref(): this is a default-branch context.
pref("general.config.filename", "config.js");

// 0 = the config file is plain text. Any other value means Firefox expects it
// byte-shifted by that amount (13 was the historical "obscure it from the user"
// default) and an unshifted file would read as garbage and silently do nothing.
pref("general.config.obscure_value", 0);

// The default AutoConfig sandbox exposes only the pref helpers (pref, lockPref,
// getPref...). config.js has to register an XPCOM factory, which needs the real
// Components object, and that only exists with the sandbox off.
pref("general.config.sandbox_enabled", false);
