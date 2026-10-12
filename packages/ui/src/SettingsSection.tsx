import { clsx } from "clsx";
import { useId } from "react";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * A titled group of settings rows (docs/DESIGN_SYSTEM.md §6, chrome).
 *
 * The heading is the section's accessible name and the group container is the kit's, so a
 * screen writes no visual classes at all: rows arrive as children and the borders, the
 * dividers between them, and the surface they sit on belong here. The ids are generated
 * rather than passed, which is what stops a feature from inventing `settings-foo-title`
 * strings that can collide once two screens are on the page.
 *
 * The container is a plain `div` rather than a `ul`: a section can hold a single empty-state
 * block as readily as a list of rows, and an empty `ul` that contains a `div` is invalid
 * markup for no gain — the `section` is the grouping the assistive technology announces.
 */
export interface SettingsSectionProps {
  readonly title: string;
  readonly description?: string;
  readonly children: ReactNode;
  readonly className?: string;
}

export function SettingsSection({
  title,
  description,
  children,
  className,
}: SettingsSectionProps): ReactElement {
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
      <div
        className={
          "flex flex-col divide-y divide-border-subtle overflow-hidden rounded-md border border-border-subtle bg-surface-1"
        }
      >
        {children}
      </div>
    </section>
  );
}
