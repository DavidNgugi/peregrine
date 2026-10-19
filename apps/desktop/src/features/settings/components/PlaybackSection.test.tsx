import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import { PlaybackSection } from "./PlaybackSection.js";

const i18n = createI18n();

/** Static render: the hook's read is an effect and does not run here, which keeps this off IPC. */
function markup(): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PlaybackSection />
    </I18nextProvider>,
  );
}

describe("PlaybackSection", () => {
  it("draws the audio language, subtitle language, and start mode controls", () => {
    const html = markup();
    expect(html).toContain("Audio language");
    expect(html).toContain("Subtitle language");
    expect(html).toContain("Subtitles start");
    expect(html.match(/<select/gu)?.length).toBe(3);
  });

  it("states that the download preference is a preference rather than a guarantee", () => {
    expect(markup()).toContain("Downloads prefer a release that names this audio language");
  });
});
