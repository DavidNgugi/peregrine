import type { CredentialStoreDto } from "@peregrine/ipc-client";
import { Button, EmptyState, ErrorState, TextInput } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { SetupStep } from "./SetupStep.js";

/**
 * One provider, as the step needs it.
 *
 * `builtIn` is the seam for a build that can supply its own key: when it is true and
 * `onUseBuiltIn` is provided, the step becomes a single button instead of a field. No build does
 * that today, which is why the wizard passes false — but the props already describe the one-click
 * path, so adding such a build is data rather than a rewrite of this component.
 */
export interface SetupProviderView {
  readonly provider: string;
  readonly displayName: string;
  readonly configured: boolean;
  /** Whether this build can supply the key itself. */
  readonly builtIn: boolean;
}

export interface ProviderStepProps {
  readonly view: SetupProviderView | null;
  readonly store: CredentialStoreDto;
  readonly loading: boolean;
  readonly failed: boolean;
  readonly busy: boolean;
  readonly outcome: { readonly key: string; readonly ok: boolean } | null;
  readonly error: string | null;
  readonly onSave: (provider: string, key: string) => Promise<void>;
  readonly onTest: (provider: string) => Promise<void>;
  readonly onUseBuiltIn?: (() => Promise<void>) | undefined;
  readonly onRetryStore: () => void;
}

/**
 * The metadata key.
 *
 * **Save is the first action, and "Test connection" follows once a key is stored.** The core has
 * no command that proves a candidate key without storing it: `set_key` runs the probe and writes
 * only on success, while `test_connection` probes the key already in the keychain. Putting Test
 * first would therefore test the previous key while the new one sat in the field, so the primary
 * action is Save — which is honest about what it does — and Test is offered only when there is a
 * stored key to test.
 */
export function ProviderStep({
  view,
  store,
  loading,
  failed,
  busy,
  outcome,
  error,
  onSave,
  onTest,
  onUseBuiltIn,
  onRetryStore,
}: ProviderStepProps): ReactElement {
  const { t } = useTranslation();
  const [key, setKey] = useState("");

  if (failed) {
    return (
      <SetupStep body={t("setup.provider.body")} title={t("setup.provider.title")}>
        <ErrorState
          body={t("setup.provider.unavailable.body")}
          title={t("setup.provider.unavailable.title")}
        />
      </SetupStep>
    );
  }

  if (view === null) {
    return (
      <SetupStep body={t("setup.provider.body")} title={t("setup.provider.title")}>
        {loading ? (
          <p className="text-body text-text-secondary" role="status">
            {t("setup.provider.loading")}
          </p>
        ) : (
          <EmptyState size="inline" title={t("setup.provider.empty")} />
        )}
      </SetupStep>
    );
  }

  const providerName = view.displayName;
  const feedback = (
    <>
      {!view.configured && store !== "answered" ? (
        <div className="flex flex-col gap-2">
          <p className="text-body text-text-secondary" role="status">
            {t(`setup.provider.store.${store}`)}
          </p>
          <div>
            <Button onClick={onRetryStore} size="sm" variant="secondary">
              {t("setup.provider.retry")}
            </Button>
          </div>
        </div>
      ) : null}
      {busy ? (
        <p className="text-body text-text-secondary" role="status">
          {t("setup.provider.checking", { provider: providerName })}
        </p>
      ) : null}
      {error === null ? null : <ErrorState title={error} />}
      {outcome === null ? null : outcome.ok ? (
        <p className="text-body text-text-secondary" role="status">
          {t(outcome.key, { provider: providerName })}
        </p>
      ) : (
        <ErrorState title={t(outcome.key, { provider: providerName })} />
      )}
    </>
  );

  if (view.builtIn && onUseBuiltIn !== undefined) {
    return (
      <SetupStep body={t("setup.provider.body")} title={t("setup.provider.title")}>
        {view.configured ? (
          <p className="text-body text-text-secondary" role="status">
            {t("setup.provider.configured", { provider: providerName })}
          </p>
        ) : (
          <Button
            className="self-start"
            disabled={busy}
            onClick={() => onUseBuiltIn()}
            variant="primary"
          >
            {t("setup.provider.builtIn")}
          </Button>
        )}
        {feedback}
      </SetupStep>
    );
  }

  return (
    <SetupStep body={t("setup.provider.body")} title={t("setup.provider.title")}>
      {view.configured ? (
        <p className="text-body text-text-secondary" role="status">
          {t("setup.provider.configured", { provider: providerName })}
        </p>
      ) : null}
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          // The candidate moves into the call and out of the field: a key left in a form is a key
          // left on screen after the reason for having it has gone.
          const candidate = key;
          setKey("");
          void onSave(view.provider, candidate);
        }}
      >
        <TextInput
          autoComplete="off"
          label={t("setup.provider.label")}
          onChange={(event) => {
            setKey(event.target.value);
          }}
          placeholder={t("setup.provider.placeholder")}
          spellCheck={false}
          type="password"
          value={key}
        />
        <div className="flex items-center gap-2">
          {view.configured ? (
            <Button disabled={busy} onClick={() => onTest(view.provider)} variant="secondary">
              {t("setup.provider.test")}
            </Button>
          ) : null}
          <Button disabled={busy} type="submit" variant="primary">
            {t("setup.provider.save")}
          </Button>
        </div>
      </form>
      {feedback}
    </SetupStep>
  );
}
