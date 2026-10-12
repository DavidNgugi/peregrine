import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Section } from "./Section.js";

function headingId(markup: string): string {
  return /<h2[^>]*id="([^"]*)"/u.exec(markup)?.[1] ?? "";
}

describe("Section", () => {
  it("names its region from the heading it drew", () => {
    const markup = renderToStaticMarkup(
      <Section title="Job journal">
        <p>rows</p>
      </Section>,
    );
    const id = headingId(markup);
    expect(id).not.toBe("");
    expect(markup).toContain(`aria-labelledby="${id}"`);
  });

  it("uses a second-level heading, because the screen already owns the first", () => {
    expect(
      renderToStaticMarkup(
        <Section title="Job journal">
          <p />
        </Section>,
      ),
    ).toContain("<h2");
  });

  it("carries a description when there is one and nothing when there is not", () => {
    const withDescription = renderToStaticMarkup(
      <Section description="What this region holds." title="Job journal">
        <p />
      </Section>,
    );
    expect(withDescription).toContain("What this region holds.");

    const without = renderToStaticMarkup(
      <Section title="Job journal">
        <p />
      </Section>,
    );
    expect(without).not.toContain("What this region holds.");
  });

  it("holds whatever the region needs, including nothing at all", () => {
    // A region that draws its own empty state must not be forced into a list, which is why
    // it is a `Section` and not a `SettingsSection` with its row container.
    const markup = renderToStaticMarkup(
      <Section title="Job journal">
        <p>Nothing recorded yet.</p>
      </Section>,
    );
    expect(markup).toContain("Nothing recorded yet.");
    expect(markup).not.toContain("divide-y");
  });
});
