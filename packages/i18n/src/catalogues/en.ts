/**
 * English messages.
 *
 * Copy rules are in docs/DESIGN_SYSTEM.md §1.2: sentence case, no exclamation
 * marks, and an error says what happened, why, and what to do next. The
 * `ipc.error.*` keys are the other half of `@peregrine/ipc-client`'s code mapping.
 */
export const en = {
  translation: {
    app: {
      name: "Peregrine",
    },
    shell: {
      theme: {
        switchToLight: "Switch to the light theme",
        switchToDark: "Switch to the dark theme",
      },
      nowPlaying: {
        idle: "Nothing playing",
      },
      error: {
        retry: "Try again",
        code: "Code {{code}}",
      },
    },
    discover: {
      title: "Discover",
      empty: {
        title: "Nothing here yet",
        body: "Peregrine has no catalogue data yet. Titles appear here once discovery is available.",
      },
    },
    ipc: {
      error: {
        invalid_input: {
          title: "That request was not accepted.",
          advice: "It came from Peregrine itself, so please report the steps that led here.",
        },
        not_found: {
          title: "That item is no longer there.",
          advice: "It may have been removed or moved. Reload the screen to see the current state.",
        },
        permission_denied: {
          title: "Peregrine is not allowed to do that.",
          advice: "Grant the permission in your system settings, then try again.",
        },
        unsafe_content: {
          title: "The download was stopped by the safety check.",
          advice: "The files inside did not match the release. Choose a different release.",
        },
        disk_full: {
          title: "There is not enough free space.",
          advice: "Free some space or choose another download folder, then resume the download.",
        },
        rate_limited: {
          title: "The provider asked Peregrine to slow down.",
          advice: "It will retry on its own. Waiting a few minutes is enough.",
        },
        provider_unavailable: {
          title: "The provider could not be reached.",
          advice: "Check the connection. Cached titles and the local library still work.",
        },
        database_busy: {
          title: "The library database is busy.",
          advice: "Another operation is writing to it. Try again in a moment.",
        },
        internal: {
          title: "Something went wrong inside Peregrine.",
          advice:
            "Try again. If it keeps happening, export diagnostics from Settings and quote the code.",
        },
      },
    },
  },
};
