import {
  EmptyState,
  ErrorState,
  Screen,
  SettingsRow,
  SettingsSection,
  ShortcutKeys,
  Switch,
} from "@peregrine/ui";
import { useEffect, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { useAutoStart } from "../hooks/useAutoStart.js";
import {
  PREFERRED_QUALITIES,
  usePreferredQuality,
  type PreferredQuality,
} from "../hooks/usePreferredQuality.js";
import type { IndexerSettings } from "../hooks/useIndexers.js";
import type { LibraryRoots } from "../hooks/useLibraryRoots.js";
import type { ManagedProwlarr } from "../hooks/useProwlarr.js";
import type { ProviderSettings } from "../hooks/useProviders.js";
import type { ShortcutRegistry } from "../hooks/useShortcuts.js";
import type { UsenetSettings } from "../hooks/useUsenet.js";
import { AboutSection } from "./AboutSection.js";
import { BackupSection } from "./BackupSection.js";
import { IndexersSection } from "./IndexersSection.js";
import { LibrariesSection } from "./LibrariesSection.js";
import { ManagedProwlarrSection } from "./ManagedProwlarrSection.js";
import { NetworkSection } from "./NetworkSection.js";
import { PlaybackSection } from "./PlaybackSection.js";
import { WebAccessSection } from "./WebAccessSection.js";
import { ProvidersSection } from "./ProvidersSection.js";
import { UsenetSection } from "./UsenetSection.js";

interface SettingsScreenProps {
  /**
   * The shortcut registry as the shell read it.
   *
   * Passed in rather than read here so the shell and this list read one answer: a second
   * fetch could disagree with the keys the shell is already dispatching on.
   */
  readonly shortcuts: ShortcutRegistry;

  /**
   * The library folders, as the shell read them.
   *
   * Passed in for the same reason the shortcut registry is: the shell owns the read, and a second
   * one here could disagree with the list the rest of the app is working from.
   */
  readonly libraries: LibraryRoots;

  /**
   * The metadata providers, as the shell read them.
   *
   * Passed in for the same reason the other two are: the shell owns the read, and a second one
   * here could disagree with the provider list the rest of the app is working from.
   */
  readonly providers: ProviderSettings;

  /**
   * The indexer configuration, as the shell read it.
   *
   * Passed in for the same reason the other reads are: the shell owns the read, and a second one
   * here could disagree with the indexer the acquire job is working from.
   */
  readonly indexers: IndexerSettings;

  /** The Usenet providers and archive policy, as the shell read them. */
  readonly usenet: UsenetSettings;

  /**
   * The managed Prowlarr instance, as the shell read it.
   *
   * Passed in for the same reason the indexer read is: the shell owns the read, and a second one
   * here could disagree with the instance the search path is using.
   */
  readonly prowlarr: ManagedProwlarr;
  /** The shell's back control, present only when there is somewhere behind this screen. */
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
}

/**
 * Settings, as far as this build has anything to put in it.
 *
 * Every element is the kit's — `SettingsSection` owns the group and its heading,
 * `SettingsRow` owns a row, `Switch` owns the toggle, `ShortcutKeys` owns the accelerator —
 * so this file holds copy and state and no visual classes at all (PG-E7-03).
 *
 * Three states of the shortcut list are distinguished rather than collapsed into "empty":
 * still reading, failed, and genuinely empty. They say different things to the user, and the
 * middle one is the difference between "you have no shortcuts" and "Peregrine could not
 * ask". The settings registry (PG-E20-01) takes this screen over; nothing here is a setting
 * of our own.
 */
export function SettingsScreen({
  shortcuts,
  libraries,
  providers,
  indexers,
  usenet,
  prowlarr,
  onBack,
  backLabel,
}: SettingsScreenProps): ReactElement {
  const { t } = useTranslation();
  const autoStart = useAutoStart();
  const quality = usePreferredQuality();
  const [activeGroup, setActiveGroup] = useState("settings-general");

  useEffect(() => {
    const groups = Array.from(document.querySelectorAll<HTMLElement>(".settings-group"));
    if (groups.length === 0 || typeof IntersectionObserver === "undefined") {
      return undefined;
    }

    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((entry) => entry.isIntersecting)
          .sort((left, right) => right.intersectionRatio - left.intersectionRatio)[0];
        const id = visible?.target.id;
        if (id !== undefined) {
          setActiveGroup(id);
        }
      },
      { rootMargin: "-8% 0px -62% 0px", threshold: [0, 0.25, 0.5, 0.75, 1] },
    );

    for (const group of groups) {
      observer.observe(group);
    }

    return () => observer.disconnect();
  }, []);

  return (
    <Screen
      backLabel={backLabel}
      className="settings-screen"
      onBack={onBack}
      title={t("settings.title")}
    >
      <div className="settings-workspace">
        <aside aria-label={t("settings.navigation.label")} className="settings-nav">
          <nav>
            <a
              aria-current={activeGroup === "settings-general" ? "location" : undefined}
              className={activeGroup === "settings-general" ? "is-active" : undefined}
              href="#settings-general"
            >
              {t("settings.navigation.general")}
            </a>
            <a
              aria-current={activeGroup === "settings-library" ? "location" : undefined}
              className={activeGroup === "settings-library" ? "is-active" : undefined}
              href="#settings-library"
            >
              {t("settings.navigation.library")}
            </a>
            <a
              aria-current={activeGroup === "settings-acquisition" ? "location" : undefined}
              className={activeGroup === "settings-acquisition" ? "is-active" : undefined}
              href="#settings-acquisition"
            >
              {t("settings.navigation.acquisition")}
            </a>
            <a
              aria-current={activeGroup === "settings-connection" ? "location" : undefined}
              className={activeGroup === "settings-connection" ? "is-active" : undefined}
              href="#settings-connection"
            >
              {t("settings.navigation.connection")}
            </a>
            <a
              aria-current={activeGroup === "settings-data" ? "location" : undefined}
              className={activeGroup === "settings-data" ? "is-active" : undefined}
              href="#settings-data"
            >
              {t("settings.navigation.data")}
            </a>
            <a
              aria-current={activeGroup === "settings-about" ? "location" : undefined}
              className={activeGroup === "settings-about" ? "is-active" : undefined}
              href="#settings-about"
            >
              {t("settings.navigation.about")}
            </a>
          </nav>
        </aside>

        <div className="settings-content">
          <div className="settings-group" id="settings-general">
            <div className="settings-group-heading">
              <h2>{t("settings.navigation.general")}</h2>
              <p>{t("settings.group.general")}</p>
            </div>
            <SettingsSection
              description={t("settings.shortcuts.description")}
              title={t("settings.shortcuts.title")}
            >
              {shortcuts.loading ? null : shortcuts.failed ? (
                <ErrorState
                  body={t("settings.shortcuts.unavailable.body")}
                  title={t("settings.shortcuts.unavailable.title")}
                />
              ) : shortcuts.bindings.length === 0 ? (
                <EmptyState title={t("settings.shortcuts.empty")} />
              ) : (
                shortcuts.bindings.map((binding) => (
                  <SettingsRow key={binding.id} label={t(binding.labelKey)}>
                    <ShortcutKeys display={binding.display} />
                  </SettingsRow>
                ))
              )}
            </SettingsSection>

            <SettingsSection
              description={t("settings.autostart.description")}
              title={t("settings.autostart.title")}
            >
              <Switch
                busy={autoStart.busy}
                checked={autoStart.enabled}
                label={t("settings.autostart.label")}
                onChange={(next) => {
                  autoStart.toggle(next);
                }}
              />
              {autoStart.failed ? <ErrorState title={t("settings.autostart.failed")} /> : null}
            </SettingsSection>

            <SettingsSection
              description={t("settings.quality.description")}
              title={t("settings.quality.title")}
            >
              <SettingsRow label={t("settings.quality.label")}>
                <select
                  className="rounded-md border border-border bg-surface px-2 py-1 text-body text-text-primary"
                  disabled={quality.busy}
                  onChange={(event) => {
                    quality.choose(event.target.value as PreferredQuality);
                  }}
                  value={quality.quality ?? ""}
                >
                  {quality.quality === null ? (
                    <option value="">{t("settings.quality.custom")}</option>
                  ) : null}
                  {PREFERRED_QUALITIES.map((choice) => (
                    <option key={choice} value={choice}>
                      {t("settings.quality.choice." + choice)}
                    </option>
                  ))}
                </select>
              </SettingsRow>
              {quality.failed ? <ErrorState title={t("settings.quality.failed")} /> : null}
            </SettingsSection>

            <PlaybackSection />
          </div>

          <div className="settings-group" id="settings-library">
            <div className="settings-group-heading">
              <h2>{t("settings.navigation.library")}</h2>
              <p>{t("settings.group.library")}</p>
            </div>
            <LibrariesSection libraries={libraries} />
            <ProvidersSection providers={providers} />
          </div>

          <div className="settings-group" id="settings-acquisition">
            <div className="settings-group-heading">
              <h2>{t("settings.navigation.acquisition")}</h2>
              <p>{t("settings.group.acquisition")}</p>
            </div>
            <UsenetSection usenet={usenet} />
            <ManagedProwlarrSection prowlarr={prowlarr} />
            <IndexersSection indexers={indexers} />
          </div>

          <div className="settings-group" id="settings-connection">
            <div className="settings-group-heading">
              <h2>{t("settings.navigation.connection")}</h2>
              <p>{t("settings.group.connection")}</p>
            </div>
            <NetworkSection />
            <WebAccessSection />
          </div>

          <div className="settings-group" id="settings-data">
            <div className="settings-group-heading">
              <h2>{t("settings.navigation.data")}</h2>
              <p>{t("settings.group.data")}</p>
            </div>
            <BackupSection />
          </div>

          <div className="settings-group" id="settings-about">
            <div className="settings-group-heading">
              <h2>{t("settings.navigation.about")}</h2>
              <p>{t("settings.group.about")}</p>
            </div>
            <AboutSection />
          </div>
        </div>
      </div>
    </Screen>
  );
}
