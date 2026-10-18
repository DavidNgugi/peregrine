import { createI18n } from "@peregrine/i18n";
import type { CastMemberDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import { CastRail } from "./CastRail.js";

const bridge = vi.hoisted(() => ({
  convertFileSrc: vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
  inTauri: true,
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: bridge.convertFileSrc,
  isTauri: () => bridge.inTauri,
}));

const i18n = createI18n();

function cast(overrides: Partial<CastMemberDto> = {}): CastMemberDto {
  return {
    id: 7,
    name: "Matthew McConaughey",
    character: "Cooper",
    profile: null,
    ...overrides,
  };
}

function markup(members: readonly CastMemberDto[], onPerson?: (id: number) => void): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <CastRail cast={members} {...(onPerson === undefined ? {} : { onPerson })} />
    </I18nextProvider>,
  );
}

describe("CastRail", () => {
  it("draws nothing for an empty cast", () => {
    expect(markup([])).toBe("");
  });

  it("draws the rail's geometry while the credits are still arriving", () => {
    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <CastRail cast={[]} loading />
      </I18nextProvider>,
    );
    expect(html).toContain("Cast");
    expect(html).toContain("rounded-full");
  });

  it("draws a name, a character, and initials when there is no portrait", () => {
    const html = markup([cast()]);
    expect(html).toContain("Matthew McConaughey");
    expect(html).toContain("Cooper");
    expect(html).toContain("MM");
    expect(html).not.toContain("asset://");
  });

  it("draws the cached portrait through the asset protocol when there is one", () => {
    const html = markup([cast({ profile: "/cache/keanu.jpg" })]);
    expect(html).toContain("asset://localhost/");
    expect(html).toContain("keanu");
  });

  it("makes a credited person a keyboard-reachable control with a visible focus ring", () => {
    const html = markup([cast()], () => undefined);

    expect(html).toContain("<button");
    expect(html).toContain("focus-visible:outline-2");
    expect(html).not.toContain('disabled=""');
  });
});
