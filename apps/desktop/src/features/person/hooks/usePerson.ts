import { useEffect, useState } from "react";
import { commands, normalizeIpcError } from "@peregrine/ipc-client";
import type { IpcError, LibraryTitleDto } from "@peregrine/ipc-client";

/** What the person screen knows while its read is in flight. */
export interface PersonState {
  readonly name: string | null;
  readonly profile: string | null;
  readonly titles: readonly LibraryTitleDto[];
  readonly loading: boolean;
  readonly failed: IpcError | null;
}

const EMPTY: PersonState = {
  name: null,
  profile: null,
  titles: [],
  loading: true,
  failed: null,
};

/** Reads one person and the library titles credited to them. */
export function usePerson(id: number | null): PersonState {
  const [state, setState] = useState<PersonState>(EMPTY);

  useEffect(() => {
    if (id === null) {
      setState({ ...EMPTY, loading: false });
      return;
    }
    let live = true;
    setState(EMPTY);
    void commands
      .getPerson({ id })
      .then((result) => {
        if (!live) {
          return;
        }
        if (result.status !== "ok") {
          setState({ ...EMPTY, loading: false, failed: normalizeIpcError(result.error) });
          return;
        }
        const page = result.data;
        setState(
          page === null
            ? { ...EMPTY, loading: false }
            : {
                name: page.name,
                profile: page.profile,
                titles: page.titles,
                loading: false,
                failed: null,
              },
        );
      })
      .catch((caught: unknown) => {
        if (live) {
          setState({ ...EMPTY, loading: false, failed: normalizeIpcError(caught) });
        }
      });
    return () => {
      live = false;
    };
  }, [id]);

  return state;
}
