import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Table, type TableColumn } from "./Table.js";

interface Row {
  readonly id: number;
  readonly name: string;
  readonly score: number;
}

const columns: readonly TableColumn<Row>[] = [
  { id: "name", header: "Name", cell: (row) => row.name },
  { id: "score", header: "Score", numeric: true, cell: (row) => row.score },
];

/**
 * Fixture values are capitalised and deliberately unlike the design system's own words.
 *
 * The first version of this test used "first" and "second", and `indexOf("second")` found
 * `text-text-secondary` in the row's class before it found either cell — an assertion about
 * order that was really an assertion about a class name.
 */
const rows: readonly Row[] = [
  { id: 1, name: "Alpha", score: 10 },
  { id: 2, name: "Beta", score: 20 },
];

function table(source: readonly Row[]): string {
  return renderToStaticMarkup(<Table columns={columns} rowKey={(row) => row.id} rows={source} />);
}

describe("Table", () => {
  it("draws a heading per column and a row per row, in order", () => {
    const markup = table(rows);
    expect(markup).toContain("Name");
    expect(markup).toContain("Score");
    expect(markup.indexOf("Alpha")).toBeLessThan(markup.indexOf("Beta"));
    expect(markup.match(/<tr/gu)).toHaveLength(3);
  });

  it("scopes every column heading to its column", () => {
    // Without `scope`, a screen reader reads a table of figures as a run of numbers.
    const markup = table(rows);
    expect(markup.match(/scope="col"/gu)).toHaveLength(columns.length);
  });

  it("aligns a numeric column one way and no other", () => {
    // The heading and the cells must not both say `text-left` and `text-right`: which wins
    // would then be whichever Tailwind emitted last.
    const markup = table(rows);
    for (const cell of markup.matchAll(/<t[hd] class="([^"]*)"/gu)) {
      const classes = cell[1] ?? "";
      expect(classes.includes("text-left") && classes.includes("text-right")).toBe(false);
    }
  });

  it("gives a numeric column figures that do not jitter as they change (§3)", () => {
    const markup = table(rows);
    const scoreCells = [...markup.matchAll(/<td class="([^"]*)"/gu)].map((match) => match[1] ?? "");
    expect(scoreCells.filter((classes) => classes.includes("tabular-nums"))).toHaveLength(
      rows.length,
    );
    // The name column is not numeric and must not be right-aligned.
    expect(scoreCells.filter((classes) => classes.includes("text-right"))).toHaveLength(
      rows.length,
    );
  });

  it("draws its head and no body rows when there is nothing to show", () => {
    // Whether an empty table is worth a state is the screen's decision, so the empty
    // treatment is the caller's — `EmptyState` is the block for it.
    const markup = table([]);
    expect(markup).toContain("Name");
    expect(markup.match(/<tr/gu)).toHaveLength(1);
  });

  it("asks for the key of a row and not of a position", () => {
    // A key is not visible in markup, so the contract is asserted where it can be: the
    // function receives the row itself. An index-keyed table is the bug §5 names, and it
    // would give away that it had been handed a position.
    const asked: unknown[] = [];
    renderToStaticMarkup(
      <Table
        columns={columns}
        rowKey={(row) => {
          asked.push(row);
          return row.id;
        }}
        rows={rows}
      />,
    );
    expect(asked).toEqual([...rows]);
  });
});
