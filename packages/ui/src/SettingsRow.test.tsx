import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { SettingsRow } from "./SettingsRow.js";

describe("SettingsRow", () => {
  it("places the label and the control at the two edges", () => {
    const markup = renderToStaticMarkup(
      <SettingsRow label="Start with your computer">
        <span>control</span>
      </SettingsRow>,
    );
    expect(markup).toContain("justify-between");
    expect(markup.indexOf("Start with your computer")).toBeLessThan(markup.indexOf("control"));
  });

  it("leaves the label as plain words when no control belongs to it", () => {
    // A row that shows a value rather than changing one must not look clickable.
    const markup = renderToStaticMarkup(
      <SettingsRow label="Go to Discover">
        <kbd>⌘1</kbd>
      </SettingsRow>,
    );
    expect(markup).not.toContain("<label");
  });

  it("ties the label to a control by id when the row has one", () => {
    const markup = renderToStaticMarkup(
      <SettingsRow controlId="the-switch" label="Start Peregrine when you sign in">
        <button id="the-switch" type="button" />
      </SettingsRow>,
    );
    expect(markup).toContain("<label");
    expect(markup).toContain('for="the-switch"');
    expect(markup).toContain('id="the-switch"');
  });

  it("keeps the description out of the control's accessible name", () => {
    // Inside the label it would be announced as part of it — "Start Peregrine when you
    // sign in Peregrine does not register itself unless you ask it to" — which is worse
    // than not being announced at all.
    const markup = renderToStaticMarkup(
      <SettingsRow controlId="the-switch" description="A longer explanation." label="Short label">
        <button id="the-switch" type="button" />
      </SettingsRow>,
    );
    const label = /<label[^>]*>(.*?)<\/label>/su.exec(markup)?.[1] ?? "";
    expect(label).toContain("Short label");
    expect(label).not.toContain("A longer explanation.");
    expect(markup).toContain("A longer explanation.");
  });
});
