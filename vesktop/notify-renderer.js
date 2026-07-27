// >>> rice-notify (appended to vencordDesktopRenderer.js by vesktop/apply.ps1)
//
// Replaces window.Notification in Discord's page context so every Vesktop
// notification is drawn by shadowplay-notify instead of the stock blue Windows
// toast.
//
// WHY THIS FILE AND NOT A VENCORD PLUGIN: Vesktop ships Vencord as a prebuilt
// bundle (Data/sessionData/vencordFiles/*.js) downloaded from GitHub releases.
// Userplugins only exist in a from-source Vencord build, so using one would mean
// replacing the whole bundle with a local `pnpm build` tree and hand-updating it
// forever. Appending to the bundle costs nothing at runtime and keeps Vencord's
// own updater working -- see notify-main.js for how the patch survives it.
//
// WHY window.Notification AND NOT electron.Notification: Vesktop's main process
// never constructs a Notification (only electron-updater does, for "update
// ready"). Discord raises everything from the renderer. Proof, from Vesktop's
// own dist/js/renderer.js:
//     Object.getOwnPropertyDescriptor(Notification.prototype,"onclick").set
// -- Vesktop patches the web Notification prototype to focus the window on
// click, which it would not need if notifications came from the main process.
//
// Discord's notification module (webpack chunk 479975) does:
//     let W = window.Notification;            // <- captured ONCE at module eval
//     ...
//     l = new W(title, {icon, body, tag, silent:true});
//     l.onclick = e => { window.focus(); l.close(); ...; opts.onClick?.("") };
// Two consequences drive the design here:
//   1. W is captured at module-eval time, so this shim MUST already be in place
//      before Discord's bundle runs. It is: Vesktop's preload runs
//      webFrame.executeJavaScript(vencordDesktopRenderer.js) -- this file --
//      before any page script.
//   2. Discord reads W.permission through `null != W && "granted" === W.permission`
//      and SUPPRESSES the notification entirely when that is not "granted". The
//      static getter below is load-bearing, not cosmetic.
//
// Deliberately NOT published as a rice_common IslandEvent (~/.config/island.json),
// for the same reason as the Firefox override: the bar's island is one persistent
// slot for rice-level events, and a busy Discord would own it indefinitely.
(function () {
    "use strict";

    var bridge = globalThis.RiceNotify; // contextBridge API from notify-preload.js
    var Native = globalThis.Notification;

    // Fail closed. Half a patch (renderer present, preload/main gone) must give
    // back stock Windows toasts, never silence.
    if (!bridge || typeof Native !== "function") return;

    // shadowplay-notify's own --x/--y defaults: top-right of the 1920 monitor in
    // this rice's 1920+2560 layout. Same origin as the Firefox override, so the
    // two DO overlap if a Firefox and a Discord toast are up at the same moment;
    // consistent placement was judged worth more than never colliding.
    var X = 1490;
    var Y = 50;
    var STEP = 112; // 108px window + its 10px outer margin, rounded up
    var SLOTS = 5; // each toast is a separate GPU-accelerated process; cap them

    var HOLD = 6; // seconds on screen
    var HOLD_STICKY = 15; // requireInteraction: we have no sticky toast, so linger
    var ACCENT = "#e0a35c"; // rice_common::theme::ACCENT
    var ICON = "bell"; // rice_common::ui::icon_glyph -- no chat glyph in the table

    // The toast is a fixed 400x108 overlay and egui wraps rather than ellipsises,
    // so overlong text pushes itself out of the window instead of clipping
    // gracefully. These caps keep it to roughly one title line and two body lines.
    var MAX_TITLE = 44;
    var MAX_BODY = 120;

    var EXIT_CLICKED = 10; // shadowplay-notify: "the user clicked me"

    var H = Symbol("rice.notify"); // per-instance state, invisible to Discord
    var nextId = 1;
    var live = new Map(); // id -> instance, only while a toast is up
    var byTag = new Map(); // tag -> instance, for replace-on-same-tag
    var usedSlots = new Set();

    function takeSlot() {
        for (var i = 0; i < SLOTS; i++) {
            if (!usedSlots.has(i)) {
                usedSlots.add(i);
                return i;
            }
        }
        return -1;
    }

    // Discord message bodies contain newlines; the toast lays out one paragraph.
    function clean(s, max) {
        var t = String(s == null ? "" : s).replace(/\s+/g, " ").trim();
        return t.length > max ? t.slice(0, max - 1) + "…" : t;
    }

    function emit(n, type) {
        try {
            n.dispatchEvent(new Event(type));
        } catch (e) {
            console.error("[rice-notify] listener for " + type + " threw", e);
        }
    }

    function retire(n) {
        var st = n[H];
        if (st.slot >= 0) {
            usedSlots.delete(st.slot);
            st.slot = -1;
        }
        live.delete(st.id);
        if (n.tag && byTag.get(n.tag) === n) byTag.delete(n.tag);
    }

    // Last resort: hand this one notification back to Windows. Used when the
    // helper could not be spawned and when more than SLOTS toasts are up at once
    // -- an Action Center entry beats a sixth overlay process.
    function fallback(n) {
        var st = n[H];
        if (st.closed || st.native) return;
        try {
            var nat = new Native(n.title, {
                body: n.body,
                icon: n.icon,
                tag: n.tag,
                silent: n.silent,
                requireInteraction: n.requireInteraction
            });
            st.native = nat;
            // Routed through our own EventTarget rather than handled here, so the
            // fallback behaves exactly like the normal path: Discord's handler was
            // registered through our patched onclick setter, which Vesktop wrapped
            // with its VesktopNative.win.focus() call. Focusing again here would
            // only double it.
            nat.onclick = function () {
                emit(n, "click");
            };
            nat.onclose = function () {
                if (st.closed) return;
                st.closed = true;
                retire(n);
                emit(n, "close");
            };
            nat.onerror = function () {
                emit(n, "error");
            };
            if (!st.shown) {
                st.shown = true;
                emit(n, "show");
            }
        } catch (e) {
            console.error("[rice-notify] native fallback failed", e);
            emit(n, "error");
        }
    }

    function start(n) {
        var st = n[H];

        // Re-showing an existing tag replaces it, same as a system toast.
        if (n.tag && byTag.has(n.tag)) byTag.get(n.tag).close();

        st.slot = takeSlot();
        if (st.slot < 0) {
            fallback(n);
            return;
        }
        live.set(st.id, n);
        if (n.tag) byTag.set(n.tag, n);

        // Discord's `icon` is a data: URL of the avatar it just circle-cropped on
        // a canvas -- hundreds of KB. It is dropped on purpose: shadowplay-notify
        // draws a Nerd Font glyph, not an image, and a data: URL on the command
        // line would blow the 32767-char Windows limit.
        var args = [
            "--title", clean(n.title, MAX_TITLE),
            "--body", clean(n.body, MAX_BODY),
            "--icon", ICON,
            "--accent", ACCENT,
            "--hold", String(n.requireInteraction ? HOLD_STICKY : HOLD),
            "--x", String(X),
            "--y", String(Y + st.slot * STEP)
        ];

        Promise.resolve(bridge.show(st.id, args)).then(
            function (ok) {
                if (!ok) {
                    // Helper missing, or the kill switch file is present.
                    retire(n);
                    fallback(n);
                    return;
                }
                if (!st.closed && !st.shown) {
                    st.shown = true;
                    emit(n, "show");
                }
            },
            function (e) {
                console.error("[rice-notify] show failed", e);
                retire(n);
                fallback(n);
            }
        );
    }

    // EventTarget gives addEventListener/dispatchEvent for free, which is what
    // the onclick/onclose/onshow accessors below are built on.
    var RiceNotification = class Notification extends EventTarget {
        constructor(title, options) {
            super();
            var o = options || {};
            this[H] = { id: nextId++, slot: -1, closed: false, shown: false, native: null, handlers: {} };

            this.title = String(title == null ? "" : title);
            this.body = o.body == null ? "" : String(o.body);
            this.icon = o.icon || "";
            this.tag = o.tag == null ? "" : String(o.tag);
            this.data = o.data;
            this.silent = !!o.silent;
            this.requireInteraction = !!o.requireInteraction;
            this.dir = o.dir || "auto";
            this.lang = o.lang || "";
            this.badge = o.badge || "";
            this.image = o.image || "";
            this.timestamp = o.timestamp || Date.now();
            this.actions = [];

            // Discord wraps `new W(...)` in a try/catch that silently DROPS the
            // notification, so throwing here loses it with no trace.
            try {
                start(this);
            } catch (e) {
                console.error("[rice-notify] falling back to the stock toast", e);
                fallback(this);
            }
        }

        close() {
            var st = this[H];
            if (st.closed) return;
            st.closed = true;
            try {
                if (st.native) st.native.close();
                else bridge.close(st.id);
            } catch (e) {}
            retire(this);
            emit(this, "close");
        }
    };

    // onclick/onclose/onshow/onerror as PROTOTYPE accessors, not instance fields.
    // Vesktop's renderer does
    //   Object.getOwnPropertyDescriptor(Notification.prototype,"onclick").set
    // and dies with a TypeError -- taking the rest of Vesktop's renderer script
    // (settings UI, badge counts, taskbar flashing) with it -- if that descriptor
    // has no setter. It then redefines onclick with a set-only descriptor, which
    // is why nothing below ever reads `this.onclick` back.
    ["click", "close", "show", "error"].forEach(function (type) {
        var key = "on" + type;
        Object.defineProperty(RiceNotification.prototype, key, {
            configurable: true,
            enumerable: true,
            get: function () {
                return (this[H] && this[H].handlers[key]) || null;
            },
            set: function (fn) {
                var st = this[H];
                if (!st) return;
                if (st.handlers[key]) this.removeEventListener(type, st.handlers[key]);
                st.handlers[key] = typeof fn === "function" ? fn : null;
                if (st.handlers[key]) this.addEventListener(type, st.handlers[key]);
            }
        });
    });

    // Load-bearing: Discord suppresses every notification unless this is
    // "granted". Delegated to the real implementation rather than hard-coded, so
    // that revoking permission still works.
    Object.defineProperty(RiceNotification, "permission", {
        configurable: true,
        get: function () {
            try {
                return Native.permission;
            } catch (e) {
                return "granted";
            }
        }
    });
    RiceNotification.requestPermission = function (cb) {
        var p = Promise.resolve().then(function () {
            return Native.requestPermission ? Native.requestPermission() : "granted";
        });
        if (typeof cb === "function") {
            p.then(cb, function () {
                cb("denied");
            });
        }
        return p;
    };
    RiceNotification.maxActions = Native.maxActions || 0;

    // The helper's exit code says how the toast ended: 10 = the user clicked it,
    // anything else = it faded. The click path is what makes Discord jump to the
    // channel (its own onclick calls opts.onClick) and what makes Vesktop's
    // onclick wrapper raise the window.
    bridge.onExit(function (id, code) {
        var n = live.get(id);
        if (!n) return;
        var st = n[H];
        retire(n);
        if (code === EXIT_CLICKED) emit(n, "click"); // Discord's handler calls close()
        if (!st.closed) {
            st.closed = true;
            emit(n, "close");
        }
    });

    Object.defineProperty(globalThis, "Notification", {
        value: RiceNotification,
        writable: true,
        configurable: true
    });
})();
// <<< rice-notify
