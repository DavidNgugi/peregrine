import { createI18n } from "@peregrine/i18n";
import type { NotificationPreferencesDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import type { NotificationConsentControl } from "../hooks/useNotificationConsent.js";
import { writeNotificationConsent } from "../hooks/useNotificationConsent.js";
import { NotificationsSection } from "./NotificationsSection.js";

const i18n = createI18n();

function preferences(
  overrides: Partial<NotificationPreferencesDto> = {},
): NotificationPreferencesDto {
  return {
    consent: "unanswered",
    dontAskAgain: false,
    osGranted: null,
    shouldPrompt: true,
    ...overrides,
  };
}

/** The section's state, with the toggle recorded rather than performed. */
function control(
  overrides: Partial<NotificationConsentControl> = {},
): NotificationConsentControl & { readonly toggled: boolean[] } {
  const toggled: boolean[] = [];
  return {
    preferences: preferences(),
    permission: { kind: "unverified" },
    loading: false,
    busy: false,
    failed: false,
    saveFailed: false,
    denied: false,
    toggle: (granted: boolean) => {
      toggled.push(granted);
    },
    ...overrides,
    toggled,
  };
}

function markup(state: NotificationConsentControl): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <NotificationsSection control={state} />
    </I18nextProvider>,
  );
}

describe("NotificationsSection", () => {
  it("draws the consent as a switch, off until the user grants it", () => {
    const html = markup(control());

    expect(html).toContain("Show system notifications");
    expect(html).toContain('role="switch"');
    expect(html).toContain('aria-checked="false"');
  });

  it("says plainly that turning it on asks the operating system", () => {
    expect(markup(control())).toContain("asks your operating system for permission");
  });

  it("draws the switch on once the core has recorded consent", () => {
    const html = markup(
      control({
        preferences: preferences({
          consent: "granted",
          shouldPrompt: false,
        }),
      }),
    );

    expect(html).toContain('aria-checked="true"');
    expect(html).not.toContain('aria-checked="false"');
  });

  it("says the system refused without claiming the event was lost", () => {
    const html = markup(
      control({
        preferences: preferences({ consent: "declined", shouldPrompt: false }),
        denied: true,
      }),
    );

    expect(html).toContain("did not grant permission");
    expect(html).toContain("notification centre");
  });

  it("is inert while the stored consent has not been read", () => {
    const html = markup(control({ preferences: null, loading: true }));

    expect(html).toContain('disabled=""');
  });
});

describe("writeNotificationConsent", () => {
  const granted = preferences({ consent: "granted", shouldPrompt: false });
  const declined = preferences({ consent: "declined", shouldPrompt: false });

  it("asks the OS before recording a grant, and records the answer it gave", async () => {
    const request = vi.fn(async () => ({ kind: "granted" }) as const);
    const save = vi.fn(async () => granted);

    const result = await writeNotificationConsent(true, request, save);

    expect(request).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith(true);
    expect(result.preferences.consent).toBe("granted");
    expect(result.permission).toEqual({ kind: "granted" });
  });

  it("records a refusal when the system will not grant", async () => {
    const request = vi.fn(async () => ({ kind: "denied" }) as const);
    const save = vi.fn(async () => declined);

    const result = await writeNotificationConsent(true, request, save);

    expect(save).toHaveBeenCalledWith(false);
    expect(result.permission).toEqual({ kind: "denied" });
  });

  it("never asks the system when consent is withdrawn", async () => {
    const request = vi.fn(async () => ({ kind: "granted" }) as const);
    const save = vi.fn(async () => declined);

    await writeNotificationConsent(false, request, save);

    expect(request).not.toHaveBeenCalled();
    expect(save).toHaveBeenCalledWith(false);
  });
});
