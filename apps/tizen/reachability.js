/* Is a Peregrine listener answering at this address? The shell asks before it navigates, because
 * a top-level navigation to a dead address lands on the engine's error page, where no code of ours
 * runs and the remote has nothing to do.
 *
 * The question is an image load rather than a fetch: there is no CORS surface to read a response
 * through (ADR 0034), an opaque fetch resolves for any server that happens to hold the address, and
 * a script tag would run a stranger's code with this app's privileges. An image proves the same
 * thing and can execute nothing. Classic script, ES5: the shell has no bundler or transpiler. */
(function (root) {
  "use strict";

  var PROBE_PATH = "/peregrine-probe.png";
  var TIMEOUT_MS = 6000;
  var DEFAULT_PORT = "8642";
  // Chromium sends about ten image requests at once and queues the rest while their timers run, so
  // going past that makes candidates time out without ever being asked; a live server was measured
  // missed that way at 24. The timeout is what a dead address costs, so these bound a full /24 to
  // about 40 seconds, and the search starts beside the old address, where a new lease usually lands.
  var SCAN_TIMEOUT_MS = 1200;
  var SCAN_CONCURRENCY = 8;
  var DEVICE_TIMEOUT_MS = 1500;

  /** The probe's URL; the query defeats the engine's image cache, which would answer for a server
   * that has since gone away. */
  function probeUrl(url, nowMs) {
    return url.replace(/\/+$/, "") + PROBE_PATH + "?t=" + nowMs;
  }

  /** Calls `done(reached)` once, and returns a function that abandons the probe silently. */
  function probe(url, environment, done, timeoutMs) {
    var image = environment.createImage();
    var timer = null;
    var settled = false;

    function release() {
      settled = true;
      environment.clearTimeout(timer);
      image.onload = null;
      image.onerror = null;
      image.src = "";
    }

    function finish(reached) {
      if (settled) return;
      release();
      done(reached);
    }

    image.onload = function () {
      finish(true);
    };
    image.onerror = function () {
      finish(false);
    };
    timer = environment.setTimeout(function () {
      finish(false);
    }, timeoutMs === undefined ? TIMEOUT_MS : timeoutMs);
    image.src = probeUrl(url, environment.now());

    return function cancel() {
      if (!settled) release();
    };
  }

  /** The four octets of a private IPv4 address, or null: a scan never leaves the LAN (R33). */
  function privateOctets(host) {
    var match = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host || "");
    if (match === null) return null;
    var octets = match.slice(1).map(Number);
    if (octets.some(function (octet) { return octet > 255; })) return null;
    var isPrivate =
      octets[0] === 10 ||
      (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) ||
      (octets[0] === 192 && octets[1] === 168);
    return isPrivate ? octets : null;
  }

  function parseUrl(url) {
    try {
      return url ? new URL(url) : null;
    } catch (error) {
      return null;
    }
  }

  function sameSubnet(left, right) {
    return (
      left !== null &&
      right !== null &&
      left[0] === right[0] &&
      left[1] === right[1] &&
      left[2] === right[2]
    );
  }

  /**
   * The addresses a listener could have moved to, nearest the old one first.
   *
   * The television's own subnet wins, because it is true now; the saved address is the fallback
   * for a set that will not say. Only a /24 is walked. The port and scheme come from the saved
   * address, and neither the television nor the address that just failed is asked again.
   */
  function scanCandidates(deviceIp, savedUrl) {
    var saved = parseUrl(savedUrl);
    var savedOctets = saved === null ? null : privateOctets(saved.hostname);
    var deviceOctets = privateOctets(deviceIp);
    var subnet = deviceOctets || savedOctets;
    if (subnet === null) return [];
    var center = sameSubnet(savedOctets, subnet) ? savedOctets[3] : subnet[3];
    var skipSaved = sameSubnet(savedOctets, subnet) ? savedOctets[3] : null;
    var skipDevice = deviceOctets === null ? null : deviceOctets[3];
    var hosts = [];
    for (var host = 1; host <= 254; host += 1) {
      if (host !== skipSaved && host !== skipDevice) hosts.push(host);
    }
    hosts.sort(function (left, right) {
      return Math.abs(left - center) - Math.abs(right - center) || left - right;
    });
    var protocol = saved === null ? "http:" : saved.protocol;
    var port = saved === null ? DEFAULT_PORT : saved.port;
    var path = saved === null || saved.pathname === "/" ? "" : saved.pathname;
    var prefix = protocol + "//" + subnet.slice(0, 3).join(".") + ".";
    return hosts.map(function (last) {
      return prefix + last + (port === "" ? "" : ":" + port) + path;
    });
  }

  /** Calls `done(url)` with the first candidate that answers, or `done(null)`; returns a cancel. */
  function scan(candidates, environment, done) {
    var next = 0;
    var active = 0;
    var finished = false;
    var cancels = [];

    function stop() {
      finished = true;
      cancels.forEach(function (cancel) {
        cancel();
      });
    }

    function launch(url) {
      active += 1;
      cancels.push(
        probe(
          url,
          environment,
          function (reached) {
            active -= 1;
            if (finished) return;
            if (reached) {
              stop();
              done(url);
              return;
            }
            fill();
          },
          SCAN_TIMEOUT_MS,
        ),
      );
    }

    function fill() {
      while (!finished && active < SCAN_CONCURRENCY && next < candidates.length) {
        next += 1;
        launch(candidates[next - 1]);
      }
      if (!finished && active === 0 && next === candidates.length) {
        finished = true;
        done(null);
      }
    }

    fill();
    return stop;
  }

  /**
   * The television's own address, or `done(null)` when the set will not say.
   *
   * The timer is there because a platform call that never answers must not hold up a screen.
   */
  function deviceAddress(systeminfo, environment, done) {
    if (!systeminfo) {
      done(null);
      return;
    }
    var settled = false;
    var timer = null;

    function finish(address) {
      if (settled) return;
      settled = true;
      environment.clearTimeout(timer);
      done(address);
    }

    function read(property, otherwise) {
      try {
        systeminfo.getPropertyValue(
          property,
          function (network) {
            if (network && network.ipAddress) finish(network.ipAddress);
            else otherwise();
          },
          otherwise,
        );
      } catch (error) {
        otherwise();
      }
    }

    timer = environment.setTimeout(function () {
      finish(null);
    }, DEVICE_TIMEOUT_MS);
    read("WIFI_NETWORK", function () {
      read("ETHERNET_NETWORK", function () {
        finish(null);
      });
    });
  }

  function browserEnvironment() {
    return {
      createImage: function () {
        return new Image();
      },
      setTimeout: function (callback, delay) {
        return window.setTimeout(callback, delay);
      },
      clearTimeout: function (timer) {
        window.clearTimeout(timer);
      },
      now: function () {
        return Date.now();
      },
    };
  }

  root.PeregrineReachability = {
    TIMEOUT_MS: TIMEOUT_MS,
    probeUrl: probeUrl,
    probe: probe,
    scanCandidates: scanCandidates,
    scan: scan,
    deviceAddress: deviceAddress,
    browserEnvironment: browserEnvironment,
  };
})(typeof window !== "undefined" ? window : this);
