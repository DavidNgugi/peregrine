/**
 * A title's poster, or the space one would fill.
 *
 * **This is the only place a cached image becomes an `<img>`.** `lib/media.ts` is the only module
 * that names `convertFileSrc`, and this is its one caller: the core answers a DTO with an absolute
 * path inside the image cache, and `imageUrl` turns it into an `asset:` URL the webview fetches
 * directly, so the bytes never cross IPC (AGENTS R11). A path the protocol refuses and a path whose
 * blob is gone are the same thing to a reader — no picture — and both land on the placeholder rather
 * than on a broken-image glyph.
 *
 * The box is drawn whether or not there is a picture, at the poster's own 2:3 ratio, so a page does
 * not reflow when one arrives.
 */

import { useState } from "react";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { imageUrl } from "../../../lib/media.js";

/**
 * The intrinsic size the cache's `w342` poster variant is served at.
 *
 * The `width`/`height` attributes say what the picture *is*, which is what lets a browser lay the
 * box out correctly before the bytes arrive; the box's own `aspect-[2/3]` class is the same ratio,
 * which is the one a film poster has.
 */
const POSTER_WIDTH = 342;
const POSTER_HEIGHT = 513;

export interface TitleArtworkProps {
  /** The title's name, which is what the picture is of when it cannot be seen. */
  readonly name: string;
  /** The cache reference the core answered with, or `null` when the cache holds no poster. */
  readonly poster: string | null;
}

/** The poster, or a placeholder of the same size. */
export function TitleArtwork({ name, poster }: TitleArtworkProps): ReactElement {
  const { t } = useTranslation();
  // The reference that failed, rather than a flag: navigating to another title has to give the next
  // poster its own chance, and a boolean would keep the previous title's refusal forever.
  const [failed, setFailed] = useState<string | null>(null);
  const source = poster !== null && poster !== failed ? imageUrl(poster) : null;

  return (
    <div className="aspect-[2/3] w-full max-w-[342px] shrink-0 overflow-hidden rounded-md border border-border-subtle bg-surface-1">
      {source === null ? (
        <p className="flex h-full items-center justify-center px-4 text-center text-caption text-text-tertiary">
          {t("title.poster.none")}
        </p>
      ) : (
        <img
          alt={t("title.poster.alt", { name })}
          className="h-full w-full object-cover"
          height={POSTER_HEIGHT}
          onError={() => {
            // A refusal from the asset protocol arrives here, and so does a file an OS cleaner took
            // between the read and the render. Both mean the same to a reader, and this is what keeps
            // either from being a broken-image glyph with the title as its alt text.
            setFailed(poster);
          }}
          src={source}
          width={POSTER_WIDTH}
        />
      )}
    </div>
  );
}
