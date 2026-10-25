import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Toast } from "./Toast.js";

describe("Toast", () => {
  it("announces a result politely and names it", () => {
    const html = renderToStaticMarkup(
      <Toast onDismiss={() => undefined} title="TheIntroDB works." tone="success" />,
    );

    expect(html).toContain('role="status"');
    expect(html).toContain("TheIntroDB works.");
    expect(html).toContain("Dismiss");
    expect(html).not.toContain('role="alert"');
  });

  it("announces a failure as an alert", () => {
    const html = renderToStaticMarkup(
      <Toast onDismiss={() => undefined} title="TheIntroDB could not be reached." tone="danger" />,
    );

    expect(html).toContain('role="alert"');
    expect(html).toContain('aria-live="assertive"');
  });

  it("is a fixed layer rather than a row in the page", () => {
    const html = renderToStaticMarkup(<Toast title="Testing TheIntroDB…" tone="progress" />);

    expect(html).toContain("fixed");
    expect(html).toContain("bottom-6");
    expect(html).toContain("pointer-events-none");
  });
});
