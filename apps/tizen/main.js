/* The Tizen shell (ADR 0034): read or ask for the Peregrine address, then replace this page with
 * the paired LAN client. Top-level navigation is the whole design: the cookie, the images, and HLS
 * stay same-origin, and the client that already passes Tizen detection owns everything after this.
 *
 * ES5 syntax on purpose. The engine is Chromium 76 and the packaged app has no transpiler. */
(function () {
  "use strict";

  var STORAGE_KEY = "peregrine.serverUrl";
  var COUNTDOWN_MS = 3000;
  var normalizer = window.PeregrineServerUrl;
  var config = window.PEREGRINE_TV || {};

  var elements = {
    connecting: document.getElementById("connecting"),
    connectingUrl: document.getElementById("connecting-url"),
    change: document.getElementById("change-server"),
    setup: document.getElementById("setup"),
    form: document.getElementById("server-form"),
    input: document.getElementById("server-url"),
    status: document.getElementById("status"),
    error: document.getElementById("error"),
    diagnostics: document.getElementById("diagnostics"),
  };

  var countdown = null;
  var pendingUrl = "";

  function storedUrl() {
    try {
      return window.localStorage.getItem(STORAGE_KEY);
    } catch (error) {
      return null;
    }
  }

  function remember(url) {
    try {
      window.localStorage.setItem(STORAGE_KEY, url);
    } catch (error) {
      // A full or disabled store only costs the next launch's prefill; this launch is fine.
    }
  }

  function cancelCountdown() {
    if (countdown !== null) {
      window.clearTimeout(countdown);
      countdown = null;
    }
  }

  function showSetup(prefill) {
    cancelCountdown();
    elements.connecting.hidden = true;
    elements.setup.hidden = false;
    elements.error.textContent = "";
    elements.status.textContent = "";
    elements.input.value = prefill || "";
    elements.input.focus();
    if (elements.input.value !== "") {
      elements.input.select();
    }
  }

  function showConnecting(url) {
    pendingUrl = url;
    elements.setup.hidden = true;
    elements.connecting.hidden = false;
    elements.connectingUrl.textContent = url;
    elements.change.focus();
  }

  function open(url) {
    elements.status.textContent = "Opening " + url + "...";
    elements.error.textContent = "";
    try {
      window.location.replace(url);
    } catch (error) {
      showSetup(url);
      elements.error.textContent =
        "This screen could not open that address. Try the address again.";
    }
  }

  function connect(raw) {
    var url = normalizer === undefined ? null : normalizer.normalize(raw);
    if (url === null) {
      elements.error.textContent =
        "Enter an http:// or https:// address, like http://192.168.1.50:8642.";
      elements.input.focus();
      return;
    }
    remember(url);
    showConnecting(url);
    open(url);
  }

  function startCountdown(url) {
    showConnecting(url);
    countdown = window.setTimeout(function () {
      countdown = null;
      open(url);
    }, COUNTDOWN_MS);
  }

  function exitApp() {
    try {
      if (window.tizen && window.tizen.application) {
        window.tizen.application.getCurrentApplication().exit();
        return true;
      }
    } catch (error) {
      // A browser without the Tizen application object has nothing to exit.
    }
    return false;
  }

  function supportsHls() {
    var native;
    try {
      native = document.createElement("video").canPlayType("application/vnd.apple.mpegurl") !== "";
    } catch (error) {
      native = false;
    }
    var mse;
    try {
      mse = !!(
        window.MediaSource &&
        window.MediaSource.isTypeSupported &&
        window.MediaSource.isTypeSupported('video/mp4; codecs="avc1.42E01E"')
      );
    } catch (error) {
      mse = false;
    }
    return native || mse;
  }

  function describeEngine() {
    var ua = navigator.userAgent || "";
    var chromium = /Gecko\) ([0-9.]+)/.exec(ua);
    if (chromium) return "Chromium " + chromium[1];
    var tizen = /Tizen ([0-9.]+)/.exec(ua);
    return tizen ? "Tizen " + tizen[1] : "TV browser";
  }

  function renderDiagnostics() {
    elements.diagnostics.textContent =
      describeEngine() +
      " · " +
      window.innerWidth +
      " x " +
      window.innerHeight +
      " · HLS " +
      (supportsHls() ? "yes" : "no");
  }

  function boot() {
    renderDiagnostics();
    var diagnosticLine =
      "peregrine-tv engine=" +
      describeEngine() +
      " screen=" +
      window.innerWidth +
      "x" +
      window.innerHeight +
      " hls=" +
      supportsHls() +
      " ua=" +
      navigator.userAgent;
    console.info(diagnosticLine);
    if (normalizer === undefined) {
      elements.error.textContent = "The app is missing its address parser; reinstall it.";
      return;
    }
    var saved = normalizer.normalize(storedUrl());
    if (saved === null) saved = normalizer.normalize(config.defaultUrl);
    if (saved === null) {
      showSetup("");
      return;
    }
    startCountdown(saved);
  }

  elements.form.addEventListener("submit", function (event) {
    event.preventDefault();
    connect(elements.input.value);
  });

  // The Tizen IME's Done key does not always submit the form; Enter is the same action.
  elements.input.addEventListener("keydown", function (event) {
    if (event.keyCode !== 13) return;
    event.preventDefault();
    connect(elements.input.value);
  });

  elements.change.addEventListener("click", function () {
    showSetup(pendingUrl);
  });

  var lastBack = 0;
  var lastBackHandled = false;

  function handleBack(event) {
    var now = Date.now();
    if (now - lastBack >= 300) {
      lastBack = now;
      lastBackHandled = false;
      if (countdown !== null) {
        showSetup(pendingUrl);
        lastBackHandled = true;
      } else if (!elements.setup.hidden) {
        lastBackHandled = exitApp();
      }
    }
    if (lastBackHandled) event.preventDefault();
  }

  window.addEventListener("tizenhwkey", function (event) {
    if (event.keyName !== "back") return;
    handleBack(event);
  });

  // Some Tizen builds deliver the same press as keyCode 10009; the debounce in handleBack makes
  // the pair one action, and an unhandled press still reaches the platform's own Back.
  window.addEventListener("keydown", function (event) {
    if (event.keyCode !== 10009) return;
    handleBack(event);
  });

  window.addEventListener("resize", renderDiagnostics);

  boot();
})();
