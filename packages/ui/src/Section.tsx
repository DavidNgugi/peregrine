import { clsx } from "clsx";
import { useId } from "react";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * A titled region inside a screen.
 *
 * It owns the `<h2>`, the id that names the `<section>`, and the typography of both the
 * heading and its description — the three things a screen would otherwise write out by hand
 * and get subtly different each time. `SettingsSection` is this with a bordered group
 * container under it, which is the difference between "a region of a screen" and "a group of
 * settings rows".
 */
export interface SectionProps {
  readonly title: string;
  /**
   * A line under the heading.
   *
   * `| undefined` on this and `className` is deliberate under `exactOptionalPropertyTypes`:
   * a wrapper that forwards its own optional props — `SettingsSection` is one — would
   * otherwise have to build its props conditionally to say the same thing.
   */
  readonly description?: string | undefined;
  readonly children: ReactNode;
  readonly className?: string | undefined;
}

export function Section({ title, description, children, className }: SectionProps): ReactElement {
  const titleId = useId();

  return (
    <section aria-labelledby={titleId} className={twMerge(clsx("flex flex-col gap-3"), className)}>
      <div className="flex flex-col gap-1">
        <h2 className="text-title-2 text-text-primary" id={titleId}>
          {title}
        </h2>
        {description === undefined ? null : (
          <p className="text-caption text-text-tertiary">{description}</p>
        )}
      </div>
      {children}
    </section>
  );
}
