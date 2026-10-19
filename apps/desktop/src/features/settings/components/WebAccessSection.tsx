import { Button, IconButton, SettingsSection, Switch } from "@peregrine/ui";
import { commands, unwrap } from "@peregrine/ipc-client";
import { copyText } from "../../../lib/clipboard.js";
import {
  WEB_AUTO_KEY,
  WEB_KEEP_AWAKE_KEY,
  WEB_STATUS_EVENT,
  startWebAccess,
} from "../../../lib/webAccess.js";
import type { PairedDeviceDto, PendingPairDto, WebStatusDto } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import QRCode from "qrcode";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

/** A QR code drawn from the encoder matrix, with no HTML injection. */
function QrCode({ value }: { readonly value: string }): ReactElement {
  const matrix = useMemo(() => {
    const code = QRCode.create(value, { errorCorrectionLevel: "M" });
    const size = code.modules.size;
    const cells: { x: number; y: number }[] = [];
    for (let row = 0; row < size; row += 1) {
      for (let column = 0; column < size; column += 1) {
        if (code.modules.get(row, column)) cells.push({ x: column, y: row });
      }
    }
    return { size, cells };
  }, [value]);
  return (
    <svg
      aria-label={value}
      className="size-full rounded-sm bg-white p-2"
      role="img"
      viewBox={`-2 -2 ${matrix.size + 4} ${matrix.size + 4}`}
    >
      {matrix.cells.map((cell) => (
        <rect
          fill="#0b0b0c"
          height={1}
          key={cell.x + ":" + cell.y}
          width={1}
          x={cell.x}
          y={cell.y}
        />
      ))}
    </svg>
  );
}

export function WebAccessSection(): ReactElement {
  const { t } = useTranslation();
  const [status, setStatus] = useState<WebStatusDto | null>(null);
  const [address, setAddress] = useState<string | null>(null);
  const [pending, setPending] = useState<readonly PendingPairDto[]>([]);
  const [devices, setDevices] = useState<readonly PairedDeviceDto[]>([]);
  const [allowAcquire, setAllowAcquire] = useState<Record<string, boolean>>({});
  const [auto, setAuto] = useState(
    () => typeof localStorage !== "undefined" && localStorage.getItem(WEB_AUTO_KEY) === "1",
  );
  const [failed, setFailed] = useState(false);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const copyTimer = useRef<number | null>(null);

  const read = useCallback(async (): Promise<void> => {
    try {
      setStatus(await unwrap(commands.webStatus()));
      setPending(await unwrap(commands.webPairings()));
      setDevices(await unwrap(commands.webDevices()));
      setFailed(false);
    } catch {
      setFailed(true);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      setAddress(await unwrap(commands.webLanAddress()));
      await read();
    })();
  }, [read]);

  useEffect(() => {
    if (status?.running !== true) return;
    const timer = setInterval(() => {
      void read();
    }, 3000);
    return () => {
      clearInterval(timer);
    };
  }, [read, status?.running]);

  // A Wi-Fi switch moves the listener; the shell pushes the new address so the code and the
  // address redraw without a person stopping and starting it. The poll above stays for the
  // pairing list, which changes without an event of its own.
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;
    void listen<WebStatusDto>(WEB_STATUS_EVENT, (event) => {
      setStatus(event.payload);
    }).then((stop) => {
      if (cancelled) {
        stop();
      } else {
        unlisten = stop;
      }
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const saveName = useCallback(
    async (id: number): Promise<void> => {
      try {
        await unwrap(commands.webRename({ id }, draft.trim()));
        setRenaming(null);
        await read();
      } catch {
        setFailed(true);
      }
    },
    [draft, read],
  );

  const start = useCallback(async (): Promise<void> => {
    try {
      await startWebAccess();
      await read();
    } catch {
      setFailed(true);
    }
  }, [read]);
  const stop = async (): Promise<void> => {
    try {
      await unwrap(commands.webStop());
      await read();
    } catch {
      setFailed(true);
    }
  };
  const chooseAuto = (on: boolean): void => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(WEB_AUTO_KEY, on ? "1" : "0");
    }
    setAuto(on);
    if (on) void start();
  };
  const chooseKeepAwake = useCallback(async (on: boolean): Promise<void> => {
    try {
      localStorage.setItem(WEB_KEEP_AWAKE_KEY, on ? "1" : "0");
      setStatus(await unwrap(commands.webSetKeepAwake(on)));
    } catch {
      setFailed(true);
    }
  }, []);

  // The address is what every browser on the network can open; a Bonjour name is only as good
  // as the client's resolver, and a television's browser usually has none. The name is offered
  // beside it, because a device that does resolve it keeps its pairing across an address change.
  const url =
    status?.running === true && status.address !== null && status.port !== null
      ? "http://" + status.address + ":" + String(status.port) + "/"
      : null;
  const nameUrl =
    status?.running === true && status.hostname !== null && status.port !== null
      ? "http://" + status.hostname + ":" + String(status.port) + "/"
      : null;

  const copyLabel = copyState === "copied" ? t("settings.web.copied") : t("settings.web.copyLink");

  const copy = useCallback(async (): Promise<void> => {
    if (url === null) return;
    try {
      await copyText(url);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
    if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    copyTimer.current = window.setTimeout(() => {
      setCopyState("idle");
    }, 2000);
  }, [url]);

  useEffect(
    () => () => {
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    },
    [],
  );

  return (
    <SettingsSection description={t("settings.web.description")} title={t("settings.web.title")}>
      <div className="flex w-full max-w-prose flex-col gap-4 px-3 py-2 text-body text-text-secondary">
        <Switch
          checked={auto}
          description={t("settings.web.autoDescription")}
          label={t("settings.web.auto")}
          onChange={chooseAuto}
        />

        <Switch
          checked={status?.keepAwake === true}
          description={t("settings.web.keepAwakeDescription")}
          label={t("settings.web.keepAwake")}
          onChange={(on) => {
            void chooseKeepAwake(on);
          }}
        />

        <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border-subtle px-3 py-2">
          <span className="text-caption text-text-tertiary">{t("settings.web.status")}</span>
          <span className="flex items-center gap-3">
            <span className={status?.running === true ? "text-state-ok" : "text-text-tertiary"}>
              {status?.running === true ? t("settings.web.running") : t("settings.web.stopped")}
            </span>
            <Button
              onClick={() => {
                void (status?.running === true ? stop() : start());
              }}
              size="sm"
              variant="secondary"
            >
              {status?.running === true ? t("settings.web.stop") : t("settings.web.start")}
            </Button>
          </span>
        </div>

        {address === null ? (
          <p className="text-caption text-state-danger">{t("settings.web.noLan")}</p>
        ) : null}

        {url === null ? (
          <p className="text-caption text-text-tertiary">{t("settings.web.startForCode")}</p>
        ) : (
          <div className="flex flex-wrap items-center gap-4 rounded-md border border-border-subtle bg-surface-1 p-3">
            <div className="size-40 shrink-0 overflow-hidden rounded-sm bg-white p-1">
              <QrCode value={url} />
            </div>
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-caption text-text-tertiary">{t("settings.web.open")}</span>
              <span className="flex items-start gap-2">
                <code className="min-w-0 select-all break-all text-body text-text-primary">
                  {url}
                </code>
                <IconButton
                  icon={copyState === "copied" ? "check" : "copy"}
                  label={copyLabel}
                  onClick={() => {
                    void copy();
                  }}
                  size="sm"
                  title={copyLabel}
                  variant="ghost"
                />
              </span>
              <span
                aria-live="polite"
                className={
                  copyState === "failed"
                    ? "min-h-4 text-caption text-state-danger"
                    : "min-h-4 text-caption text-state-ok"
                }
                role="status"
              >
                {copyState === "copied"
                  ? t("settings.web.copied")
                  : copyState === "failed"
                    ? t("settings.web.copyFailed")
                    : ""}
              </span>
              {nameUrl === null ? null : (
                <span className="break-all text-caption text-text-tertiary">
                  {t("settings.web.bonjourName", { url: nameUrl })}
                </span>
              )}
              <span className="text-caption text-text-secondary">{t("settings.web.code")}</span>
            </div>
          </div>
        )}

        {pending.length === 0 ? null : (
          <div className="flex flex-col gap-2">
            <p className="text-caption text-text-tertiary">{t("settings.web.pending")}</p>
            {pending.map((pair) => (
              <div
                className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border-subtle bg-surface-1 px-3 py-2"
                key={pair.pairId}
              >
                <span className="flex flex-col">
                  <span className="font-mono text-title-2 tracking-[0.18em] text-text-primary">
                    {pair.code.slice(0, 4) + " " + pair.code.slice(4)}
                  </span>
                  <span className="text-caption text-text-secondary">
                    {pair.name}
                    {pair.platform === null ? "" : " \u00b7 " + pair.platform}
                  </span>
                </span>
                <span className="flex flex-wrap items-center gap-3">
                  <label
                    className="flex items-center gap-2 text-caption text-text-secondary"
                    title={t("settings.web.allowDownloadsHint")}
                  >
                    <input
                      checked={allowAcquire[pair.pairId] ?? false}
                      onChange={(event) => {
                        const checked = event.currentTarget.checked;
                        setAllowAcquire((held) => ({ ...held, [pair.pairId]: checked }));
                      }}
                      type="checkbox"
                    />
                    {t("settings.web.allowDownloads")}
                  </label>
                  <span className="flex gap-2">
                    <Button
                      onClick={() => {
                        void unwrap(
                          commands.webApprove(pair.pairId, allowAcquire[pair.pairId] ?? false),
                        )
                          .then(read)
                          .catch(() => {
                            setFailed(true);
                          });
                      }}
                      size="sm"
                    >
                      {t("settings.web.approve")}
                    </Button>
                    <Button
                      onClick={() => {
                        void unwrap(commands.webDeny(pair.pairId))
                          .then(read)
                          .catch(() => {
                            setFailed(true);
                          });
                      }}
                      size="sm"
                      variant="ghost"
                    >
                      {t("settings.web.deny")}
                    </Button>
                  </span>
                </span>
              </div>
            ))}
          </div>
        )}

        {devices.filter((device) => !device.revoked).length === 0 ? null : (
          <div className="flex flex-col gap-2">
            <p className="text-caption text-text-tertiary">{t("settings.web.devices")}</p>
            {devices
              .filter((device) => !device.revoked)
              .map((device) => (
                <div
                  className="flex flex-wrap items-center justify-between gap-3 border-t border-border-subtle pt-2"
                  key={device.id}
                >
                  {renaming === device.id ? (
                    <input
                      aria-label={t("settings.web.rename")}
                      autoFocus
                      className="min-w-0 flex-1 rounded-sm border border-border-subtle bg-surface-1 px-2 py-1 text-body text-text-primary"
                      maxLength={64}
                      onChange={(event) => {
                        setDraft(event.currentTarget.value);
                      }}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") void saveName(device.id);
                        if (event.key === "Escape") setRenaming(null);
                      }}
                      value={draft}
                    />
                  ) : (
                    <span className="flex flex-col">
                      <span className="text-body text-text-primary">{device.name}</span>
                      <span className="text-caption text-text-tertiary">
                        {device.platform ?? device.scopes}
                      </span>
                      <span className="text-caption text-text-tertiary">
                        {t("settings.web.added", {
                          date: new Date(device.createdAt).toLocaleDateString(),
                        })}
                      </span>
                      {device.scopes.split(",").includes("acquire") ? (
                        <span className="text-caption text-state-ok">
                          {t("settings.web.downloadsAllowed")}
                        </span>
                      ) : null}
                    </span>
                  )}
                  <span className="flex items-center gap-2">
                    {renaming === device.id ? (
                      <>
                        <Button
                          disabled={draft.trim() === ""}
                          onClick={() => {
                            void saveName(device.id);
                          }}
                          size="sm"
                        >
                          {t("settings.web.save")}
                        </Button>
                        <Button
                          onClick={() => {
                            setRenaming(null);
                          }}
                          size="sm"
                          variant="ghost"
                        >
                          {t("settings.web.cancel")}
                        </Button>
                      </>
                    ) : (
                      <>
                        <Button
                          onClick={() => {
                            setDraft(device.name);
                            setRenaming(device.id);
                          }}
                          size="sm"
                          variant="ghost"
                        >
                          {t("settings.web.rename")}
                        </Button>
                        <Button
                          onClick={() => {
                            void unwrap(commands.webRevoke({ id: device.id }))
                              .then(read)
                              .catch(() => {
                                setFailed(true);
                              });
                          }}
                          size="sm"
                          variant="ghost"
                        >
                          {t("settings.web.revoke")}
                        </Button>
                      </>
                    )}
                  </span>
                </div>
              ))}
          </div>
        )}

        <p className="text-caption text-text-tertiary">{t("settings.web.acquireNote")}</p>

        <p className="text-caption text-text-tertiary">{t("settings.web.caveat")}</p>

        {failed ? (
          <p className="text-caption text-state-danger" role="alert">
            {t("settings.web.failed")}
          </p>
        ) : null}
      </div>
    </SettingsSection>
  );
}
