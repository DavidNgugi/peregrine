import { ErrorState, SettingsRow, SettingsSection } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import {
  LANGUAGE_CHOICES,
  SUBTITLE_MODES,
  usePlaybackPreferences,
} from "../hooks/usePlaybackPreferences.js";

const SELECT_CLASS =
  "rounded-md border border-border bg-surface px-2 py-1 text-body text-text-primary";

/**
 * The languages the player starts on.
 *
 * The core stores one preference and the player reads it; this section only edits it. The note
 * says what the preference does and does not promise: a release that names no language is still
 * a candidate, so the download preference is a preference rather than a filter.
 */
export function PlaybackSection(): ReactElement {
  const { t } = useTranslation();
  const playback = usePlaybackPreferences();
  const preferences = playback.preferences;
  const disabled = preferences === null || playback.busy;

  return (
    <SettingsSection
      description={t("settings.playback.description")}
      title={t("settings.playback.title")}
    >
      {playback.failed ? <ErrorState title={t("settings.playback.failed")} /> : null}
      <SettingsRow controlId="playback-audio-language" label={t("settings.playback.audio")}>
        <select
          className={SELECT_CLASS}
          disabled={disabled}
          id="playback-audio-language"
          onChange={(event) => playback.chooseAudio(event.target.value)}
          value={preferences?.audioLanguage ?? "en"}
        >
          {LANGUAGE_CHOICES.map((language) => (
            <option key={language} value={language}>
              {t("settings.playback.language." + language)}
            </option>
          ))}
        </select>
      </SettingsRow>
      <SettingsRow controlId="playback-subtitle-language" label={t("settings.playback.subtitles")}>
        <select
          className={SELECT_CLASS}
          disabled={disabled}
          id="playback-subtitle-language"
          onChange={(event) => playback.chooseSubtitle(event.target.value)}
          value={preferences?.subtitleLanguage ?? "en"}
        >
          {LANGUAGE_CHOICES.map((language) => (
            <option key={language} value={language}>
              {t("settings.playback.language." + language)}
            </option>
          ))}
        </select>
      </SettingsRow>
      <SettingsRow controlId="playback-subtitle-mode" label={t("settings.playback.mode")}>
        <select
          className={SELECT_CLASS}
          disabled={disabled}
          id="playback-subtitle-mode"
          onChange={(event) => {
            const mode = SUBTITLE_MODES.find((choice) => choice === event.target.value);
            if (mode !== undefined) playback.chooseMode(mode);
          }}
          value={preferences?.subtitles ?? "auto"}
        >
          {SUBTITLE_MODES.map((mode) => (
            <option key={mode} value={mode}>
              {t("settings.playback.mode" + mode.charAt(0).toUpperCase() + mode.slice(1))}
            </option>
          ))}
        </select>
      </SettingsRow>
      <p className="px-3 py-2 text-caption text-text-tertiary">{t("settings.playback.note")}</p>
      {playback.saveFailed ? <ErrorState title={t("settings.playback.saveFailed")} /> : null}
    </SettingsSection>
  );
}
