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
    common: {
      close: "Close",
      cancel: "Cancel",
    },
    title: {
      section: "In your library",
      column: {
        items: "Items",
        episode: "Episode",
        name: "Name",
      },
      unnamedEpisode: "Not named yet",
      episodes: {
        title: "Episodes",
        description: "Every episode the library holds, in the order they are watched.",
      },
      missing: {
        title: "This title is not here.",
        body: "Nothing in your library has this id. It may have been a link to something this installation does not have.",
      },
      unavailable: {
        title: "This title could not be read.",
        body: "Peregrine's core did not answer, so this page is unknown rather than empty. Going back and returning tries the read again.",
      },
    },
    library: {
      title: "Library",
      section: "What is here",
      description:
        "Every title the scans have found in your folders, with how many copies and how much space it takes. Nothing here is downloaded or moved.",
      scan: {
        action: "Scan now",
        working: "Scanning",
        running: "Scanning your folders…",
        idle: "No scan has run from here yet.",
        finished: "Scan finished.",
        canceled: "The last scan was cancelled.",
        failed: "The last scan did not finish.",
        refused: "That scan could not be started.",
      },
      queue: {
        title: "Needs a decision",
        description:
          "Files a scan found but could not place, with what their names said. Matching one files it under the title its name read.",
        column: {
          file: "File",
          why: "Why not",
        },
        match: "Match",
        matchUnder: "File under {{title}}",
        looksLike: "Looks like {{title}}",
        noReason: "—",
        nothingRead: "Nothing was read from this name",
        refused: "That file could not be matched.",
        empty: {
          title: "Nothing is waiting.",
          body: "Every file a scan found has been placed.",
        },
      },
      rules: {
        title: "Remembered",
        description:
          "Answers you have given: when a name reads this way, it is filed under that title without asking again. Forgetting a rule does not move the files it has already placed.",
        column: {
          name: "A name that reads",
          placedUnder: "Is filed under",
        },
        readsAs: "remembered from a correction",
        forget: "Forget",
        refused: "That rule could not be forgotten.",
        confirm: {
          title: "Forget this rule?",
          body: "Names that read as “{{pattern}}” will be asked about again. The files it has already placed stay where they are.",
        },
      },
      column: {
        title: "Title",
        kind: "Kind",
        files: "Copies",
        size: "Size",
        missing: "Missing",
      },
      kind: {
        films: "Film",
        series: "Series",
      },
      stub: "No metadata yet",
      size: {
        none: "—",
        value: "{{value}} {{unit}}",
      },
      empty: {
        title: "Nothing has been scanned in yet.",
        body: "Add a folder in Settings, then run a scan. Titles appear here as the scan finds them.",
      },
      unavailable: {
        title: "Your library could not be read.",
        body: "Peregrine's core did not answer, so this list is unknown rather than empty. Reopening the app tries the read again.",
      },
    },
    settings: {
      title: "Settings",
      shortcuts: {
        title: "Keyboard shortcuts",
        description: "Every shortcut this build has, written the way this platform writes them.",
        empty: "No shortcut is registered in this build.",
        unavailable: {
          title: "The shortcut list could not be read.",
          body: "Peregrine's core did not answer, so this list is unknown rather than short. Reopening the app tries the read again.",
        },
      },
      libraries: {
        title: "Media folders",
        description:
          "The folders Peregrine is allowed to scan. Removing one stops it being scanned: nothing is deleted, and what you have watched stays.",
        remove: "Remove",
        refused: {
          title: "That folder could not be added.",
        },
        unavailable: {
          title: "Your folders could not be read.",
          body: "Peregrine's core did not answer, so this list is unknown rather than empty. Reopening the app tries the read again.",
        },
        empty: {
          title: "No folders yet.",
          body: "Add the folder your films or series are in. Peregrine scans it where it is and moves nothing.",
        },
        add: {
          label: "Add a folder",
          description:
            "Peregrine asks the system for a folder and scans what is in it. Nothing is moved, renamed, or copied.",
          action: "Add folder",
        },
        kind: {
          movies: "Films",
          shows: "Series",
          mixed: "Films and series",
          downloads: "Downloads",
        },
        scan: {
          label: "Scan now",
          description:
            "Walks every folder above and updates what Peregrine knows about them. Nothing is moved or renamed, and a folder that cannot be reached is reported rather than treated as empty.",
          action: "Scan now",
        },
        neverScanned: "{{kind}} · not scanned yet",
        scanned: "{{kind}} · scanned",
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
