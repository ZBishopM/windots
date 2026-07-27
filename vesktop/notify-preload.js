// >>> rice-notify (appended to vencordDesktopPreload.js by vesktop/apply.ps1)
//
// The bridge between the Notification shim (notify-renderer.js, main world) and
// the process spawner (notify-main.js, main process).
//
// This step is not avoidable. Vesktop creates its window with sandbox:true --
// dist/js/main.js does `sandbox: hE()`, where hE() is true for any Vencord new
// enough to expose VencordGetRendererCss, which this one is -- so the preload
// gets Electron's cut-down require() with electron/renderer and nothing else.
// child_process is simply not reachable from here, hence the IPC round trip.
//
// Evaluated either as a CommonJS module or as the body of a Function() with
// `require` passed in (Vesktop picks depending on __dirname), so plain
// `require("electron/renderer")` -- exactly what the Vencord preload above uses
// -- works in both shapes.
(function () {
    "use strict";
    try {
        var electron = require("electron/renderer");

        var onExit = null;
        electron.ipcRenderer.on("RICE_NOTIFY_EXIT", function (_e, id, code) {
            try {
                if (onExit) onExit(id, code);
            } catch (err) {
                console.error("[rice-notify] exit callback threw", err);
            }
        });

        electron.contextBridge.exposeInMainWorld("RiceNotify", {
            // Resolves false when the helper is missing or the kill switch file
            // exists; the renderer falls back to a stock Windows toast on false.
            show: function (id, args) {
                return electron.ipcRenderer.invoke("RICE_NOTIFY_SHOW", id, args);
            },
            close: function (id) {
                return electron.ipcRenderer.invoke("RICE_NOTIFY_CLOSE", id);
            },
            onExit: function (cb) {
                onExit = cb;
            }
        });
    } catch (e) {
        // Never rethrow: this file is Vencord's preload, and taking it down would
        // cost the whole client, not just the toasts.
        console.error("[rice-notify] preload bridge failed, keeping stock toasts:", e);
    }
})();
// <<< rice-notify
