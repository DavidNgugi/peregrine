import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { SettingsSection } from "./SettingsSection.js";

function titleId(markup: string): string {
  return /<h2[^>]*id="([^"]*)"/u.exec(markup)?.[1] ?? "";
}

describe("SettingsSection", () => {
  it("names itself from the heading it drew rather than from a caller's id", () => {
    const markup = renderToStaticMarkup(
      <SettingsSection title="Keyboard shortcuts">
        <div>a row</div>
      </SettingsSection>,
    );
    const id = titleId(markup);
    expect(id).not.toBe("");
    expect(markup).toContain(`aria-labelledby="${id}"`);
  });

  it("gives two sections different names, so neither can steal the other's", () => {
    // The ids are generated, which is what stops a screen inventing `settings-foo-title`
    // strings that collide once two screens are on the page.
    const markup = renderToStaticMarkup(
      <>
        <SettingsSection title="One">
          <div />
        </SettingsSection>
        <SettingsSection title="Two">
          <div />
        </SettingsSection>
      </>,
    );
    const ids = [...markup.matchAll(/<h2[^>]*id="([^"]*)"/gu)].map((match) => match[1]);
    expect(ids).toHaveLength(2);
    expect(ids[0]).not.toBe(ids[1]);
  });

  it("owns the group container so no screen styles one", () => {
    const markup = renderToStaticMarkup(
      <SettingsSection title="One">
        <div>a row</div>
      </SettingsSection>,
    );
    expect(markup).toContain("divide-border-subtle");
    expect(markup).toContain("border-border-subtle");
  });

  it("carries the description when there is one and nothing when there is not", () => {
    const withDescription = renderToStaticMarkup(
      <SettingsSection description="Why this is here." title="One">
        <div />
      </SettingsSection>,
    );
    expect(withDescription).toContain("Why this is here.");

    const without = renderToStaticMarkup(
      <SettingsSection title="One">
        <div />
      </SettingsSection>,
    );
    expect(without).not.toContain("Why this is here.");
  });

  it("holds a block that is not a row, because a section can be empty", () => {
    // The container is a `div` rather than a `ul` for exactly this: an empty state is not
    // a list item, and a `ul` containing one would be invalid markup for no gain.
    const markup = renderToStaticMarkup(
      <SettingsSection title="One">
        <p>Nothing yet.</p>
      </SettingsSection>,
    );
    expect(markup).toContain("Nothing yet.");
    expect(markup).not.toContain("<ul");
  });
});
