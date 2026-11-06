import { readFileSync } from "node:fs";
import vm from "node:vm";
import { describe, expect, it } from "vitest";

// The shipped file is a classic script, so the test runs that exact source.
const source = readFileSync(new URL("../reachability.js", import.meta.url), "utf8");
const context = { window: {}, URL };
vm.runInNewContext(source, context);
const { deviceAddress, probe, probeUrl, scan, scanCandidates, TIMEOUT_MS } =
  context.window.PeregrineReachability;

function fakeEnvironment() {
  const environment = {
    image: { onload: null, onerror: null, src: null },
    timeouts: [],
    cleared: [],
    createImage: () => environment.image,
    setTimeout: (callback, delay) => environment.timeouts.push({ callback, delay }),
    clearTimeout: (timer) => environment.cleared.push(timer),
    now: () => 1234,
  };
  return environment;
}

function run() {
  const environment = fakeEnvironment();
  const answers = [];
  const cancel = probe("http://192.168.1.50:8642", environment, (reached) => answers.push(reached));
  return { environment, answers, cancel };
}

describe("probeUrl", () => {
  it("names the probe under the server, past any trailing slash or proxy path", () => {
    expect(probeUrl("http://192.168.1.50:8642", 7)).toBe(
      "http://192.168.1.50:8642/peregrine-probe.png?t=7",
    );
    expect(probeUrl("https://example.test/lan/", 7)).toBe(
      "https://example.test/lan/peregrine-probe.png?t=7",
    );
  });
});

describe("probe", () => {
  it("answers reached when the image loads", () => {
    const { environment, answers } = run();
    expect(environment.image.src).toContain("/peregrine-probe.png?t=1234");
    environment.image.onload();
    expect(answers).toEqual([true]);
  });

  it("answers not reached when nothing, or something else, answers", () => {
    const { environment, answers } = run();
    environment.image.onerror();
    expect(answers).toEqual([false]);
  });

  it("gives up at the timeout, which a silent address needs", () => {
    const { environment, answers } = run();
    expect(environment.timeouts[0].delay).toBe(TIMEOUT_MS);
    environment.timeouts[0].callback();
    expect(answers).toEqual([false]);
    expect(environment.image.src).toBe("");
  });

  it("answers once when a load races the timeout", () => {
    const { environment, answers } = run();
    const { onload } = environment.image;
    onload();
    onload();
    environment.timeouts[0].callback();
    expect(answers).toEqual([true]);
  });

  it("stays silent after a cancel, so an abandoned attempt cannot navigate", () => {
    const { environment, answers, cancel } = run();
    const { onload } = environment.image;
    const timeout = environment.timeouts[0].callback;
    cancel();
    onload();
    timeout();
    expect(answers).toEqual([]);
  });
});

describe("scanCandidates", () => {
  it("walks the saved subnet nearest the old address, skipping it", () => {
    const hosts = scanCandidates(null, "http://192.168.100.40:8642");
    expect(hosts).toHaveLength(253);
    expect(hosts.slice(0, 3)).toEqual([
      "http://192.168.100.39:8642",
      "http://192.168.100.41:8642",
      "http://192.168.100.38:8642",
    ]);
    expect(hosts).not.toContain("http://192.168.100.40:8642");
    expect(hosts).toContain("http://192.168.100.1:8642");
    expect(hosts).toContain("http://192.168.100.254:8642");
  });

  it("prefers the television's own subnet and never asks the television", () => {
    const hosts = scanCandidates("10.0.5.3", "http://192.168.100.40:8642");
    expect(hosts).toHaveLength(253);
    expect(hosts[0]).toBe("http://10.0.5.2:8642");
    expect(hosts).not.toContain("http://10.0.5.3:8642");
  });

  it("uses the default port on first run and keeps a saved scheme, port, and path", () => {
    expect(scanCandidates("192.168.1.9", null)[0]).toBe("http://192.168.1.8:8642");
    expect(scanCandidates(null, "https://192.168.1.9:9000/lan")[0]).toBe(
      "https://192.168.1.8:9000/lan",
    );
    expect(scanCandidates(null, "http://192.168.1.9")[0]).toBe("http://192.168.1.8");
  });

  it("uses the television's subnet when the saved address is a name", () => {
    expect(scanCandidates("192.168.1.9", "http://studio.local:9000")[0]).toBe(
      "http://192.168.1.8:9000",
    );
  });

  it("never scans outside a private range or without a subnet", () => {
    expect(scanCandidates(null, null)).toEqual([]);
    expect(scanCandidates(null, "http://studio.local:8642")).toEqual([]);
    expect(scanCandidates("8.8.8.8", "http://1.2.3.4:8642")).toEqual([]);
    expect(scanCandidates(null, "http://172.32.0.1:8642")).toEqual([]);
    expect(scanCandidates(null, "http://192.168.1.999:8642")).toEqual([]);
    expect(scanCandidates(null, "not a url")).toEqual([]);
    expect(scanCandidates(null, "http://172.31.0.1:8642")).toHaveLength(253);
  });
});

function scanEnvironment() {
  const images = [];
  return {
    images,
    createImage: () => {
      const image = { onload: null, onerror: null, src: null };
      images.push(image);
      return image;
    },
    setTimeout: () => 0,
    clearTimeout: () => {},
    now: () => 1,
  };
}

const live = (environment) => environment.images.filter((image) => image.onerror !== null);
const urls = Array.from({ length: 60 }, (_, index) => "http://10.0.0." + String(index + 1));

describe("scan", () => {
  it("answers null at once with nothing to scan", () => {
    const answers = [];
    scan([], scanEnvironment(), (found) => answers.push(found));
    expect(answers).toEqual([null]);
  });

  it("holds its concurrency and refills as probes fail", () => {
    const environment = scanEnvironment();
    scan(urls, environment, () => {});
    // Past Chromium's limit of about ten, queued candidates time out without being asked.
    expect(live(environment)).toHaveLength(8);
    live(environment)[0].onerror();
    expect(live(environment)).toHaveLength(8);
    expect(environment.images).toHaveLength(9);
  });

  it("answers the first listener once and abandons the rest", () => {
    const environment = scanEnvironment();
    const answers = [];
    scan(urls, environment, (found) => answers.push(found));
    const [first, second] = live(environment);
    const lateAnswer = second.onload;
    first.onload();
    lateAnswer();
    expect(answers).toEqual(["http://10.0.0.1"]);
    expect(live(environment)).toHaveLength(0);
  });

  it("answers null once every candidate has failed", () => {
    const environment = scanEnvironment();
    const answers = [];
    scan(urls, environment, (found) => answers.push(found));
    while (live(environment).length > 0) live(environment)[0].onerror();
    expect(environment.images).toHaveLength(60);
    expect(answers).toEqual([null]);
  });

  it("stays silent after a cancel", () => {
    const environment = scanEnvironment();
    const answers = [];
    const cancel = scan(urls, environment, (found) => answers.push(found));
    const { onload } = live(environment)[0];
    cancel();
    onload();
    expect(answers).toEqual([]);
    expect(live(environment)).toHaveLength(0);
  });
});

describe("deviceAddress", () => {
  const environment = { setTimeout: (callback) => callback, clearTimeout: () => {} };
  const answer = (systeminfo, env = environment) => {
    const answers = [];
    deviceAddress(systeminfo, env, (address) => answers.push(address));
    return answers;
  };

  it("answers null without the platform object", () => {
    expect(answer(undefined)).toEqual([null]);
  });

  it("reads Wi-Fi first, then Ethernet", () => {
    const networks = { WIFI_NETWORK: { ipAddress: "" }, ETHERNET_NETWORK: { ipAddress: "10.0.0.5" } };
    const systeminfo = { getPropertyValue: (name, success) => success(networks[name]) };
    expect(answer(systeminfo)).toEqual(["10.0.0.5"]);
  });

  it("answers null when the platform refuses or throws", () => {
    expect(answer({ getPropertyValue: (_, __, failure) => failure() })).toEqual([null]);
    expect(
      answer({
        getPropertyValue: () => {
          throw new Error("denied");
        },
      }),
    ).toEqual([null]);
  });

  it("gives up when the platform never calls back", () => {
    let expire = () => {};
    const waiting = {
      setTimeout: (callback) => {
        expire = callback;
      },
      clearTimeout: () => {},
    };
    const answers = answer({ getPropertyValue: () => {} }, waiting);
    expect(answers).toEqual([]);
    expire();
    expect(answers).toEqual([null]);
  });
});
