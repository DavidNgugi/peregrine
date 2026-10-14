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
  CARD_SIZES,
  Dialog,
  Grid,
  EmptyState,
  ErrorState,
  Icon,
  ICON_NAMES,
  IconButton,
  Link,
  Panel,
  Progress,
  Rail,
  Screen,
  Section,
  SettingsRow,
  SettingsSection,
  ShortcutKeys,
  Sidebar,
  SidebarItem,
  Switch,
  Table,
  TextInput,
  TitleCard,
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
  "Grid",
  "Icon",
  "IconButton",
  "Link",
  "Panel",
  "Progress",
  "Rail",
  "Screen",
  "Section",
  "SettingsRow",
  "SettingsSection",
  "ShortcutKeys",
  "Sidebar",
  "SidebarItem",
  "Switch",
  "Table",
  "TextInput",
  "TitleCard",
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

/** The dialog's demo: closed until the reader opens it, because an open one covers the gallery. */ function GalleryDialog(): ReactElement {
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

/**
 * A stand-in poster for the card section.
 *
 * The gallery has no image cache and no provider, and a tile with nothing in it cannot be reviewed.
 * A data URI rather than a file in `public/`: the app's own CSP already allows `img-src … data:`, so
 * this adds no asset the build has to carry and no second copy of the same picture for a screenshot.
 */
const POSTER = `data:image/svg+xml,${encodeURIComponent(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 300">
    <defs>
      <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#1b1d3a"/>
        <stop offset="0.55" stop-color="#4a3a63"/>
        <stop offset="1" stop-color="#8a5a52"/>
      </linearGradient>
    </defs>
    <rect width="200" height="300" fill="url(#sky)"/>
    <circle cx="132" cy="96" r="34" fill="#f2e6c8" opacity="0.92"/>
    <path d="M0 214c38-16 74 4 106-8s66-30 94-18v112H0Z" fill="#241f36"/>
    <path d="M0 250c46-12 82 12 118 2s58-16 82-8v56H0Z" fill="#15121f"/>
  </svg>`,
)}`;

/**
 * A rail's worth of titles for the card and rail sections.
 *
 * Twelve of them so the rail overflows the gallery's own column width and both edge affordances have
 * something to answer for: a rail that fits on screen shows neither chevron, and showing only that
 * state would be showing the one case where the component does nothing.
 *
 * Each one carries what a list answer carries — a synopsis and a vote count — because the hover
 * preview draws them (`PG-E7-05`) and a gallery entry with nothing behind the ladder would be a
 * gallery entry that cannot be reviewed. Two deliberately have neither, so the panel's empty and
 * partial states are on the page rather than only in a test.
 */
const RAIL_TITLES = [
  {
    id: 1,
    kind: "movie",
    meta: "1979 · Film",
    rating: 8.5,
    title: "Alien",
    overview:
      "The crew of a commercial towing ship answers a distress call and finds something aboard that should not be there.",
    votes: 14_208,
  },
  {
    id: 2,
    kind: "movie",
    meta: "2010 · Film",
    rating: 8.8,
    title: "Inception",
    overview: "A thief who enters dreams to steal secrets is asked to plant one instead.",
    votes: 37_402,
  },
  {
    id: 3,
    kind: "movie",
    meta: "1999 · Film",
    rating: 8.7,
    title: "The Matrix",
    overview: "A hacker learns that the world he lives in is a construction, and what it is for.",
    votes: 26_113,
  },
  {
    id: 4,
    kind: "series",
    meta: "2008 · Series",
    rating: 9.5,
    title: "Breaking Bad",
    overview: null,
    votes: 15_004,
  },
  {
    id: 5,
    kind: "movie",
    meta: "2014 · Film",
    rating: 8.6,
    title: "Interstellar",
    overview: "A farmer and former pilot leaves his children to look for a home for everyone else.",
    votes: 35_901,
  },
  {
    id: 6,
    kind: "series",
    meta: "2019 · Series",
    rating: 8.7,
    title: "The Expanse",
    overview: "Three factions, one missing woman, and a system that is not as empty as it looks.",
    votes: 4_882,
  },
  {
    id: 7,
    kind: "movie",
    meta: "1982 · Film",
    rating: 8.1,
    title: "Blade Runner",
    overview: "A retired officer is brought back to retire four who came back on their own.",
    votes: 13_540,
  },
  {
    id: 8,
    kind: "movie",
    meta: "2016 · Film",
    rating: 7.8,
    title: "Arrival",
    overview: "A linguist is asked to talk to something that does not experience time as she does.",
    votes: 18_770,
  },
  {
    id: 9,
    kind: "series",
    meta: "2002 · Series",
    rating: 9.2,
    title: "The Wire",
    overview: null,
    votes: null,
  },
  {
    id: 10,
    kind: "movie",
    meta: "1968 · Film",
    rating: 8.3,
    title: "2001: A Space Odyssey",
    overview: "A monolith, a mission, and the one member of the crew that never sleeps.",
    votes: 11_920,
  },
  {
    id: 11,
    kind: "movie",
    meta: "2019 · Film",
    rating: 7.9,
    title: "Ad Astra",
    overview:
      "An astronaut goes looking for the father whose mission everyone else would rather forget.",
    votes: 8_311,
  },
  {
    id: 12,
    kind: "series",
    meta: "2020 · Series",
    rating: 8.4,
    title: "Raised by Wolves",
    overview: null,
    votes: 2_100,
  },
] as const;

/**
 * Five thousand tiles for the grid section.
 *
 * Five thousand because that is the number PG-E7-06's acceptance names, and a gallery entry that showed
 * twelve would be showing the case the component does not exist for. The ids are the domain ids the
 * tiles are keyed by, not positions.
 */
const GRID_TITLES = Array.from({ length: 5000 }, (_, index) => ({
  id: index + 1,
  title: `Title ${String(index + 1)}`,
}));

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
        component="Grid"
        description="A virtualized grid of fixed-geometry tiles: 5 000 items in this section, and the DOM holds only the rows on screen plus one of overscan. Scroll inside it, or Tab to it and use the arrow keys the platform gives a scroller."
      >
        <div className="h-80 w-full">
          <Grid
            gap={12}
            itemKey={(item) => item.id}
            items={GRID_TITLES}
            label="Five thousand titles"
            renderItem={(item) => (
              <TitleCard
                detailsLabel="Details"
                href={`/title/movie/${String(item.id)}`}
                overview="One tile of five thousand, at the geometry §4 fixes."
                poster={POSTER}
                size="sm"
                title={item.title}
              />
            )}
            rowHeight={180}
            tileWidth={112}
          />
        </div>
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
        component="Link"
        description="An anchor that says where it goes, with the platform's focus ring. Given onNavigate it takes the click rather than letting the webview reload, and it leaves a modified click to the platform: cmd-click is a request for a new window, not for the app to decide. With a variant it is drawn as a button — Discover's hero puts a Play button and a Details link side by side — and it stays an anchor, so the destination is still in the markup and a context menu can still offer it."
      >
        <div className="flex flex-col items-start gap-2">
          <Link href="/title/movie/7">A title's own screen</Link>
          <Link href="/library">The library</Link>
          <div className="flex items-center gap-2">
            <Link href="/title/movie/7" variant="primary">
              Details
            </Link>
            <Link href="/library" variant="secondary">
              Secondary
            </Link>
            <Link href="/library" size="sm" variant="ghost">
              Small
            </Link>
          </div>
        </div>
      </Section_>

      <Section_
        component="Rail"
        description="A row of tiles that pages sideways. One tab stop for the whole rail, the arrows move inside it, and each chevron appears only on an edge there is still somewhere to go from. Tab to it, then press ← → Home End."
      >
        <div className="w-full">
          <Rail
            itemKey={(item) => item.id}
            items={RAIL_TITLES}
            renderItem={(item, tabIndex) => (
              <TitleCard
                detailsLabel="Details"
                href={`/title/${item.kind}/${item.id}`}
                meta={item.meta}
                overview={item.overview}
                poster={POSTER}
                rating={item.rating}
                ratingLabel={`rated ${String(item.rating)} out of 10`}
                size="md"
                tabIndex={tabIndex}
                title={item.title}
                votesLabel={item.votes === null ? null : `${item.votes.toLocaleString("en")} votes`}
              />
            )}
            title="Trending this week"
          />
        </div>
      </Section_>

      <Section_
        component="TitleCard"
        description="A poster tile, and the hover preview PG-E7-05 is judged on. Rest a pointer on one for 450 ms: it grows to 1.06 over 180 ms with the metadata scrimmed inside the enlarged layer, and the tiles beside it do not move. Leave, scroll, hide the tab, or scroll it below 60% and it cancels; Tab to it and the panel appears without the scale. The 2:3 box is reserved before the bytes arrive, so a late, failed, or never-cached image never moves the grid; every word comes from the caller, because the kit holds no copy."
      >
        <div className="flex w-full flex-col gap-6">
          <div className="flex flex-wrap items-end gap-3">
            {CARD_SIZES.map((size) => (
              <div className="flex flex-col items-start gap-1" key={size}>
                <TitleCard
                  availability="acquiring"
                  availabilityLabel="Downloading"
                  detailsLabel="Details"
                  href="/title/movie/1"
                  meta="1979 · Film"
                  overview="The crew of a commercial towing ship answers a distress call and finds something aboard that should not be there."
                  poster={POSTER}
                  rating={8.5}
                  ratingLabel="rated 8.5 out of 10"
                  size={size}
                  title="Alien"
                  votesLabel="14,208 votes"
                />
                <span className="text-micro text-text-tertiary">{size}</span>
              </div>
            ))}
          </div>
          <div className="flex flex-wrap items-start gap-3">
            {/* The four states the revealed panel can be in, side by side: everything a list
                answer carries, prose with no score, a score with no prose, and a title the provider
                said nothing about at all. */}
            <TitleCard
              detailsLabel="Details"
              href="/title/movie/2"
              meta="2010 · Film"
              overview="A thief who enters dreams to steal secrets is asked to plant one instead."
              poster={POSTER}
              rating={7.25}
              ratingLabel="rated 7.3 out of 10"
              size="lg"
              title="Inception"
              votesLabel="37,402 votes"
            />
            <TitleCard
              availability="unavailable"
              availabilityLabel="Unavailable"
              detailsLabel="Details"
              href="/title/movie/3"
              meta="1999 · Film"
              overview="A hacker learns that the world he lives in is a construction, and what it is for."
              poster={POSTER}
              size="lg"
              title="The Matrix"
              votesLabel="26,113 votes"
            />
            <TitleCard
              availability="watchlisted"
              availabilityLabel=""
              detailsLabel="Details"
              href="/title/movie/4"
              meta="2019 · Film"
              placeholder="oklch(0.45 0.06 250)"
              poster={null}
              rating={8.4}
              ratingLabel="rated 8.4 out of 10"
              size="lg"
              title="No poster cached"
            />
            <TitleCard
              href="/title/series/5"
              meta="2008 · Series"
              priority
              size="lg"
              title="No artwork at all"
            />
          </div>
        </div>
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
        component="TextInput"
        description="A line of text with the label and the message that belong to it: the id is minted inside so the label cannot be left unconnected, and a message that is a failure carries role=alert rather than only a colour."
      >
        <div className="flex w-72 flex-col gap-3">
          <TextInput label="Search your catalogue" placeholder="A film or series name" />
          <TextInput label="With help" message="Full titles work better than fragments." />
          <TextInput invalid label="Refused" message="That query is empty." value="" />
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
