import { commands, unwrap } from "@peregrine/ipc-client";

/** The port the TV client is offered on; the backend accepts any, this is the one the UI shows. */
export const WEB_PORT = 8642;

/** The renderer's own autostart choice, which the backend does not need to know about. */
export const WEB_AUTO_KEY = "peregrine.web.autoStart";

/**
 * Starts the LAN listener on the detected address.
 *
 * One helper for the launch path and the Settings toggle, so both ask the core the same question
 * and neither owns a second copy of the port or the address rule.
 */
export async function startWebAccess(): Promise<void> {
  const address = await unwrap(commands.webLanAddress());
  if (address === null) throw new Error("No local network address was found.");
  await unwrap(commands.webStart(address, WEB_PORT));
}
