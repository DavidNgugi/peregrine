import { clsx } from "clsx";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * One column of a `Table`.
 *
 * `numeric` is not only an alignment: it also turns on tabular figures, which §3 requires of
 * a numeric column, because proportional digits make a column of numbers jitter as it
 * changes.
 */
export interface TableColumn<Row> {
  /** Stable id, used as the column's key. */
  readonly id: string;
  readonly header: string;
  readonly numeric?: boolean;
  readonly cell: (row: Row) => ReactNode;
}

export interface TableProps<Row> {
  readonly columns: readonly TableColumn<Row>[];
  readonly rows: readonly Row[];
  /**
   * The row's stable domain id — never its index (§5, keys).
   *
   * A number is allowed because React accepts one and because the core's row ids are
   * integers; making the caller stringify its own identifier would be a conversion that
   * exists only to satisfy this signature.
   */
  readonly rowKey: (row: Row) => string | number;
  readonly className?: string;
}

/**
 * A table of few rows (docs/DESIGN_SYSTEM.md §6, collection).
 *
 * It is presentational on purpose: columns, rows, and a key, and nothing else. Sorting,
 * selection with shift and cmd/ctrl, column resizing, keyset paging, and virtualization are
 * PG-E21-18's, and a component that guessed at them now would be guessing at a data layer
 * that does not exist yet. What it does own is the part that was being got wrong at each call
 * site: `scope="col"` on every heading, tabular figures on every numeric column, and row
 * keys that come from the data rather than from the position, so a row that is replaced keeps
 * its identity.
 *
 * A table with no rows draws its head and nothing else. Whether that is a state worth showing
 * is the screen's decision, so the empty treatment belongs to the caller — `EmptyState` is
 * the block for it.
 */
export function Table<Row>({ columns, rows, rowKey, className }: TableProps<Row>): ReactElement {
  return (
    <table className={twMerge(clsx("w-full border-collapse text-caption"), className)}>
      <thead>
        <tr className="text-text-tertiary">
          {columns.map((column) => (
            <th
              // `twMerge`, not `clsx`: the base alignment and the numeric one are the same
              // class group, and leaving both to the stylesheet's order would put a numeric
              // heading on whichever side Tailwind happened to emit last.
              className={twMerge(
                clsx("py-1 pr-4 text-left", column.numeric === true && "text-right"),
              )}
              key={column.id}
              scope="col"
            >
              {column.header}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr className="border-t border-border-subtle text-text-secondary" key={rowKey(row)}>
            {columns.map((column) => (
              <td
                className={twMerge(
                  clsx("py-1 pr-4", column.numeric === true && "text-right tabular-nums"),
                )}
                key={column.id}
              >
                {column.cell(row)}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
