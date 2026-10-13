/**
 * The picker: search the catalogue, then file the file under the title you chose.
 *
 * What the queue could not do before a dialog existed. A parked file's name may read a title that is
 * neither right nor close to anything the library holds — a foreign title, a remake, a film filed
 * under a name the parser turned into something else — and only a person can say which title it is.
 *
 * The search is the kit's `TextInput` and the results are the kit's list of buttons rather than the
 * `Table` the screens use: a picker is one decision, and a table with columns would ask the user to
 * read a row before they can read a name.
 *
 * **What is owed here:** choosing an *episode* rather than a title (`PG-E14-07`'s numbering, which is
 * what a series' file needs), and a way to create a title a search does not find — the catalogue is
 * the provider's (`PG-E5`), and nothing in this build creates a title except the library itself.
 */

import { Button, Dialog, EmptyState, TextInput } from "@peregrine/ui";
import type { TitleMatchDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useTitleSearch } from "../hooks/useTitleSearch.js";

interface TitlePickerProps {
  /** The file being placed, as the dialog's description names it. */
  readonly fileName: string;
  /** Files the file under the chosen title and closes. */
  readonly onChoose: (title: TitleMatchDto) => void;
  /** Closes without choosing anything. */
  readonly onClose: () => void;
}

/** The picker dialog. */
export function TitlePicker({ fileName, onChoose, onClose }: TitlePickerProps): ReactElement {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const search = useTitleSearch(query);

  return (
    <Dialog
      closeLabel={t("common.close")}
      description={t("library.picker.description", { file: fileName })}
      onClose={onClose}
      title={t("library.picker.title")}
    >
      <TextInput
        autoFocus
        label={t("library.picker.search")}
        onChange={(event) => {
          setQuery(event.target.value);
        }}
        placeholder={t("library.picker.placeholder")}
        type="search"
        value={query}
      />

      {search.failed ? (
        <EmptyState
          body={t("library.picker.failed.body")}
          size="inline"
          title={t("library.picker.failed.title")}
        />
      ) : search.idle || search.results.length === 0 ? (
        // One block for both, because they say the same thing to the user: nothing to choose from
        // yet. A separate "no results" for an empty query would be answering nothing.
        <EmptyState
          body={t("library.picker.empty.body")}
          size="inline"
          title={t("library.picker.empty.title")}
        />
      ) : (
        <ul className="flex max-h-64 flex-col gap-1 overflow-y-auto">
          {search.results.map((result) => (
            <li key={result.id}>
              <Button
                className="w-full justify-start"
                onClick={() => {
                  onChoose(result);
                }}
                size="sm"
                variant="ghost"
              >
                {result.name}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}
