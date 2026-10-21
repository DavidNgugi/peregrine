/* The one rule the shell needs: a server address is an http(s) URL, and a person typing on a
 * remote should not have to type the scheme. Kept as a classic script (no module syntax) because
 * the Tizen shell runs it directly and the packaged app has no bundler. */
(function (root) {
  "use strict";

  var MAX_LENGTH = 2048;

  function normalize(raw) {
    if (typeof raw !== "string") return null;
    var text = raw.replace(/^\s+|\s+$/g, "");
    if (text.length === 0 || text.length > MAX_LENGTH) return null;
    // A bare `192.168.1.50:8642` is the shape a person reads off the desktop; the scheme is
    // implied. Anything that already names a scheme is left for the protocol check below.
    if (text.indexOf("://") === -1) text = "http://" + text;
    if (/\s/.test(text)) return null;
    var parsed;
    try {
      parsed = new URL(text);
    } catch (error) {
      return null;
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    if (parsed.hostname === "") return null;
    // Credentials in a URL would be sent on every request and would reach the desktop's logs.
    if (parsed.username !== "" || parsed.password !== "") return null;
    var path = parsed.pathname === "/" ? "" : parsed.pathname;
    return parsed.protocol + "//" + parsed.host + path;
  }

  root.PeregrineServerUrl = { normalize: normalize };
})(typeof window !== "undefined" ? window : this);
