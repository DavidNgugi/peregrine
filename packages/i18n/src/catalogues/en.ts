/**
 * English messages.
 *
 * Copy rules are in docs/DESIGN_SYSTEM.md §1.2: sentence case, no exclamation
 * marks, and an error says what happened, why, and what to do next. The
 * `ipc.error.*` keys are the other half of `@peregrine/ipc-client`'s code mapping.
 */
export const en = {
  translation: {
    shell: {
      nav: {
        label: "Screens",
        collapse: "Hide the sidebar",
        expand: "Show the sidebar",
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
    downloads: {
      title: "Downloads",
      subtitle: "Transfers this build is running, and the queue behind them.",
      section: "Transfer queue",
      description: "Progress updates while a transfer is active.",
      none: "—",
      acquireStarted: "Finding the best release and queueing it. Track it in Downloads.",
      acquireFinding: "Finding a release… {{percent}}%",
      acquireQueued: "Added to Downloads.",
      acquireBlocked: "Peregrine refused this release, so nothing was downloaded.",
      acquireFailed: "No release could be queued.",
      view: "View downloads",
      empty: {
        title: "Nothing is downloading",
        body: "Open a title and choose Download; its transfers appear here.",
      },
      error: {
        title: "The downloads could not be read",
        body: "The core did not answer the transfer queue. Try again.",
      },
      progress: { label: "{{title}} download progress" },
      size: { none: "0 B", value: "{{value}} {{unit}}" },
      rate: { value: "{{value}}/s" },
      eta: { seconds: "{{value}} s", minutes: "{{value}} min", hours: "{{value}} h" },
      column: {
        title: "Title",
        state: "State",
        reason: "Reason",
        progress: "Progress",
        size: "Size",
        rate: "Rate",
        eta: "ETA",
        peers: "Peers",
        actions: "Actions",
      },
      action: { pause: "Pause", resume: "Resume", remove: "Remove" },
      remove: {
        title: "Remove this download?",
        body: "The transfer stops and the job leaves the queue. Files already written are not deleted.",
        confirm: "Remove",
        cancel: "Cancel",
      },
      state: {
        queued: "Queued",
        inspecting: "Inspecting",
        metadata: "Fetching metadata",
        downloading: "Downloading",
        paused: "Paused",
        seeding: "Seeding",
        completed: "Completed",
        failed: "Failed",
        blocked: "Blocked",
        removed: "Removed",
      },
      // One sentence per `Reason` spelling in peregrine-domain's safety report. A code with no
      // entry here falls back to the code itself rather than rendering blank, so a new reason is a
      // missing sentence and never a missing row.
      finding: {
        path_traversal: "The name tries to leave the download folder.",
        absolute_path: "The name is an absolute path, not a file inside the folder.",
        percent_encoded_path: "The name hides a path character behind percent-encoding.",
        reserved_device_name: "The name is a reserved device name on Windows.",
        forbidden_character: "The name contains a character this filesystem cannot store.",
        component_too_long: "A path component is longer than the filesystem allows.",
        malformed_path: "The entry is not a plain relative file name.",
        symlink_entry: "The torrent contains a symbolic link.",
        hardlink_entry: "The torrent contains a hard link.",
        executable_payload: "The torrent contains a program or a script.",
        unicode_spoofing: "The name uses invisible characters that change how it reads.",
        nested_archive: "The torrent contains an archive whose contents were not inspected.",
        disk_image: "The torrent contains a disk image, which mounts another filesystem.",
        extension_not_allowed: "The file's type is not one Peregrine downloads.",
        file_count_capped: "The torrent has more files than the limit allows.",
        total_size_capped: "The torrent is larger than the size limit allows.",
      },
    },
    discover: {
      title: "Discover",
      filter: { label: "Title type", all: "All", movies: "Movies", shows: "TV shows" },
      empty: {
        title: "Nothing here yet",
        body: "Peregrine has no catalogue data yet. Refresh to ask the providers — a provider key is set up in Settings.",
      },
      fetching: {
        body: "Fetching the rails from your provider. They appear as the answers arrive.",
      },
      refresh: {
        action: "Refresh",
        running: "Refreshing…",
        failed: "The refresh could not be started. Nothing was lost — try again.",
      },
      hero: {
        label: "Featured title",
        play: "Play",
        // Why the primary action cannot be taken, said where the control is. `noFile` is the
        // title's own fact and comes first; `noPlayer` is this build's, because no playback
        // surface exists yet — a live button that fired nothing would be worse than a word.
        noFile: "Not in your library yet",
        noPlayer: "Playback is not available yet",
        strip: "Featured titles",
        show: "Show {{name}}",
      },
      kind: {
        movie: "Film",
        show: "Series",
      },
      rail: {
        trendingWeek: "Trending this week",
        popularMovies: "Popular films",
        topRatedMovies: "Top rated films",
        upcomingMovies: "Coming soon",
        popularShows: "Popular series",
        onTheAirShows: "On the air",
        topRatedShows: "Top rated series",
        continueWatching: "Continue Watching",
        unknown: "More to watch",
        empty: {
          title: "Nothing in this rail yet",
          body: "Refresh to ask the providers, or add a provider key in Settings.",
        },
      },
      continue: {
        percent: "{{percent}}% watched",
      },
      search: {
        label: "Search for a title",
        placeholder: "A film or a series",
        action: "Search",
        searching: "Searching the providers…",
        // **Which of the two answered.** The catalogue's index is instant and offline; a provider
        // answer has just been written into the catalogue. The two are otherwise indistinguishable,
        // and a reader is entitled to know whether a request was spent.
        source: {
          catalogue: "In your catalogue",
          provider: "From your providers",
        },
      },
      searchFailed: {
        title: "The search could not be made",
        body: "Nothing was lost — the library is unchanged. Check the provider's API key in Settings, then try again.",
      },
      failed: {
        title: "The rails could not be read",
        body: "Nothing was lost. Try again, or open Diagnostics to see what the core reported.",
      },
      none: {
        title: "Nothing matched",
        body: "The providers answered, and the catalogue holds nothing by that name. Try a different spelling, or the original title.",
      },
      result: {
        movie: "Film",
        show: "Series",
      },
    },
    // The queried grid, which is the screen a rail cannot be: a rail is a list the provider
    // publishes, and this is a list the reader composes out of filters.
    browse: {
      title: "Browse",
      kind: { label: "Title type" },
      genre: {
        label: "Genre",
        // The word for the unfiltered choice, and it is a choice rather than an absence: the filter
        // is applied by the provider, so "All genres" is a request that asks for every one of them.
        all: "All genres",
      },
      sort: {
        label: "Sort by",
        popularity: "Most popular",
        rating: "Best rated",
        newest: "Newest",
        oldest: "Oldest",
      },
      rating: {
        label: "Minimum rating",
        any: "Any rating",
        // Out of ten, which is the scale the badge on a tile shows. `{{score}}` is a whole number,
        // so no plural form is needed in any locale that counts stars this way.
        floor: "{{score}}+ stars",
      },
      grid: { label: "Browse results" },
      loading: "Loading more…",
      // The other half of `loading`: a reader who has reached the end of the loaded pages has to be
      // able to tell "nothing more is coming" from "the next page is still on its way".
      end: "That is everything for these filters.",
      empty: {
        title: "Nothing matches these filters",
        body: "The provider has no title that fits. Try another genre, a lower rating, or a different order.",
      },
      failed: {
        title: "This page could not be loaded",
        body: "The provider did not answer. Check your connection and the API key in Settings, then scroll again.",
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
      cancelConfirm: {
        title: "Cancel the running job?",
        body: "The job stops at its next step. Work it has already finished is kept; it will not be resumed.",
        confirm: "Cancel the job",
        cancel: "Keep it running",
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
    // The poster tile's own words, which the kit deliberately does not hold (`AGENTS R7`): a chip's
    // state, a rating's spoken form, a vote count, and one verb. They live here rather than under
    // `discover` because every screen that draws a tile reads them.
    card: {
      details: "View More",
      rating: "rated {{score}} out of 10",
      // **Both plural forms, because a count of one is not a count of many.** `TitleCard` passes
      // `count` alongside a `formatted` string it has already grouped by locale, so the number reads
      // the same as every other count in the tile; i18next picks `_one` or `_other` from `count`. One
      // `votes` key cannot do this in any language whose plural rules are not English's, which is why
      // the pair is the shape rather than a single string with a number in it.
      votes_one: "{{formatted}} vote",
      votes_other: "{{formatted}} votes",
    },
    player: {
      title: "Player",
      playing: "Playing from your library",
      back: "Back to the title",
      starting: "Opening the file…",
      // Reached when the core refused, which is the block above it; this is the body under the error.
      nothing: {
        title: "Nothing is playing.",
        body: "The file could not be opened. Its title page says whether the library still holds a copy.",
      },
      refused: "That file could not be played.",
      // The sentence beside the control shown when the webview refused to start playing on its own — which
      // is a gesture the route change spent, not a fault in the file.
      start: "Play",
      // The element's own fallback text, for a platform whose media stack will not take the stream.
      unsupported: "This build cannot play that file's format.",
      // What the session's container means, said beside the element rather than as a refusal: it explains
      // which reader is doing the work and what that costs.
      support: {
        webview: "Playing with this system's own video decoder.",
        externalPlayer: "Playing with an external player.",
        unknown: "Peregrine does not recognise this file's format. Playing it may not work.",
      },
    },
    title: {
      section: "In your library",
      column: {
        items: "Items",
        episode: "Episode",
        name: "Name",
      },
      unnamedEpisode: "Not named yet",
      poster: {
        alt: "Poster for {{name}}",
        none: "No poster yet",
      },
      episodes: {
        title: "Episodes",
        description: "The season you have chosen, in the order the episodes are watched.",
        failed: "That season's episodes could not be read. Choosing it again tries once more.",
        none: {
          title: "Nothing listed for this season.",
          body: "No provider has described this season's episodes yet, and the library holds none of them.",
        },
      },
      // The hero: the wide picture, the name, the facts, and the actions the core says it can honour.
      // The reasons a control is disabled are `lib/play-action.ts`'s, one sentence per value of the
      // offer the core derives, and each names the thing a reader can act on.
      hero: {
        released: "{{date}}",
        seasons: "{{count}} season",
        seasons_other: "{{count}} seasons",
        episodes: "{{count}} episode",
        episodes_other: "{{count}} episodes",
        play: "Play",
        download: "Download",
        finding: "Finding a release…",
        noFile: "The library holds no file for this title yet.",
        fileMissing: "The library recorded a file for this title, and it is not on disk right now.",
        arriving: "A download for this title is in flight.",
        noPlayer: "This build has no player yet, so there is nothing to play it with.",
        // The container, said before the press rather than after the black rectangle. It names what is
        // missing — an external player — rather than what failed, because nothing has failed yet and a
        // reader can go and get one.
        container:
          "This build cannot read that file's format yet. It needs an external player, which is not installed.",
        // An admission rather than a refusal: the format is not one Peregrine can name, and the
        // system's own decoder may well take it. Saying nothing would make a failure look like a bug.
        unnamedContainer:
          "Peregrine does not recognise this file's format. Playing it may not work.",
        acquire: "Peregrine will find the best release for this title and add it to your downloads.",
      },
      // The switcher. `{{number}}` is the only thing a chip needs for a season a provider named the
      // way it names every season — "Season 4" — and the provider's own name is used only when it says
      // something the number does not.
      season: {
        label: "Seasons",
        number: "Season {{number}}",
        specials: "Specials",
      },
      // One episode's card. Every field but the number is optional, because an episode the library
      // holds and no provider has described is the ordinary state of a fresh scan.
      episode: {
        still: {
          none: "No still yet",
          alt: "Still for episode {{number}}, {{name}}",
        },
        aired: "Aired {{date}}",
        runtime: "{{minutes}} min",
        // The word and not the dot: a card's state is carried by its word (§2.2), and the dot's colour
        // mapping belongs to `TitleCard`, which owns the one copy of it.
        availability: {
          available: "In your library",
          partial: "Partly here",
          missing: "Files are gone",
          wanted: "Not acquired",
        },
      },
      missing: {
        title: "This title is not here.",
        body: "Nothing in your library has this id. It may have been a link to something this installation does not have.",
      },
      unavailable: {
        title: "This title could not be read.",
        body: "Peregrine's core did not answer, so this page is unknown rather than empty. Going back and returning tries the read again.",
      },
      // The provider's recommendations for the title on screen. One string, because the rail draws
      // the heading and there is no second surface for a description — a `Section` wrapped around it
      // printed these words twice.
      similar: {
        title: "More like this",
      },
      // The endless feed under the episodes: the recommendations, then the catalogue's own pages of the
      // genres they share. One heading, because it is one list rather than a rail plus a shelf per genre.
      feed: {
        title: "More like this",
        description: "Recommended titles, then everything else in the genres they share.",
        end: "That is everything your providers have for this title.",
        failed: {
          title: "More titles could not be loaded",
          body: "What arrived is above. Scroll again to try for the rest.",
        },
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
      picker: {
        title: "File this under…",
        description: "Choose the title “{{file}}” belongs to. Nothing is downloaded or moved.",
        search: "Search your catalogue",
        placeholder: "A film or series name",
        empty: {
          title: "Nothing to choose from yet.",
          body: "Type a name to search the catalogue.",
        },
        failed: {
          title: "The search did not run.",
          body: "Peregrine's core did not answer, so this list is unknown rather than empty.",
        },
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
        choose: "Choose…",
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
      // The list/grid choice. The label names the group rather than either drawing; the two buttons
      // carry their own words.
      view: {
        label: "How to show your library",
        list: "List",
        grid: "Grid",
      },
      // The grid's own accessible name, and it says what the region *holds* rather than calling them
      // results: nothing here was searched for, these are the user's own files.
      grid: { label: "Titles in your library" },
      // **Posters this machine makes from the films themselves.** A freshly scanned folder is titles no
      // provider has described, and the screen captures a frame of each so the grid is not a wall of grey.
      // Both plural forms for the reason `card.votes` gives; the number is the count already stored.
      posters: {
        working_one: "Making a poster from the film… {{count}} done",
        working_other: "Making posters from the films… {{count}} done",
        done_one: "{{count}} poster made from the film.",
        done_other: "{{count}} posters made from the films.",
      },
      tile: {
        // The table's Missing column in words, drawn in the tile's revealed panel. Both plural forms
        // for the reason `card.votes` gives: a count of one is not a count of many, and i18next picks
        // the form from `count`.
        missing_one: "{{count}} copy missing",
        missing_other: "{{count}} copies missing",
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
        removeConfirm: {
          title: "Remove this folder?",
          body: "Peregrine stops scanning {{path}}. Nothing is deleted and what you have watched stays.",
          confirm: "Remove folder",
          cancel: "Keep folder",
        },
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
      indexers: {
        title: "Indexers",
        description:
          "Where Peregrine looks for releases. Prowlarr is asked over its own API, and the API key is stored in your operating system's keychain, never in Peregrine's database.",
        baseUrl: {
          label: "Prowlarr URL",
          accessible: "Prowlarr base URL",
          placeholder: "http://localhost:9696",
          description: "The address Prowlarr answers on, including the scheme and port.",
        },
        key: {
          label: "API key",
          accessible: "Prowlarr API key",
          placeholder: "Paste the key",
          description:
            "From Prowlarr under Settings → General → API Key. Stored in your keychain and never shown again.",
        },
        status: {
          label: "Status",
        },
        configured: "Configured at {{url}}",
        notConfigured: "No indexer is configured yet.",
        save: "Save",
        test: "Test connection",
        clear: "Remove",
        saved: "Saved. Peregrine will search this indexer for releases.",
        removed: "Removed. Downloads already queued are not affected.",
        tested: "Prowlarr answered. The URL and key work.",
        testFailed: "Prowlarr did not answer with that URL and key.",
        list: {
          title: "Indexers in Prowlarr",
          empty: "Prowlarr has no indexers yet. Add one below.",
        },
        remove: "Remove",
        removeConfirm: {
          title: "Remove this indexer?",
          body: "{{name}} will be removed from Prowlarr. Downloads already queued are not affected.",
        },
        deleted: "Removed from Prowlarr.",
        add: {
          title: "Add an indexer",
          browse: "Browse indexers",
          browsing: "Loading indexers…",
          search: "Filter",
          searchPlaceholder: "Type to filter…",
          choose: "Choose an indexer",
          name: "Name",
          submit: "Add indexer",
          added: "Added to Prowlarr.",
          none: "Prowlarr offered no indexer definitions.",
          noMatches: "No indexer matches that filter.",
          change: "Change indexer",
        },
        confirm: {
          title: "Remove the indexer?",
          body: "Peregrine will stop searching Prowlarr for releases. Downloads already queued are not affected, and nothing else is removed.",
        },
        unavailable: {
          title: "Your indexer settings could not be read.",
          body: "Peregrine's core did not answer, so this is unknown rather than empty.",
        },
      },
      prowlarr: {
        title: "Managed Prowlarr",
        description:
          "Let Peregrine install and run Prowlarr for you. It downloads the official release, checks it against a pinned checksum, and runs it on this machine only; the API key it generates is stored in your keychain.",
        state: "State",
        version: "Version",
        notInstalled: "Not installed",
        installed: "Installed, not running",
        runningAt: "Running at {{url}}",
        install: "Install Prowlarr",
        installing: "Installing…",
        start: "Start",
        starting: "Starting…",
        stop: "Stop",
        stopConfirm: {
          title: "Stop Prowlarr?",
          body: "Managed Prowlarr will stop serving this machine immediately. Downloads already running continue; searching for new releases stops until you start it again.",
          confirm: "Stop Prowlarr",
          cancel: "Keep running",
        },
        outcome: {
          installed: "Prowlarr is installed.",
          started: "Prowlarr is running and searching releases now works.",
          stopped: "Prowlarr is stopped.",
        },
        unsupported:
          "This platform has no pinned Prowlarr build, so point Peregrine at a remote instance below instead.",
        customHint:
          "Use the fields below instead to point at your own Prowlarr — a remote box, a homelab server, or an existing instance.",
        unavailable: {
          title: "The managed Prowlarr could not be read.",
          body: "Peregrine's core did not answer, so this is unknown rather than not installed.",
        },
      },
      providers: {
        title: "Metadata providers",
        description:
          "The services Peregrine asks about films and series. A key is stored in your operating system's keychain, never in Peregrine's database, so reinstalling Peregrine does not remove it. You can remove it again here.",
        entry: "{{endpoint}} · {{state}}",
        configured: "Configured",
        notConfigured: "No key stored",
        // The three states of a credential store that is not simply answering. They exist because a
        // keychain that refused access leaves every provider looking unconfigured, and telling a
        // user their key is missing when it is still in the keychain is the sentence that makes
        // reinstalling look like it deleted something.
        store: {
          unread: "Checking the keychain…",
          denied: "Keychain access refused",
          unavailable: "No keychain available",
        },
        storeNotice: {
          unread:
            "Peregrine has not finished reading your keychain. The first read from a newly built copy of the app can raise macOS's permission dialog — look for it behind this window or in the Dock and choose Allow, then reopen Peregrine. The dialog is macOS asking whether this build may read the “Peregrine” entry; declining it does not delete the key.",
          denied:
            "macOS asked whether Peregrine may read your keychain and the answer was no, so Peregrine cannot tell whether a key is stored. Nothing has been deleted: your key is still in Keychain Access under “Peregrine”. Allow the request when the system asks, or open Keychain Access, find the “Peregrine” item, and set its access control to allow Peregrine — then reopen the app.",
          unavailable:
            "This system has no keychain Peregrine can use, so a key cannot be stored or read here. Keys you entered on another machine or OS login are not lost; they are simply not available in this session.",
        },
        retry: "Try again",
        checking: "Asking the provider whether that key works…",
        remove: "Remove",
        unavailable: {
          title: "Your providers could not be read.",
          body: "Peregrine's core did not answer, so this list is unknown rather than empty. Reopening the app tries the read again.",
        },
        empty: {
          title: "No metadata provider is built in.",
          body: "This build knows no provider to configure. Peregrine can still scan and play what is already in your library.",
        },
        key: {
          label: "API key",
          accessible: "{{provider}} API key",
          placeholder: "Paste the key",
          description:
            "Paste the key for {{provider}}. Saving proves it against the provider first, so a key that does not work is never stored.",
          save: "Save and test",
          test: "Test connection",
        },
        confirm: {
          title: "Remove the {{provider}} key?",
          body: "Peregrine forgets the key and stops asking that provider. Titles already in your library stay where they are.",
        },
        outcome: {
          saved: "That key works. Peregrine stored it for {{provider}}.",
          working: "{{provider}} accepted the stored key.",
          removed: "The key was removed. {{provider}} is no longer asked.",
          unauthorized: "That key was refused. Check you copied the API key and not another value.",
          rateLimited: "{{provider}} is rate limiting this app; try again in a minute.",
          unreachable: "{{provider}} could not be reached.",
          store: "Peregrine could not read its credential store, so the key was not saved.",
          notConfigured:
            "No key is stored for that provider, so there was nothing to test. Paste one and save it.",
          cancelled: "That was cancelled. Nothing was changed.",
          failed:
            "The key could not be confirmed. Try again, and check the provider's own status if it keeps happening.",
        },
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
