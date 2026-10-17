import type { ReactElement, ReactNode } from "react";
import { useId } from "react";

/**
 * The shape every wizard step shares: one heading, one sentence, then the decision itself.
 *
 * The heading is focusable rather than a plain `<h1>`: after Back or Next the wizard moves focus
 * here, and a heading that cannot take focus cannot be announced. `tabIndex={-1}` keeps it out of
 * the tab order, which is the difference between "where the wizard puts you" and "another stop on
 * the way to the controls".
 */
export interface SetupStepProps {
  readonly title: string;
  /** One sentence about what this step decides. */
  readonly body: string;
  readonly children?: ReactNode;
}

export function SetupStep({ title, body, children }: SetupStepProps): ReactElement {
  const titleId = useId();

  return (
    <section aria-labelledby={titleId} className="flex flex-col gap-4">
      <div className="flex flex-col gap-2">
        <h1 className="text-title-1 text-text-primary" id={titleId} tabIndex={-1}>
          {title}
        </h1>
        <p className="text-body text-text-secondary">{body}</p>
      </div>
      {children}
    </section>
  );
}
