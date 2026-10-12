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
      nav: {
        label: "Screens",
      },
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
    jobs: {
      title: "Diagnostics",
      subtitle: "The scheduler's own view of the work this build can do.",
      degraded: "{{count}} updates were dropped; the numbers below may lag.",
      error: {
        dismiss: "Dismiss",
      },
      action: {
        start: "Start a hello job",
        cancel: "Cancel the job",
        refresh: "Refresh",
      },
      progress: {
        label: "Job progress",
        idle: "No job has run in this session.",
        value: "{{phase}} · {{percent}}%",
      },
      journal: {
        title: "Job journal",
        empty: "No job has been recorded yet.",
        id: "ID",
        kind: "Kind",
        state: "State",
        progress: "Progress",
        attempts: "Attempts",
      },
      log: {
        title: "Recent log",
        empty: "Nothing has been logged yet.",
      },
    },
    shortcuts: {
      discover: "Go to Discover",
      jobs: "Go to Diagnostics",
      settings: "Go to Settings",
    },
    settings: {
      title: "Settings",
      shortcuts: {
        title: "Keyboard shortcuts",
        description: "Every shortcut this build has, written the way this platform writes them.",
      },
      autostart: {
        title: "Start with your computer",
        description:
          "Peregrine does not register itself to start with your computer unless you ask it to, and turning this off removes the entry the system keeps.",
        label: "Start Peregrine when you sign in",
        failed:
          "The system would not change whether Peregrine starts with it. Your account may not allow it.",
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
        conflict: {
          title: "Something else is already doing that.",
          advice: "Wait for the current operation to finish, then try again.",
        },
        storage_busy: {
          title: "The library database is busy.",
          advice: "Another operation is writing to it. Try again in a moment.",
        },
        storage_corrupt: {
          title: "The library database is damaged.",
          advice:
            "Peregrine has to rebuild it. Export diagnostics from Settings before you do, so the cause can be reported.",
        },
        permission_denied: {
          title: "Peregrine is not allowed to do that.",
          advice: "Grant the permission in your system settings, then try again.",
        },
        disk_full: {
          title: "There is not enough free space.",
          advice: "Free some space or choose another download folder, then resume the download.",
        },
        network: {
          title: "The network is unavailable.",
          advice: "Check the connection. Cached titles and the local library still work.",
        },
        provider_rate_limited: {
          title: "The provider asked Peregrine to slow down.",
          advice: "It will retry on its own. Waiting a few minutes is enough.",
        },
        provider_unavailable: {
          title: "The provider could not be reached.",
          advice: "Check the connection. Cached titles and the local library still work.",
        },
        provider_schema_drift: {
          title: "A provider changed how it answers.",
          advice: "Peregrine shows what it already has and refreshes later. Nothing is lost.",
        },
        unsafe_content: {
          title: "The download was stopped by the safety check.",
          advice: "The files inside did not match the release. Choose a different release.",
        },
        playback_failed: {
          title: "Playback stopped unexpectedly.",
          advice: "Peregrine can resume from the last watched position.",
        },
        device_unavailable: {
          title: "That device is no longer reachable.",
          advice: "Pick another device, or play here instead.",
        },
        cancelled: {
          title: "That was cancelled.",
          advice: "Start it again when you are ready.",
        },
        unsupported: {
          title: "That is not available on this setup.",
          advice: "Peregrine cannot do it on this platform or with the current configuration.",
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
