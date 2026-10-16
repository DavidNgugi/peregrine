import { describeIpcError, normalizeIpcError } from "@peregrine/ipc-client";
import { Button } from "@peregrine/ui";
import type { ReactElement, ReactNode } from "react";
import { ErrorBoundary, type FallbackProps } from "react-error-boundary";
import { useTranslation } from "react-i18next";
import { logUiError } from "../lib/error-log.js";

function ErrorFallback({ error, resetErrorBoundary }: FallbackProps): ReactElement {
  const { t } = useTranslation();
  // Any thrown value is normalized through the same mapping IPC errors use: this
  // is the last surface before a blank window (docs/ARCHITECTURE.md §15).
  const normalized = normalizeIpcError(error);
  const copy = describeIpcError(normalized);

  return (
    <div className="m-auto flex max-w-md flex-col gap-2 p-6" role="alert">
      <h1 className="text-title-2 text-text-primary">{t(copy.titleKey)}</h1>
      <p className="text-body text-text-secondary">{t(copy.adviceKey)}</p>
      <p className="text-caption text-text-tertiary">
        {t("shell.error.code", { code: normalized.code })}
      </p>
      <Button className="mt-2 self-start" onClick={resetErrorBoundary} variant="secondary">
        {t("shell.error.retry")}
      </Button>
    </div>
  );
}

export function AppErrorBoundary({ children }: { readonly children: ReactNode }): ReactElement {
  return (
    <ErrorBoundary
      FallbackComponent={ErrorFallback}
      onError={(error) => {
        logUiError("AppErrorBoundary", normalizeIpcError(error));
      }}
    >
      {children}
    </ErrorBoundary>
  );
}
