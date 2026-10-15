import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { TextInput } from "./TextInput.js";

describe("TextInput", () => {
  it("labels the input it belongs to rather than sitting beside it", () => {
    // A label with no `htmlFor` is a label a screen reader never connects, and one a click does not
    // focus either.
    const markup = renderToStaticMarkup(<TextInput label="Search" />);
    const [label] = [...markup.matchAll(/for="([^"]+)"/gu)].map((match) => match[1] ?? "");
    const [input] = [...markup.matchAll(/id="([^"]+)"/gu)].map((match) => match[1] ?? "");

    expect(label).toBeDefined();
    expect(label).toBe(input);
  });

  it("describes the input with its message and announces a failure", () => {
    // A validation failure that appears silently beside a field is one a screen reader user never
    // hears (§9), which is why the error is a role and not only a colour.
    const help = renderToStaticMarkup(<TextInput label="Search" message="Try a title." />);
    const failure = renderToStaticMarkup(
      <TextInput invalid label="Search" message="That query is empty." />,
    );

    expect(help).toContain("aria-describedby=");
    expect(help).not.toContain('role="alert"');
    expect(failure).toContain('role="alert"');
    expect(failure).toContain('aria-invalid="true"');
    expect(failure).toContain("border-state-danger");
  });

  it("says nothing it was not given", () => {
    // No message means no `aria-describedby` pointing at an id nothing has, which is worse than no
    // description at all.
    const markup = renderToStaticMarkup(<TextInput label="Search" />);

    expect(markup).not.toContain("aria-describedby=");
    expect(markup).not.toContain('role="alert"');
  });

  it("keeps the label for assistive technology when it is hidden visually", () => {
    const markup = renderToStaticMarkup(<TextInput label="Search" labelHidden />);

    expect(markup).toContain("sr-only");
    expect(markup).toContain("Search");
  });

  it("passes the input's own attributes through", () => {
    const markup = renderToStaticMarkup(
      <TextInput label="Search" placeholder="Alien" type="search" value="al" />,
    );

    expect(markup).toContain('type="search"');
    expect(markup).toContain('placeholder="Alien"');
    expect(markup).toContain('value="al"');
  });

  it("draws the focus ring on the input by default", () => {
    // The rule a reader of this primitive should be able to rely on: the field shows focus itself, with
    // the token ring rather than the browser's default outline.
    const markup = renderToStaticMarkup(<TextInput label="Search" />);

    expect(markup).toContain("focus-visible:outline-2");
    expect(markup).toContain("outline-focus-ring");
    expect(markup).not.toContain("focus-visible:outline-none");
  });

  it("hands the focus ring to the caller when asked, so a composite field draws one ring", () => {
    // The bug this prop exists for: Discover's search box is an input restyled into a transparent,
    // borderless, rounded box *inside* a pill, and with the default ring the input drew a second rounded
    // outline within the pill — two concentric pills on focus. `"none"` is the caller saying "the pill is
    // the field, and I draw the affordance there".
    const markup = renderToStaticMarkup(
      <TextInput
        className="[&_input]:rounded-full"
        focusRing="none"
        label="Search"
        type="search"
      />,
    );

    expect(markup).toContain("focus-visible:outline-none");
    expect(markup).not.toContain("outline-focus-ring");
  });
});
