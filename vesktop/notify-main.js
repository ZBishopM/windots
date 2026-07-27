// >>> rice-notify (appended to vencordDesktopMain.js by vesktop/apply.ps1)
//
// Main-process half of the Vesktop notification override: spawns
// shadowplay-notify for the renderer, reports how each toast ended, and keeps
// the patch alive across Vencord updates.
//
// Vencord's preload is sandboxed (Vesktop passes sandbox:true for any Vencord
// that supports it), so child_process only exists here. This file is require()d
// by Vesktop's main.js as `require(join(vencordDir,"vencordDesktopMain.js"))`,
// before the browser window is created -- which is also why the self-heal below
// can fix the preload and renderer in time for the current launch.
(function () {
    "use strict";
    try {
        var electron = require("electron");
        var childProcess = require("child_process");
        var fs = require("fs");
        var path = require("path");

        // Hard-coded the way the rest of the repo is; install.ps1's Deploy() and
        // vesktop/apply.ps1 rewrite the literal C:\\Users\\obisp into the target
        // machine's home.
        var EXE = "C:\\Users\\obisp\\dev\\target\\release\\shadowplay-notify.exe";
        var STAGE = "C:\\Users\\obisp\\.config\\vesktop";

        // Escape hatch that needs no restart and no file editing: `touch` this and
        // the very next notification is a stock Windows toast again, because
        // RICE_NOTIFY_SHOW answers false and the renderer falls back. Delete it to
        // get the rice toasts back. Checked per notification on purpose.
        var KILL_SWITCH = path.join(STAGE, "disabled");

        var MARKER = "// >>> rice-notify";
        var FRAGMENTS = {
            "vencordDesktopMain.js": "notify-main.js",
            "vencordDesktopPreload.js": "notify-preload.js",
            "vencordDesktopRenderer.js": "notify-renderer.js"
        };

        // ---------------------------------------------------------------- spawn
        var live = new Map(); // id -> ChildProcess, only while a toast is up

        function reply(event, id, code) {
            try {
                var wc = event.sender;
                if (wc && !wc.isDestroyed()) wc.send("RICE_NOTIFY_EXIT", id, code);
            } catch (e) {}
        }

        // removeHandler first: this module is require()d once, but a duplicate
        // registration throws and would take the whole Vencord main down with it.
        electron.ipcMain.removeHandler("RICE_NOTIFY_SHOW");
        electron.ipcMain.handle("RICE_NOTIFY_SHOW", function (event, id, args) {
            try {
                if (!Array.isArray(args)) return false;
                if (fs.existsSync(KILL_SWITCH)) return false;
                // Checked synchronously so a missing build answers false *now*:
                // spawn() reports ENOENT asynchronously via 'error', by which time
                // the renderer has already handed the object back to Discord.
                if (!fs.existsSync(EXE)) return false;

                var child = childProcess.spawn(EXE, args.map(String), {
                    windowsHide: true,
                    stdio: "ignore"
                });
                child.once("error", function (e) {
                    console.error("[rice-notify] spawn failed", e);
                    live.delete(id);
                    reply(event, id, -1);
                });
                child.once("exit", function (code) {
                    live.delete(id);
                    reply(event, id, code == null ? 0 : code);
                });
                live.set(id, child);
                return true;
            } catch (e) {
                console.error("[rice-notify] show failed", e);
                return false;
            }
        });

        electron.ipcMain.removeHandler("RICE_NOTIFY_CLOSE");
        electron.ipcMain.handle("RICE_NOTIFY_CLOSE", function (_event, id) {
            var child = live.get(id);
            if (!child) return false;
            live.delete(id);
            try {
                child.kill();
            } catch (e) {
                // already exited between the lookup and here; nothing to kill
            }
            return true;
        });

        // ------------------------------------------------------------ self-heal
        // Vencord's standalone updater rewrites all four dist files in place
        // (vencordDesktopMain.js's own `en()` writes to __dirname), and Vesktop's
        // "Force Update Vencord" / "Repair Vencord" does the same. Either wipes
        // this patch, and with autoUpdate on that happens as often as Vencord
        // publishes a build -- potentially daily. So re-append from the staged
        // fragments whenever the files change, and again on the way out, since
        // every restart path goes through app.relaunch()/app.exit() first.
        var dir = __dirname; // == the vencordFiles directory Vesktop loaded us from

        function repatch() {
            Object.keys(FRAGMENTS).forEach(function (target) {
                try {
                    var frag = path.join(STAGE, FRAGMENTS[target]);
                    var dst = path.join(dir, target);
                    if (!fs.existsSync(frag) || !fs.existsSync(dst)) return;
                    if (fs.readFileSync(dst, "utf-8").indexOf(MARKER) !== -1) return;
                    fs.appendFileSync(dst, "\n" + fs.readFileSync(frag, "utf-8"));
                    console.log("[rice-notify] re-applied to " + target);
                } catch (e) {
                    console.error("[rice-notify] could not re-apply to " + target, e);
                }
            });
        }

        repatch(); // fixes preload/renderer in time for THIS launch

        // Debounced: the updater writes four files in one Promise.all, and
        // appending to a file that is about to be truncated loses the append.
        var pending = null;
        try {
            var watcher = fs.watch(dir, function () {
                clearTimeout(pending);
                pending = setTimeout(repatch, 600);
            });
            watcher.on("error", function () {});
        } catch (e) {
            console.error("[rice-notify] could not watch " + dir, e);
        }

        // app.exit() skips before-quit entirely, and Vesktop's VCD_RELAUNCH -- the
        // handler behind Vencord's "restart to apply the update" button -- uses
        // exactly that. Wrapping the two methods catches every restart path;
        // before-quit only covers the plain app.quit() one.
        var app = electron.app;
        ["relaunch", "exit"].forEach(function (m) {
            var orig = app[m].bind(app);
            app[m] = function () {
                try {
                    repatch();
                } catch (e) {}
                return orig.apply(null, arguments);
            };
        });
        app.on("before-quit", function () {
            try {
                repatch();
            } catch (e) {}
        });
    } catch (e) {
        // Never rethrow: this file is Vencord's main process entry point, and
        // taking it down would cost the whole client, not just the toasts.
        console.error("[rice-notify] main hook failed, keeping stock toasts:", e);
    }
})();
// <<< rice-notify
