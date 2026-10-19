/**
 * The audio and subtitle languages the account prefers.
 *
 * The read happens once when the section mounts; a write sends the whole preference because the
 * core stores one row, and a partial write would race a second control the person changed first.
 * Nothing here invents a language: the choices are the ones the screen offers, and the core is the
 * authority on what was stored.
 */

import {
  commands,
  normalizeIpcError,
  unwrap,
  type PlaybackPreferencesDto,
} from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";

/** The language tags the picker offers, in the order it draws them. */
export const LANGUAGE_CHOICES = [
  "en",
  "es",
  "fr",
  "de",
  "it",
  "pt",
  "ja",
  "ko",
  "zh",
  "ru",
  "ar",
  "hi",
] as const;

/** The subtitle start modes the picker offers. */
export const SUBTITLE_MODES = ["auto", "forced", "off"] as const;

/** The preference as the Settings section reads and changes it. */
export interface PlaybackPreferencesControl {
  /** The stored preference, or null before the first read answers. */
  readonly preferences: PlaybackPreferencesDto | null;
  /** The first read is still running. */
  readonly loading: boolean;
  /** A write is in flight. */
  readonly busy: boolean;
  /** The first read failed. */
  readonly failed: boolean;
  /** The last write failed. */
  readonly saveFailed: boolean;
  readonly chooseAudio: (language: string) => void;
  readonly chooseSubtitle: (language: string) => void;
  readonly chooseMode: (mode: "auto" | "off" | "forced") => void;
}

/** The preference and the three actions that change it. */
export function usePlaybackPreferences(): PlaybackPreferencesControl {
  const [state, setState] = useState<{
    readonly preferences: PlaybackPreferencesDto | null;
    readonly loading: boolean;
    readonly busy: boolean;
    readonly failed: boolean;
    readonly saveFailed: boolean;
  }>({ preferences: null, loading: true, busy: false, failed: false, saveFailed: false });

  useEffect(() => {
    let cancelled = false;
    unwrap(commands.playbackPreferences())
      .then((preferences) => {
        if (!cancelled) {
          setState({ preferences, loading: false, busy: false, failed: false, saveFailed: false });
        }
      })
      .catch((caught: unknown) => {
        logUiError("playbackPreferences", normalizeIpcError(caught));
        if (!cancelled) {
          setState({
            preferences: null,
            loading: false,
            busy: false,
            failed: true,
            saveFailed: false,
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const save = useCallback((next: PlaybackPreferencesDto): void => {
    setState((previous) => ({ ...previous, busy: true, saveFailed: false }));
    unwrap(commands.setPlaybackPreferences(next))
      .then((stored) => {
        setState({
          preferences: stored,
          loading: false,
          busy: false,
          failed: false,
          saveFailed: false,
        });
      })
      .catch((caught: unknown) => {
        logUiError("setPlaybackPreferences", normalizeIpcError(caught));
        setState((previous) => ({ ...previous, busy: false, saveFailed: true }));
      });
  }, []);

  const current = state.preferences;

  const chooseAudio = useCallback(
    (audioLanguage: string): void => {
      if (current === null) return;
      const next = { ...current, audioLanguage };
      setState((previous) => ({ ...previous, preferences: next }));
      save(next);
    },
    [current, save],
  );

  const chooseSubtitle = useCallback(
    (subtitleLanguage: string): void => {
      if (current === null) return;
      const next = { ...current, subtitleLanguage };
      setState((previous) => ({ ...previous, preferences: next }));
      save(next);
    },
    [current, save],
  );

  const chooseMode = useCallback(
    (subtitles: "auto" | "off" | "forced"): void => {
      if (current === null) return;
      const next = { ...current, subtitles };
      setState((previous) => ({ ...previous, preferences: next }));
      save(next);
    },
    [current, save],
  );

  return { ...state, chooseAudio, chooseSubtitle, chooseMode };
}
