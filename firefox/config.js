// Firefox DISCARDS THIS FIRST LINE -- keep a comment here, never code.
//
// Renders every Firefox notification with the rice's own toast.
//
// A page's `new Notification()`, a service worker's web push, a finished
// download and a WebExtension's notifications.create() all funnel through one
// XPCOM component: @mozilla.org/alerts-service;1. In
// toolkit/components/build/components.conf that component is declared
// 'overridable': True, which makes Gecko resolve it by contract ID on every call
// instead of holding a static pointer -- so a runtime registerFactory() over the
// contract is genuinely honoured. That is the whole trick: no patched binary, no
// extension, nothing to keep running.
//
// This is not only about looks. nsAlertsService::ShouldShowAlert() asks Windows
// through SHQueryUserNotificationState() and DROPS the alert entirely when a D3D
// fullscreen app is running or Do Not Disturb is on -- notifications during a
// game are simply lost. shadowplay-notify draws a topmost, non-activating
// WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW overlay built for exactly that case, so
// taking the service over is what makes them survive.
//
// The file is one big try/catch on purpose. If anything here throws we never get
// as far as registerFactory and Firefox keeps its stock Windows toasts: the
// failure mode is ugly-but-working, never silence.
//
// Deliberately NOT published as a rice_common IslandEvent (~/.config/island.json):
// the bar's island is one persistent slot for rice-level events (replay saved,
// mic switched), so any chatty tab would own it indefinitely -- and closeAlert()
// has no way to hand it back.
//
// ASCII only in here: Gecko evaluates this file as UTF-8 but the first line is
// eaten before it is decoded, so there is no room for a BOM or a clever header.
// Notification text itself is never touched by that -- it arrives as a JS string.

try {
  const { classes: Cc, interfaces: Ci, results: Cr, utils: Cu } = Components;

  // Hard-coded the way the rest of the repo is; install.ps1's Deploy() rewrites
  // the literal C:\\Users\\obisp into the target machine's home.
  const NOTIFY_EXE = "C:\\Users\\obisp\\dev\\target\\release\\shadowplay-notify.exe";

  // shadowplay-notify's own --x/--y defaults, i.e. top-right of the 1920 monitor
  // in this rice's 1920+2560 layout. Toasts stack downwards so that a burst does
  // not render as one illegible pile.
  const X = 1490;
  const Y = 50;
  const STEP = 112; // 108px window + its 10px outer margin, rounded up
  const SLOTS = 5; // each toast is a separate GPU-accelerated process; cap them

  const HOLD = 6; // seconds on screen
  const HOLD_STICKY = 15; // requireInteraction: we have no sticky toast, so linger
  const ACCENT = "#e0a35c"; // rice_common::theme::ACCENT
  const MAX_BODY = 160; // the toast is a fixed 400x108 overlay; longer text clips

  const EXIT_CLICKED = 10; // shadowplay-notify: "the user clicked me"
  const CONTRACT = "@mozilla.org/alerts-service;1";
  const PREF_ENABLED = "rice.alerts.enabled";

  // Hand-rolled instead of ChromeUtils.generateQI: the fewer chrome globals this
  // file assumes exist in the AutoConfig scope, the fewer ways it can fail
  // closed.
  function qi(ifaces) {
    return function (iid) {
      if (iid.equals(Ci.nsISupports)) {
        return this;
      }
      for (const i of ifaces) {
        if (i && iid.equals(i)) {
          return this;
        }
      }
      throw Cr.NS_ERROR_NO_INTERFACE;
    };
  }

  function safe(fn, fallback) {
    try {
      const v = fn();
      return v === undefined || v === null ? fallback : v;
    } catch (e) {
      return fallback;
    }
  }

  // Escape hatch. This file lives in Program Files and needs UAC to edit, so the
  // off switch has to be reachable without elevation: flip rice.alerts.enabled to
  // false in about:config and the very next notification is a stock Windows
  // toast again -- no restart.
  function enabled() {
    return safe(
      () =>
        Cc["@mozilla.org/preferences-service;1"]
          .getService(Ci.nsIPrefBranch)
          .getBoolPref(PREF_ENABLED, true),
      true
    );
  }

  // Fallback path. Components.classesByID was removed, so the native
  // nsAlertsService is no longer reachable by CID once we own its contract; the
  // OS-level service under its own contract is, and it is the thing
  // nsAlertsService would have called anyway. Resolved lazily -- instantiating
  // alert services during AutoConfig, which runs long before
  // profile-after-change, is asking for trouble.
  let systemSvc;
  function system() {
    if (systemSvc === undefined) {
      systemSvc = safe(
        () =>
          Cc["@mozilla.org/system-alerts-service;1"].getService(Ci.nsIAlertsService),
        null
      );
    }
    return systemSvc;
  }

  function fire(listener, topic, cookie) {
    if (!listener) {
      return;
    }
    try {
      listener.observe(null, topic, cookie);
    } catch (e) {
      Cu.reportError(e);
    }
  }

  // name -> { proc, listener, cookie, pbm, slot }, only while a toast is up.
  const live = new Map();

  // Every name shown this session. getHistory() returning [] tells Gecko that all
  // stored service-worker notifications are gone and it unpersists them, so this
  // has to outlive the toast on screen: our overlay fades after a few seconds,
  // but the notification is only *closed* when the page, the worker or the user
  // says so -- which is closeAlert(), and that is where names are dropped again.
  const shown = new Set();

  function takeSlot() {
    const used = new Set();
    for (const e of live.values()) {
      used.add(e.slot);
    }
    for (let i = 0; i < SLOTS; i++) {
      if (!used.has(i)) {
        return i;
      }
    }
    return 0;
  }

  function argsFor(alert, slot) {
    const title = safe(() => alert.title, "").trim();
    // Only one body line fits under the title, so the origin stands in only when
    // there is no message at all -- better than a clipped third line.
    let body = safe(() => alert.text, "").replace(/\s+/g, " ").trim();
    if (!body) {
      body = safe(() => alert.source, "");
    }
    if (body.length > MAX_BODY) {
      body = body.slice(0, MAX_BODY - 1) + "...";
    }
    const hold = safe(() => alert.requireInteraction, false) ? HOLD_STICKY : HOLD;
    return [
      "--title", title,
      "--body", body,
      "--icon", "bell",
      "--accent", ACCENT,
      "--hold", String(hold),
      "--x", String(X),
      "--y", String(Y + slot * STEP),
    ];
  }

  function spawn(args, onExit) {
    const exe = Cc["@mozilla.org/file/local;1"].createInstance(Ci.nsIFile);
    exe.initWithPath(NOTIFY_EXE);
    const proc = Cc["@mozilla.org/process/util;1"].createInstance(Ci.nsIProcess);
    proc.init(exe);

    const watcher = {
      observe(subject) {
        // nsProcess reports ANY non-zero exit as "process-failed" and only 0 as
        // "process-finished", so the click signal (10) arrives on the failure
        // topic. Read exitValue; never trust the topic.
        onExit(safe(() => subject.QueryInterface(Ci.nsIProcess).exitValue, 0));
      },
      QueryInterface: qi([Ci.nsIObserver]),
    };

    // runwAsync, not runAsync: the narrow variant pushes arguments through the
    // ANSI code page, which mangles accented and emoji titles.
    proc.runwAsync(args, args.length, watcher, false);
    return proc;
  }

  const riceAlerts = {
    // ---- nsIAlertsService -------------------------------------------------
    showAlert(alert, listener) {
      try {
        const name = safe(() => alert.name, "") || "rice-" + Math.random().toString(36).slice(2);
        const cookie = safe(() => alert.cookie, "");

        // The user's own gates stay honoured -- the point of this override is to
        // bypass the *fullscreen* check, not to override someone who asked for
        // quiet or is sharing their screen. Mirrors what nsAlertsService does
        // when it suppresses: report the alert as immediately finished.
        const off = !enabled();
        if (off || this._dnd || this._screenShare) {
          const sys = off ? system() : null;
          if (sys) {
            sys.showAlert(alert, listener);
            return;
          }
          fire(listener, "alertshow", cookie);
          fire(listener, "alertfinished", cookie);
          return;
        }

        // Re-showing an existing tag replaces it, same as a system toast.
        this.closeAlert(name);

        // Past the cap, hand the overflow to Windows rather than opening a sixth
        // overlay process: it at least lands in the Action Center.
        if (live.size >= SLOTS) {
          const sys = system();
          if (sys) {
            sys.showAlert(alert, listener);
            shown.add(name);
            return;
          }
        }

        const slot = takeSlot();
        const proc = spawn(argsFor(alert, slot), code => {
          const e = live.get(name);
          if (!e || e.proc !== proc) {
            return; // already superseded or closed; that path fired alertfinished
          }
          live.delete(name);
          if (code === EXIT_CLICKED) {
            fire(e.listener, "alertclickcallback", e.cookie);
          }
          fire(e.listener, "alertfinished", e.cookie);
        });

        live.set(name, {
          proc,
          listener,
          cookie,
          pbm: safe(() => alert.inPrivateBrowsing, false),
          slot,
        });
        shown.add(name);
        fire(listener, "alertshow", cookie);
      } catch (e) {
        // Anything unexpected at runtime: still show the user something.
        Cu.reportError(e);
        const sys = system();
        if (sys) {
          sys.showAlert(alert, listener);
        }
      }
    },

    // Not in the Firefox 154 IDL (checked against xul.dll's XPT string table,
    // which lists exactly showAlert/closeAlert/getHistory/teardown/pbmTeardown/
    // isFullscreen). Kept as a cheap alias so a build that has it does not fall
    // through to a missing method and throw at the caller.
    showAlertWithCallbacks(alert, listener) {
      this.showAlert(alert, listener);
    },

    closeAlert(name, contextClosed) {
      if (!name) {
        for (const k of [...live.keys()]) {
          this.closeAlert(k, contextClosed);
        }
        return;
      }
      // Dropped from the history too: this is the page/worker/user actually
      // dismissing it, which is exactly when Gecko should stop persisting it.
      shown.delete(name);
      const e = live.get(name);
      if (!e) {
        return;
      }
      live.delete(name);
      try {
        e.proc.kill();
      } catch (err) {
        // already exited between the lookup and here; nothing to kill
      }
      fire(e.listener, "alertfinished", e.cookie);
    },

    getHistory() {
      return [...shown];
    },

    teardown() {
      for (const k of [...live.keys()]) {
        this.closeAlert(k);
      }
    },

    pbmTeardown() {
      for (const [k, e] of [...live]) {
        if (e.pbm) {
          this.closeAlert(k);
        }
      }
    },

    // nsAlertsService derives this from SHQueryUserNotificationState() and
    // callers read it as "the alert cannot be seen right now". Our overlay is
    // topmost and non-activating, so it is visible over a fullscreen game --
    // saying anything but false here would give back the behaviour we removed.
    get isFullscreen() {
      return false;
    },

    // ---- nsIAlertsDoNotDisturb -------------------------------------------
    // about:preferences QIs the alerts service to this to draw its notification
    // settings, and webrtcUI sets suppressForScreenSharing while a screen or
    // window is being shared. Without the interface those callers get
    // NS_NOINTERFACE.
    _dnd: false,
    _screenShare: false,
    get manualDoNotDisturb() {
      return this._dnd;
    },
    set manualDoNotDisturb(v) {
      this._dnd = !!v;
    },
    get suppressForScreenSharing() {
      return this._screenShare;
    },
    set suppressForScreenSharing(v) {
      this._screenShare = !!v;
    },

    QueryInterface: qi([Ci.nsIAlertsService, Ci.nsIAlertsDoNotDisturb]),
  };

  const factory = {
    createInstance(a, b) {
      // nsIFactory.createInstance lost its aOuter argument in Fx 86; accept both
      // shapes so this does not depend on which one the caller compiled against.
      return riceAlerts.QueryInterface(b || a);
    },
    QueryInterface: qi([Ci.nsIFactory]),
  };

  Components.manager
    .QueryInterface(Ci.nsIComponentRegistrar)
    .registerFactory(
      Components.ID("{c4bc4eef-cdec-4956-bbc2-34b47dbba879}"),
      "Rice alerts service",
      CONTRACT,
      factory
    );
} catch (e) {
  // Never rethrow: an AutoConfig failure that propagates can take startup with
  // it, and the worst acceptable outcome here is stock Windows toasts.
  Components.utils.reportError("rice alerts override failed, keeping stock toasts: " + e);
}
