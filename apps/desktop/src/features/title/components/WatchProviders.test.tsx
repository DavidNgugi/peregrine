import { createI18n } from "@peregrine/i18n";
import type { WatchAvailabilityDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import { WatchProviders } from "./WatchProviders.js";

const i18n = createI18n();

function availability(overrides: Partial<WatchAvailabilityDto> = {}): WatchAvailabilityDto {
  return {
    region: "US",
    link: "https://www.themoviedb.org/movie/1/watch?locale=US",
    fetchedAt: 1,
    offers: [
      { category: "rent", providerId: 2, provider: "Apple TV", logo: "/cache/apple.jpg" },
      { category: "flatrate", providerId: 8, provider: "Netflix", logo: "/cache/netflix.jpg" },
      { category: "flatrate", providerId: 9, provider: "Prime Video", logo: null },
    ],
    ...overrides,
  };
}

function markup(data: WatchAvailabilityDto | null): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <WatchProviders data={data} />
    </I18nextProvider>,
  );
}

describe("WatchProviders", () => {
  it("draws subscription first and every mark in the same box", () => {
    const html = markup(availability());

    expect(html).toContain("Where to watch");
    expect(html).toContain("Availability in US");
    expect(html.indexOf("Streaming")).toBeLessThan(html.indexOf("Rent"));
    expect(html).toContain("Netflix");
    expect(html).toContain("Prime Video");
    expect(html).toContain("Apple TV");
    // Two offers carry a cached logo and one falls back to its name; every box is the same size.
    expect(html.match(/h-9 w-16/gu)).toHaveLength(3);
    expect(html).toContain("Streaming data by JustWatch.");
  });

  it("draws nothing before anyone has asked", () => {
    expect(markup(availability({ fetchedAt: null }))).toBe("");
    expect(markup(null)).toBe("");
  });

  it("says the region is empty rather than looking broken when the answer held nothing", () => {
    const html = markup(availability({ offers: [] }));
    expect(html).toContain("Nothing is listed in US yet.");
  });
});
