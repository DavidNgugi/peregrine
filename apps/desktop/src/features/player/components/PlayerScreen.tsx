/**
 * The player: one title's file, playing in the page.
 *
 * **The surface is the webview's own media element, and the bytes arrive over loopback HTTP.** That is
 * the decision ADR 0023 records: the asset protocol deliberately never exposes the library root, so a
 * local file is served by a listener this process opened for it — one file, one token, one socket, for
 * as long as this screen is mounted. The URL comes from the core and is handed straight to the element;
 * nothing here builds one, and nothing here knows the token.
 *
 * **The three states are drawn from props rather than from the hook, and that is so they can be tested.**
 * `PlayerView` is what the answers look like; `PlayerScreen` is the two lines that read them. This
 * harness renders to static markup with no DOM, so a state reached only through an effect is a state no
 * test ever sees — and "a refused file" and "an empty session" are exactly the two a reader is most
 * likely to meet.
 *
 * **Autoplay can be refused, and the refusal is drawn rather than left as a black rectangle.** The Play
 * press and the element's `play()` are separated by a route change and an IPC round trip, and a webview is
 * entitled to decide the user gesture has been spent — `play()` then rejects with `NotAllowedError` and the
 * element sits paused with nothing playing. So the element is asked to play again once it can, and if that
 * is refused the screen draws its own play control over the video, which *is* a gesture and therefore
 * succeeds. A reader who sees a black rectangle has no way to tell a blocked autoplay from a broken file;
 * a reader who sees a play button knows exactly what to do.
 *
 * **What this screen is not, yet.** No timeline of our own, no track menu, no hover scrub, no next-up:
 * `PG-E16-07` and `PG-E16-08` own those, and they are written against the session rather than against
 * the element. What is here is `controls`, which is the platform's, and a way back.
 */

import { Button, EmptyState, ErrorState, Screen } from "@peregrine/ui";
import { normalizeIpcError, playExternally, type IpcError } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { messageOf } from "../../../lib/ipc-message.js";
import { useFrameCapture } from "../hooks/useFrameCapture.js";
import { usePlayback, type Playback } from "../hooks/usePlayback.js";
import { usePlaybackProgress } from "../hooks/usePlaybackProgress.js";

export interface PlayerViewProps {
  /** What the hook answered: the session, or why there is none. */
  readonly playback: Playback;
  /** Why the external player could not be started, when the session needs one. */
  readonly externalFailure?: IpcError | null | undefined;
  /** Leaves the player. */
  readonly onClose: () => void;
}

/** The player as it is drawn, from answers. */
export function PlayerView({ playback, externalFailure, onClose }: PlayerViewProps): ReactElement {
  const { t } = useTranslation();
  const { session } = playback;
  // **An external session draws no picture in this page.** The webview opened a session to learn
  // what the file is, but mpv owns the surface; a video element here would be a black rectangle over
  // a format it cannot demux. The core's own progress sink is what records this one.
  const external = session?.support === "externalPlayer";
  const surface = useRef<HTMLVideoElement | null>(null);
  // Whether the webview refused to start playing on its own. Set from the promise `play()` returns and
  // cleared by the same element's `playing` event, so the overlay cannot outlive the state it reports.
  const [blocked, setBlocked] = useState(false);
  // Seek to the account's stored position on the first frame, and report as the film advances.
  // An external session is not this hook's: the core records mpv's position over its own socket.
  usePlaybackProgress(surface, external ? null : session);

  // A new session is a new chance to autoplay, and a refusal belongs to the session it happened in.
  useEffect(() => {
    setBlocked(false);
  }, [session?.url]);

  const start = (): void => {
    const element = surface.current;
    if (element === null) {
      return;
    }
    const attempt = element.play();
    // `undefined` on an engine that answers nothing, which is not a refusal and must not draw the overlay.
    if (attempt === undefined) {
      return;
    }
    void attempt.catch(() => {
      // Only the honest case: the element is there, it has a source, and the engine declined to start.
      setBlocked(true);
    });
  };

  return (
    <Screen title={t("player.title")} subtitle={session === null ? undefined : t("player.playing")}>
      <div className="flex min-h-0 flex-1 flex-col gap-3">
        <div className="flex items-center justify-between gap-3">
          <Button onClick={onClose} size="sm" variant="ghost">
            {t("player.back")}
          </Button>
          {/* Which reader is doing the work, said beside the picture rather than as a refusal: it is an
              explanation for a container the webview cannot name, and reassurance otherwise. */}
          {session === null ? null : (
            <span className="text-caption text-text-tertiary">
              {t(`player.support.${session.support}`)}
            </span>
          )}
        </div>

        {playback.failed === null ? null : (
          <ErrorState body={messageOf(playback.failed)} title={t("player.refused")} />
        )}

        {externalFailure === null || externalFailure === undefined ? null : (
          <ErrorState body={messageOf(externalFailure)} title={t("player.refused")} />
        )}

        {playback.starting ? (
          // A sentence rather than a spinner: `PG-E21-12` owns the skeleton, and binding a loopback
          // listener for a file this machine already has is not a wait worth decorating.
          <p className="text-caption text-text-tertiary" role="status">
            {t("player.starting")}
          </p>
        ) : session === null ? (
          // Reached after a refusal and after a reader pressed Back mid-open. The failure block above
          // carries the reason when there is one, so this only draws when there is not.
          playback.failed === null ? (
            <EmptyState
              body={t("player.nothing.body")}
              size="page"
              title={t("player.nothing.title")}
            />
          ) : null
        ) : (
          <div className="min-h-0 flex-1" data-player-session="">
            {external ? (
              // The external reader owns the picture; this page says where it went and that its
              // progress is still being kept. No video element, because the platform cannot demux
              // the container the core just classified.
              <div
                className="flex h-full items-center justify-center rounded-md border border-border-subtle bg-surface-1"
                data-player-external=""
              >
                <p className="max-w-md px-6 text-center text-body text-text-secondary">
                  {t("player.external")}
                </p>
              </div>
            ) : (
              // **The element is given the URL the core answered with and nothing else.** `controls` is
              // deliberate: the platform's own controls are honest about what it can do, and our own
              // overlay is `PG-E16-07`'s. `playsInline` keeps a fullscreen hand-off from being forced on
              // a reader who wants the page.
              <video
                autoPlay
                className="h-full max-h-full w-full rounded-md bg-black"
                controls
                data-player-surface=""
                // The engine's own account of the autoplay attempt: `canplay` is where `play()` is worth
                // asking again, and `playing` is what says it worked.
                onCanPlay={start}
                onPlaying={() => {
                  setBlocked(false);
                }}
                playsInline
                ref={surface}
                src={session.url}
              >
                {t("player.unsupported")}
              </video>
            )}
          </div>
        )}

        {/* **The fallback, and it is deliberate rather than a duplicate of the native control.** A blocked
            autoplay leaves an element that will not start until something *in the page* is clicked, and the
            webview's own control is not always offered over a video that never began. This button is the
            gesture. */}
        {blocked ? (
          <div className="flex items-center justify-center" data-player-blocked="">
            <Button onClick={start} variant="primary">
              {t("player.start")}
            </Button>
          </div>
        ) : null}
      </div>
    </Screen>
  );
}

export interface PlayerScreenProps {
  /** The title's id, which is what a session is opened for. */
  readonly id: number;
  /** The episode to open, when the press named one; absent plays the title's own file. */
  readonly season?: number | undefined;
  readonly episode?: number | undefined;
  /** Leaves the player. */
  readonly onClose: () => void;
}

/** The player, wired to the core — including the poster it captures while it plays. */
export function PlayerScreen({ id, season, episode, onClose }: PlayerScreenProps): ReactElement {
  const playback = usePlayback(
    id,
    season === undefined || episode === undefined ? undefined : { season, episode },
  );
  // The capture is offered by the session and asked for by the core: `needsPoster` is the core's answer to
  // "has this title nothing to draw", which is the only case a film still is an improvement on. An
  // external session cannot be decoded by the webview, so a still is not on offer there.
  const wantsPoster =
    playback.session?.needsPoster === true && playback.session.support !== "externalPlayer";
  const capture = useFrameCapture(
    wantsPoster ? (playback.session?.thumbnailUrl ?? null) : null,
    // No duration hint: the element the capture creates reads its own, which is the same answer without a
    // second piece of state to keep in step with it.
    null,
  );

  useEffect(() => {
    if (wantsPoster) {
      capture.capture();
    }
  }, [wantsPoster, capture]);

  // **An external session is a process launch, not a route.** The player route can be opened directly
  // (Continue Watching's tile links to it), so the screen — not only the title page — has to start the
  // reader the core named. The launch replaces the loopback session this hook opened for the same file,
  // and the core's position sink is what keeps Continue Watching current.
  const [externalFailure, setExternalFailure] = useState<IpcError | null>(null);
  const launched = useRef<number | null>(null);
  const session = playback.session;

  useEffect(() => {
    if (session === null || session.support !== "externalPlayer") {
      setExternalFailure(null);
      return;
    }
    if (launched.current === session.sessionId) {
      return;
    }
    launched.current = session.sessionId;
    void playExternally(session.titleId).catch((caught: unknown) => {
      setExternalFailure(normalizeIpcError(caught));
    });
  }, [session]);

  return <PlayerView externalFailure={externalFailure} onClose={onClose} playback={playback} />;
}
