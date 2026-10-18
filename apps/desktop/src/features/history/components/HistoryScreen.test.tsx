import { createI18n } from "@peregrine/i18n";
import type { WatchHistoryDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import { HISTORY_PAGE, HistoryList } from "./HistoryScreen.js";

const i18n = createI18n();

function rows(count: number): WatchHistoryDto[] {
  return Array.from({ length: count }, (_, index) => ({
    titleId: index + 1,
    title: `Title ${String(index + 1)}`,
    poster: null,
    titleKind: "show",
    seasonNumber: null,
    episodeNumber: null,
    watched: false,
    percent: 0.1,
    updatedAt: index,
  }));
}

function markup(count: number): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <HistoryList
        history={{ rows: rows(count), loading: false, failed: null }}
        onNavigate={() => undefined}
      />
    </I18nextProvider>,
  );
}

describe("HistoryList", () => {
  it("opens on one page and says how many rows there are", () => {
    const html = markup(HISTORY_PAGE + 5);

    expect(html).toContain("Title 1");
    expect(html).toContain(`Title ${String(HISTORY_PAGE)}`);
    expect(html).not.toContain(`Title ${String(HISTORY_PAGE + 1)}`);
    expect(html).toContain(`Showing ${String(HISTORY_PAGE)} of ${String(HISTORY_PAGE + 5)}`);
    expect(html).toContain("Show more");
  });

  it("draws no pager when everything fits", () => {
    const html = markup(HISTORY_PAGE);

    expect(html).not.toContain("Show more");
    expect(html).not.toContain("Showing");
  });
});
