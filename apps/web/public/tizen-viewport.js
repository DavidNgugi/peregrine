/* global window, document */
/* The listener serves this page with `width=1920, height=1080` for a Tizen User-Agent, because a
 * viewport meta changed by script after the parser has read it does not resize the engine's
 * canvas. This script runs before the first paint to add the Tizen class, and probes the one
 * feature the layout cannot assume: flex `gap`. A desktop or phone returns immediately. */
(function () {
  "use strict";
  var ua = String(window.navigator && window.navigator.userAgent ? window.navigator.userAgent : "");
  if (!/Tizen/i.test(ua)) return;
  var root = document.documentElement;
  root.classList.add("tizen");

  // Flex `gap` arrived in Chromium 84; the Tizen 6.0 class this app targets is older. Probe it
  // rather than assume, because a later Tizen set does support it and would double the spacing
  // if every gap were replaced by a margin.
  try {
    var probe = document.createElement("div");
    probe.style.position = "absolute";
    probe.style.left = "-9999px";
    probe.style.top = "-9999px";
    probe.style.display = "flex";
    probe.style.flexDirection = "column";
    probe.style.rowGap = "1px";
    var child = document.createElement("div");
    child.style.height = "1px";
    probe.appendChild(child);
    probe.appendChild(child.cloneNode(false));
    root.appendChild(probe);
    if (probe.scrollHeight !== 3) root.classList.add("no-flex-gap");
    root.removeChild(probe);
  } catch {
    root.classList.add("no-flex-gap");
  }
})();
