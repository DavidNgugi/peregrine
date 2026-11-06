/* The Tizen shell (ADR 0034): read or ask for the Peregrine address, then replace this page with
 * the paired LAN client. Top-level navigation is the whole design: the cookie, the images, and HLS
 * stay same-origin, and the client that already passes Tizen detection owns everything after this.
 * Navigating is also the one step this code cannot come back from, so the address is proved first:
 * a dead one stays here, on a screen with a way out, instead of on the engine's error page.
 *
 * ES5 syntax on purpose. The engine is Chromium 76 and the packaged app has no transpiler. */
(function () {
  "use strict";

  var STORAGE_KEY = "peregrine.serverUrl";
  var COUNTDOWN_MS = 3000;
  var RETRY_SECONDS = 10;
  var CHANGE_HINT = "Press BACK to change the server.";
  var LOOKING = "Looking for Peregrine on your network...";
  var normalizer = window.PeregrineServerUrl;
  var reachability = window.PeregrineReachability;
  var config = window.PEREGRINE_TV || {};

  var elements = {
    connecting: document.getElementById("connecting"),
    connectingUrl: document.getElementById("connecting-url"),
    connectingHint: document.getElementById("connecting-hint"),
    change: document.getElementById("change-server"),
    unreachable: document.getElementById("unreachable"),
    unreachableUrl: document.getElementById("unreachable-url"),
    retry: document.getElementById("retry"),
    changeAddress: document.getElementById("change-address"),
    retryStatus: document.getElementById("retry-status"),
    setup: document.getElementById("setup"),
    form: document.getElementById("server-form"),
    input: document.getElementById("server-url"),
    useFound: document.getElementById("use-found"),
    status: document.getElementById("status"),
    error: document.getElementById("error"),
    diagnostics: document.getElementById("diagnostics"),
  };

  var countdown = null;
  var pendingUrl = "";
  var cancelProbe = null;
  var retryTimer = null;
  var scanDue = true;
  var foundUrl = "";
  var deviceIp = null;

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

  function stopAttempts() {
    if (countdown !== null) {
      window.clearTimeout(countdown);
      countdown = null;
    }
    if (retryTimer !== null) {
      window.clearTimeout(retryTimer);
      retryTimer = null;
    }
    if (cancelProbe !== null) {
      cancelProbe();
      cancelProbe = null;
    }
  }

  function showSetup(prefill) {
    stopAttempts();
    elements.connecting.hidden = true;
    elements.unreachable.hidden = true;
    elements.setup.hidden = false;
    elements.error.textContent = "";
    elements.status.textContent = "";
    elements.useFound.hidden = true;
    elements.input.value = prefill || "";
    elements.input.focus();
    if (elements.input.value !== "") {
      elements.input.select();
    }
  }

  function showConnecting(url, hint) {
    pendingUrl = url;
    elements.connectingHint.textContent = hint || CHANGE_HINT;
    elements.setup.hidden = true;
    elements.unreachable.hidden = true;
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

  // The desktop waking up is the common way this screen ends, so it keeps trying by itself and a
  // person only has to act when the address itself is wrong.
  function countDownToRetry(url, seconds) {
    if (seconds === 0) {
      attempt(url);
      return;
    }
    elements.retryStatus.textContent = "Trying again in " + seconds + "s";
    retryTimer = window.setTimeout(function () {
      retryTimer = null;
      countDownToRetry(url, seconds - 1);
    }, 1000);
  }

  // Finding the listener is one sweep per failure rather than part of the ten-second retry: the
  // saved address is the likely answer, and a television should not walk the network forever.
  function findServer(savedUrl, done) {
    var environment = reachability.browserEnvironment();
    var systeminfo = window.tizen ? window.tizen.systeminfo : null;
    var cancelled = false;
    var cancelScan = null;
    cancelProbe = function () {
      cancelled = true;
      if (cancelScan !== null) cancelScan();
    };
    reachability.deviceAddress(systeminfo, environment, function (address) {
      if (cancelled) return;
      if (address !== null) {
        deviceIp = address;
        renderDiagnostics();
      }
      var candidates = reachability.scanCandidates(address, savedUrl);
      cancelScan = reachability.scan(candidates, environment, function (found) {
        cancelProbe = null;
        done(found);
      });
    });
  }

  function showUnreachable(url) {
    var entering = elements.unreachable.hidden;
    elements.setup.hidden = true;
    elements.connecting.hidden = true;
    elements.unreachable.hidden = false;
    elements.unreachable.classList.remove("trying");
    elements.unreachableUrl.textContent = url;
    // A retry that fails again must not pull focus off the button the person moved to.
    if (entering) elements.retry.focus();
    if (!scanDue) {
      countDownToRetry(url, RETRY_SECONDS);
      return;
    }
    scanDue = false;
    elements.unreachable.classList.add("trying");
    elements.retryStatus.textContent = LOOKING;
    findServer(url, function (found) {
      elements.unreachable.classList.remove("trying");
      if (found !== null) {
        startCountdown(found, "Found Peregrine at a new address. " + CHANGE_HINT);
        return;
      }
      countDownToRetry(url, RETRY_SECONDS);
    });
  }

  function attempt(url) {
    stopAttempts();
    pendingUrl = url;
    if (!elements.unreachable.hidden) {
      elements.unreachable.classList.add("trying");
      elements.retryStatus.textContent = "Trying again...";
    }
    cancelProbe = reachability.probe(url, reachability.browserEnvironment(), function (reached) {
      cancelProbe = null;
      if (reached) {
        remember(url);
        open(url);
      } else {
        showUnreachable(url);
      }
    });
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
    scanDue = true;
    showConnecting(url);
    attempt(url);
  }

  // Typing on a remote is the worst part of first run, so the sweep offers its answer beside the
  // form rather than connecting underneath someone who may be typing a different address.
  function offerFoundServer() {
    elements.status.textContent = LOOKING;
    findServer(null, function (found) {
      elements.status.textContent = "";
      if (found === null) return;
      foundUrl = found;
      elements.useFound.textContent = "Use " + found;
      elements.useFound.hidden = false;
      if (elements.input.value === "") elements.useFound.focus();
    });
  }

  function startCountdown(url, hint) {
    showConnecting(url, hint);
    countdown = window.setTimeout(function () {
      countdown = null;
      attempt(url);
    }, COUNTDOWN_MS);
  }

  // Tizen delivers only the D-pad, OK, and Back until these are registered. The registration
  // belongs to the application rather than the document, so doing it here, where the `tizen`
  // object certainly exists, is what lets the player on the navigated page receive them.
  var MEDIA_KEYS = [
    "MediaPlayPause",
    "MediaPlay",
    "MediaPause",
    "MediaStop",
    "MediaFastForward",
    "MediaRewind",
  ];

  function registerMediaKeys() {
    var input = window.tizen && window.tizen.tvinputdevice;
    if (!input) return;
    try {
      if (typeof input.registerKeyBatch === "function") {
        input.registerKeyBatch(MEDIA_KEYS);
        return;
      }
    } catch (error) {
      // Fall through to one key at a time, so one key the model lacks does not cost the rest.
    }
    for (var i = 0; i < MEDIA_KEYS.length; i += 1) {
      try {
        input.registerKey(MEDIA_KEYS[i]);
      } catch (error) {
        // A model without this key keeps the others.
      }
    }
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
      (supportsHls() ? "yes" : "no") +
      (deviceIp === null ? "" : " · TV " + deviceIp);
  }

  function boot() {
    registerMediaKeys();
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
    if (normalizer === undefined || reachability === undefined) {
      elements.error.textContent = "The app is missing its address parser; reinstall it.";
      return;
    }
    var saved = normalizer.normalize(storedUrl());
    if (saved === null) saved = normalizer.normalize(config.defaultUrl);
    if (saved === null) {
      showSetup("");
      offerFoundServer();
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

  elements.retry.addEventListener("click", function () {
    scanDue = true;
    attempt(pendingUrl);
  });

  elements.useFound.addEventListener("click", function () {
    connect(foundUrl);
  });

  elements.changeAddress.addEventListener("click", function () {
    showSetup(pendingUrl);
  });

  elements.unreachable.addEventListener("keydown", function (event) {
    if (event.keyCode === 37) elements.retry.focus();
    if (event.keyCode === 39) elements.changeAddress.focus();
  });

  var lastBack = 0;
  var lastBackHandled = false;

  function handleBack(event) {
    var now = Date.now();
    if (now - lastBack >= 300) {
      lastBack = now;
      lastBackHandled = false;
      if (!elements.connecting.hidden) {
        showSetup(pendingUrl);
        lastBackHandled = true;
      } else {
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
