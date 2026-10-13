/**
 * Every primitive in `packages/ui`, in every state the design system requires of it.
 *
 * The design system asks for this in §12: "component gallery | static build of `packages/ui` |
 * visual regression baseline per theme and OS". A primitive that cannot be looked at is a
 * primitive nobody reviews — the only way to judge a focus ring, a disabled state, or whether a
 * dark-theme accent really carries `--text-on-accent` is to see them side by side, in both
 * themes, at once.
 *
 * It is a *second entry point of the app's build* rather than a package of its own.
 * `packages/ui` has no bundler, no Tailwind configuration, and no browser target, and giving it
 * one would be a second build chain to keep in step for a page that only reads those packages.
 * The app already resolves them from source and already has the theme tokens, and `vite build`
 * emits both entries — so the gallery is built by the same command that builds the app, which
 * is what makes it checkable in CI.
 *
 * `GALLERY_COMPONENTS` is what keeps it honest: `Gallery.test.tsx` asserts it against the
 * package's real exports, so a primitive added to the kit and not shown here fails a test
 * rather than being discovered missing during a review that never happens.
 */

import {
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  ICON_NAMES,
  IconButton,
  Panel,
  Progress,
  Screen,
  Section,
  SettingsRow,
  SettingsSection,
  ShortcutKeys,
  Sidebar,
  SidebarItem,
  Switch,
  Table,
  type TableColumn,
} from "@peregrine/ui";
import type { ReactElement, ReactNode } from "react";
import { useState } from "react";

/**
 * Every primitive this page shows, by its exported name.
 *
 * It is data rather than a comment because the test reads it: `Gallery.test.tsx` checks both
 * directions — that each name here is really exported, and that each function `@peregrine/ui`
 * exports appears here.
 */
export const GALLERY_COMPONENTS: readonly string[] = [
  "Button",
  "Dialog",
  "EmptyState",
  "ErrorState",
  "Icon",
  "IconButton",
  "Panel",
  "Progress",
  "Screen",
  "Section",
  "SettingsRow",
  "SettingsSection",
  "ShortcutKeys",
  "Sidebar",
  "SidebarItem",
  "Switch",
  "Table",
];

/** The two themes the token pipeline ships. */
type GalleryTheme = "dark" | "light";

function useGalleryTheme(): readonly [GalleryTheme, () => void] {
  const [theme, setTheme] = useState<GalleryTheme>("dark");

  return [
    theme,
    () => {
      const next: GalleryTheme = theme === "dark" ? "light" : "dark";
      document.documentElement.setAttribute("data-theme", next);
      setTheme(next);
    },
  ] as const;
}

function Section_({
  component,
  description,
  children,
}: {
  readonly component: string;
  readonly description: string;
  readonly children: ReactNode;
}): ReactElement {
  return (
    <section className="flex flex-col gap-3 border-t border-border-subtle pt-6">
      <div className="flex flex-col gap-1">
        <h2 className="text-title-2 text-text-primary" data-component={component}>
          {component}
        </h2>
        <p className="text-caption text-text-tertiary">{description}</p>
      </div>
      <div className="flex flex-wrap items-start gap-3">{children}</div>
    </section>
  );
}

/** A fixed box, for primitives that fill their container. */
function Box({ children }: { readonly children: ReactNode }): ReactElement {
  return (
    <div className="h-64 w-full overflow-hidden rounded-md border border-border-subtle">
      {children}
    </div>
  );
}

interface Row {
  readonly id: number;
  readonly kind: string;
  readonly size: number;
}

const ROWS: readonly Row[] = [
  { id: 1, kind: "hello", size: 4096 },
  { id: 2, kind: "fetch", size: 8192 },
];

const COLUMNS: readonly TableColumn<Row>[] = [
  { id: "id", header: "ID", numeric: true, cell: (row) => row.id },
  { id: "kind", header: "Kind", cell: (row) => row.kind },
  { id: "size", header: "Size", numeric: true, cell: (row) => row.size },
];

/** The dialog's demo: closed until the reader opens it, because an open one covers the gallery. */
function GalleryDialog(): ReactElement {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex flex-col items-start gap-2">
      <Button
        onClick={() => {
          setOpen(true);
        }}
        size="sm"
      >
        Open the dialog
      </Button>
      {open ? (
        <Dialog
          closeLabel="Close"
          description="One decision, and nothing else on the page can be reached until it is answered."
          onClose={() => {
            setOpen(false);
          }}
          title="Forget this rule?"
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setOpen(false);
              }}
              size="sm"
              variant="secondary"
            >
              Cancel
            </Button>
            <Button
              onClick={() => {
                setOpen(false);
              }}
              size="sm"
              variant="danger"
            >
              Forget
            </Button>
          </div>
        </Dialog>
      ) : null}
    </div>
  );
}

export function Gallery(): ReactElement {
  const [theme, toggleTheme] = useGalleryTheme();
  const [on, setOn] = useState(true);
  const [busy, setBusy] = useState(false);

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6 px-6 py-8">
      <header className="flex items-baseline justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="text-title-1 text-text-primary">Peregrine component gallery</h1>
          <p className="text-caption text-text-tertiary">
            Every primitive in <code>packages/ui</code>, in both themes and in the states the design
            system requires of it.
          </p>
        </div>
        <Button onClick={toggleTheme} size="sm" variant="secondary">
          {theme === "dark" ? "Light theme" : "Dark theme"}
        </Button>
      </header>

      <Section_
        component="Button"
        description="Three variants, two sizes, and disabled. Tab through them to see the focus ring."
      >
        <Button variant="primary">Primary action</Button>
        <Button variant="secondary">Secondary</Button>
        <Button variant="ghost">Ghost</Button>
        <Button size="sm">Small</Button>
        <Button size="sm" variant="primary">
          Small primary
        </Button>
        <Button disabled>Disabled</Button>
        <Button variant="primary">Primary with a much longer label</Button>
      </Section_>

      <Section_
        component="IconButton"
        description="An icon alone takes a required label, because an icon alone has no name."
      >
        <IconButton icon="sun" label="Switch to the light theme" />
        <IconButton icon="moon" label="Switch to the dark theme" size="sm" />
        <IconButton icon="tune" label="Settings" variant="secondary" />
        <IconButton disabled icon="pulse" label="Unavailable" />
      </Section_>

      <Section_
        component="Icon"
        description={`The whole set (${String(ICON_NAMES.length)} marks) at each size §9 allows: 16, 20, 24, 32.`}
      >
        {ICON_NAMES.map((name) => (
          <span className="flex items-center gap-2" key={name}>
            <Icon name={name} size={16} />
            <Icon name={name} size={20} />
            <Icon name={name} size={24} />
            <Icon name={name} size={32} />
            <span className="text-caption text-text-tertiary">{name}</span>
          </span>
        ))}
      </Section_>

      <Section_
        component="Switch"
        description="role=switch with aria-checked, a 28 px target around a 20 px track, and a busy state that refuses a second press."
      >
        {/* A switch is a full-width settings row, so it is shown at the width one is used
            at: loose in a wrapping flex row it would put the track half a screen from the
            label and misrepresent the component. */}
        <div className="flex w-96 flex-col divide-y divide-border-subtle overflow-hidden rounded-md border border-border-subtle bg-surface-1">
          <Switch checked={on} label="Notify me" onChange={setOn} />
          <Switch checked={false} label="Start with my computer" onChange={() => undefined} />
          <Switch busy checked={busy} label="Saving…" onChange={setBusy} />
          <Switch checked disabled label="Managed by the system" onChange={() => undefined} />
        </div>
      </Section_>

      <Section_
        component="ShortcutKeys"
        description="The accelerator as the core rendered it. Composing it here would be a second table free to disagree with the keys the shell dispatches on."
      >
        <ShortcutKeys display="⌘1" />
        <ShortcutKeys display="⌥⇧⌘K" />
        <ShortcutKeys display="Ctrl+Shift+P" />
      </Section_>

      <Section_
        component="Sidebar"
        description="The labelled navigation landmark: 232 px, a 60 px rail below the lg breakpoint, and a content area beside it."
      >
        <Box>
          <div className="flex h-full">
            <Sidebar label="Gallery navigation">
              <SidebarItem current icon="compass" label="Discover" onSelect={() => undefined} />
              <SidebarItem icon="pulse" label="Diagnostics" onSelect={() => undefined} />
            </Sidebar>
            <div className="flex-1 bg-surface-0 p-4 text-caption text-text-tertiary">
              Content area
            </div>
          </div>
        </Box>
      </Section_>

      <Section_
        component="SidebarItem"
        description="One screen's row: a 32 px target, aria-current on the one on show, and the token focus ring. The 60 px rail it falls back to is a viewport breakpoint, so narrow the window below lg to see it — a narrowed container is not the same thing."
      >
        <Box>
          <div className="flex h-full">
            <Sidebar label="Rows">
              <SidebarItem
                current
                icon="compass"
                label="A screen on show"
                onSelect={() => undefined}
              />
              <SidebarItem icon="pulse" label="Another screen" onSelect={() => undefined} />
              <SidebarItem icon="tune" label="A third" onSelect={() => undefined} />
            </Sidebar>
            <div className="flex-1 bg-surface-0 p-4 text-caption text-text-tertiary">
              Content area
            </div>
          </div>
        </Box>
      </Section_>

      <Section_
        component="SettingsSection"
        description="A titled group of rows: the heading and its generated id from Section, the surface from Panel, and full-bleed rows separated by hairlines."
      >
        <SettingsSection description="What the list is, in one line." title="Keyboard shortcuts">
          <SettingsRow label="Go to Discover">
            <ShortcutKeys display="⌘1" />
          </SettingsRow>
          <SettingsRow label="Go to Diagnostics">
            <ShortcutKeys display="⌘2" />
          </SettingsRow>
        </SettingsSection>
      </Section_>

      <Section_
        component="SettingsRow"
        description="A label, an optional line under it, and a trailing control. The description sits beside the label so it cannot become part of a control's accessible name."
      >
        <div className="w-full max-w-lg">
          <SettingsSection title="A group of rows">
            <SettingsRow description="A second line explaining the consequence." label="A row">
              <span className="text-caption text-text-secondary">value</span>
            </SettingsRow>
            <SettingsRow label="A row with a control">
              <Switch checked label="Inline switch" onChange={() => undefined} />
            </SettingsRow>
          </SettingsSection>
        </div>
      </Section_>

      <Section_
        component="Section"
        description="A titled region of a screen: a second-level heading and the id that names it."
      >
        <div className="w-full">
          <Section description="What this region holds." title="Job journal">
            <p className="text-caption text-text-tertiary">region content</p>
          </Section>
        </div>
      </Section_>

      <Section_
        component="Screen"
        description="The whole screen scaffold: the one h1 a page has, the inset, and the scroll that keeps the chrome and the player bar still."
      >
        <Box>
          <Screen subtitle="What this screen is" title="Diagnostics">
            <p className="text-caption text-text-tertiary">screen content</p>
          </Screen>
        </Box>
      </Section_>

      <Section_
        component="Dialog"
        description="A surface above the page holding one decision: role=dialog with aria-modal, a heading it is labelled by, the scrim behind it, and §4's --elev-3 shadow plus a border. Escape closes the top overlay only, which is the rule overlay-stack owns."
      >
        <GalleryDialog />
      </Section_>

      <Section_
        component="Panel"
        description="The bordered surface a block sits on. No shadow: §4 allows elevation only for what floats above the page."
      >
        <Panel className="w-64 p-4">
          <p className="text-caption text-text-secondary">A panel</p>
        </Panel>
        <Panel className="w-64 divide-y divide-border-subtle">
          <p className="p-3 text-caption text-text-secondary">Divided row</p>
          <p className="p-3 text-caption text-text-secondary">Divided row</p>
        </Panel>
      </Section_>

      <Section_
        component="Progress"
        description="A determinate bar. The fill moves with scaleX and never with width, so it composites instead of laying out."
      >
        <div className="flex w-full flex-col gap-2">
          <Progress label="Empty" value={0} />
          <Progress label="Half" value={42} />
          <Progress label="Over the top, and clamped" value={140} />
        </div>
      </Section_>

      <Section_
        component="Table"
        description="Presentational: scope on every heading, tabular figures on a numeric column, and a row key from the data."
      >
        <div className="w-full">
          <Table columns={COLUMNS} rowKey={(row) => row.id} rows={ROWS} />
        </div>
      </Section_>

      <Section_
        component="EmptyState"
        description="A state with nothing to show. An inline one takes the row's type; a page one has to carry the screen."
      >
        <Panel className="w-80">
          <EmptyState title="Nothing here yet" />
        </Panel>
        <Panel className="w-80">
          <EmptyState
            body="What to do about it, in one line."
            size="page"
            title="Nothing here yet"
          />
        </Panel>
      </Section_>

      <Section_
        component="ErrorState"
        description="A failure the user can read where it happened, announced with role=alert, with the tone on the border because §2.2 holds danger to 3:1 and body text to 4.5:1."
      >
        <div className="w-full">
          <ErrorState
            action={
              <Button size="sm" variant="ghost">
                Dismiss
              </Button>
            }
            body="Why it happened, or what to do about it."
            title="Something went wrong inside Peregrine."
          />
        </div>
      </Section_>
    </div>
  );
}
