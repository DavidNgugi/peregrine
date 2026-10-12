import type { ReactElement, ReactNode } from "react";
import { Panel } from "./Panel.js";
import { Section } from "./Section.js";

/**
 * A titled group of settings rows (docs/DESIGN_SYSTEM.md §6, chrome).
 *
 * It is a `Section` with a `Panel` under it: the heading and its generated id come from the
 * section, the surface and its border from the panel, and the only thing this adds is the
 * row layout — full-bleed rows separated by a hairline, which is what makes a settings group
 * read as one control surface rather than a pile of boxes.
 *
 * Splitting it this way is what lets a screen that is not settings (Diagnostics, and every
 * screen after it) have titled regions without either borrowing this name or writing the
 * heading and its id out by hand.
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
  return (
    <Section className={className} description={description} title={title}>
      <Panel className="flex flex-col divide-y divide-border-subtle overflow-hidden">
        {children}
      </Panel>
    </Section>
  );
}
