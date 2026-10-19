import { commands, unwrap } from "@peregrine/ipc-client";

/** The port the TV client is offered on; the backend accepts any, this is the one the UI shows. */
export const WEB_PORT = 8642;

/** The renderer's own autostart choice, which the backend does not need to know about. */
export const WEB_AUTO_KEY = "peregrine.web.autoStart";

/** The renderer's own choice to keep the machine awake while the surface is on. */
export const WEB_KEEP_AWAKE_KEY = "peregrine.web.keepAwake";

/** The event the shell pushes when the listener moves to the machine's new address. */
export const WEB_STATUS_EVENT = "web://status";

/**
 * Starts the LAN listener on the detected address and the name this machine advertises.
 *
 * One helper for the launch path and the Settings toggle, so both ask the core the same question
 * and neither owns a second copy of the port, the address rule, or the hostname.
 */
export async function startWebAccess(): Promise<void> {
  const [address, hostname] = await Promise.all([
    unwrap(commands.webLanAddress()),
    unwrap(commands.webLanHostname()),
  ]);
  if (address === null) throw new Error("No local network address was found.");
  await unwrap(commands.webStart(address, WEB_PORT, hostname));
  await unwrap(commands.webSetKeepAwake(localStorage.getItem(WEB_KEEP_AWAKE_KEY) === "1"));
}
