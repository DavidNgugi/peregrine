import { Button, Switch } from "@peregrine/ui";
import { commands, unwrap } from "@peregrine/ipc-client";
import type { PairedDeviceDto, PendingPairDto, WebStatusDto } from "@peregrine/ipc-client";
import QRCode from "qrcode";
import { useCallback, useEffect, useMemo, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

export const WEB_PORT = 8642;
export const WEB_AUTO_KEY = "peregrine.web.autoStart";

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
  const [auto, setAuto] = useState(
    () => typeof localStorage !== "undefined" && localStorage.getItem(WEB_AUTO_KEY) === "1",
  );
  const [failed, setFailed] = useState(false);

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

  const start = async (): Promise<void> => {
    if (address === null) {
      setFailed(true);
      return;
    }
    try {
      setStatus(await unwrap(commands.webStart(address, WEB_PORT)));
      await read();
    } catch {
      setFailed(true);
    }
  };
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

  const url =
    status?.running === true && status.address !== null && status.port !== null
      ? "http://" + status.address + ":" + String(status.port) + "/"
      : null;

  return (
    <section className="flex flex-col gap-3 px-3 py-2">
      <div className="flex flex-col gap-1">
        <h3 className="text-title text-text-primary">{t("settings.web.title")}</h3>
        <p className="max-w-prose text-body text-text-secondary">{t("settings.web.description")}</p>
      </div>
      <Switch
        checked={auto}
        description={t("settings.web.autoDescription")}
        label={t("settings.web.auto")}
        onChange={chooseAuto}
      />
      <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_15rem]">
        <div className="flex min-w-0 flex-col gap-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-caption text-text-tertiary">{t("settings.web.status")}</span>
            <span className="flex items-center gap-2">
              <span className="text-body text-text-primary">
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
          {pending.map((pair) => (
            <div
              className="flex flex-wrap items-center justify-between gap-2 rounded-sm border border-border-subtle p-2"
              key={pair.pairId}
            >
              <span className="flex flex-col">
                <span className="text-title text-text-primary">{pair.code}</span>
                <span className="text-caption text-text-secondary">
                  {pair.name}
                  {pair.platform === null ? "" : " · " + pair.platform}
                </span>
              </span>
              <span className="flex gap-2">
                <Button
                  onClick={() => {
                    void unwrap(commands.webApprove(pair.pairId))
                      .then(read)
                      .catch(() => {
                        setFailed(true);
                      });
                  }}
                  size="sm"
                  variant="primary"
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
            </div>
          ))}
          {devices
            .filter((device) => !device.revoked)
            .map((device) => (
              <div
                className="flex flex-wrap items-center justify-between gap-2 border-t border-border-subtle pt-2"
                key={device.id}
              >
                <span className="flex flex-col">
                  <span className="text-body text-text-primary">{device.name}</span>
                  <span className="text-caption text-text-tertiary">
                    {device.platform ?? device.scopes}
                  </span>
                </span>
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
              </div>
            ))}
          {failed ? (
            <p className="text-caption text-state-danger" role="alert">
              {t("settings.web.failed")}
            </p>
          ) : null}
        </div>
        <aside className="flex flex-col items-center gap-2 self-start rounded-md border border-border-subtle bg-surface-2 p-3">
          {url === null ? (
            <p className="text-caption text-text-secondary">{t("settings.web.startForCode")}</p>
          ) : (
            <>
              <div className="aspect-square w-full">
                <QrCode value={url} />
              </div>
              <code className="select-all break-all text-center text-caption text-text-primary">
                {url}
              </code>
              <p className="text-center text-caption text-text-secondary">
                {t("settings.web.code")}
              </p>
            </>
          )}
        </aside>
      </div>
    </section>
  );
}
