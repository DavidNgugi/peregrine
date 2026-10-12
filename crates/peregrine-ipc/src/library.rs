//! The `library` namespace of the command surface (docs/ARCHITECTURE.md §7).
//!
//! Three commands, and the shape they establish is the one every later screen reuses: a read that
//! pages, a write that takes the thing the user chose, and a removal that says what it did not
//! touch. The DTOs are this module's rather than the domain's, because `peregrine-domain` cannot
//! depend on `specta` (AGENTS R6) and the wire shape is a contract of its own — a domain field
//! renamed should not silently rename a field the webview reads.

use peregrine_app::{LibraryRootKind, ScanMode, Timestamp};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::runtime::RuntimeState;

/// What a root is expected to hold, as the webview spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RootKind {
    /// Films.
    Movies,
    /// Series.
    Shows,
    /// Both, or neither — a folder the user has not organised.
    Mixed,
    /// A download directory, which is watched rather than browsed.
    Downloads,
}

impl From<LibraryRootKind> for RootKind {
    fn from(kind: LibraryRootKind) -> Self {
        match kind {
            LibraryRootKind::Movies => Self::Movies,
            LibraryRootKind::Shows => Self::Shows,
            LibraryRootKind::Mixed => Self::Mixed,
            LibraryRootKind::Downloads => Self::Downloads,
        }
    }
}

impl From<RootKind> for LibraryRootKind {
    fn from(kind: RootKind) -> Self {
        match kind {
            RootKind::Movies => Self::Movies,
            RootKind::Shows => Self::Shows,
            RootKind::Mixed => Self::Mixed,
            RootKind::Downloads => Self::Downloads,
        }
    }
}

/// When a root is scanned, as the webview spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RootScanMode {
    /// A watcher keeps it current.
    Incremental,
    /// On a schedule the settings own.
    Scheduled,
    /// Only when the user asks.
    Manual,
}

impl From<ScanMode> for RootScanMode {
    fn from(mode: ScanMode) -> Self {
        match mode {
            ScanMode::Incremental => Self::Incremental,
            ScanMode::Scheduled => Self::Scheduled,
            ScanMode::Manual => Self::Manual,
        }
    }
}

impl From<RootScanMode> for ScanMode {
    fn from(mode: RootScanMode) -> Self {
        match mode {
            RootScanMode::Incremental => Self::Incremental,
            RootScanMode::Scheduled => Self::Scheduled,
            RootScanMode::Manual => Self::Manual,
        }
    }
}

/// One title the library holds, as the library screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTitleDto {
    /// The row id, which is the paging cursor.
    #[specta(type = Number)]
    pub id: i64,
    /// The display name.
    pub name: String,
    /// Film or series, as the webview spells it.
    pub kind: RootKind,
    /// How complete the metadata is: a title the library holds is a stub until a sync fills it in,
    /// and a screen that showed a stub as complete would promise artwork and a synopsis it does not
    /// have (`PG-E5-06`).
    pub stub: bool,
    /// How many items of it the library holds.
    #[specta(type = Number)]
    pub items: i64,
    /// How many files those items have.
    #[specta(type = Number)]
    pub files: i64,
    /// What those files add up to, in bytes.
    #[specta(type = Number)]
    pub bytes: i64,
    /// How many of them are marked missing.
    #[specta(type = Number)]
    pub missing_files: i64,
}

impl From<peregrine_app::LibraryTitleRow> for LibraryTitleDto {
    fn from(row: peregrine_app::LibraryTitleRow) -> Self {
        Self {
            id: row.title.id,
            name: row.title.name,
            kind: match row.title.kind {
                peregrine_app::TitleKind::Movie => RootKind::Movies,
                peregrine_app::TitleKind::Show => RootKind::Shows,
            },
            stub: row.title.metadata_state != peregrine_app::MetadataState::Complete,
            items: row.items,
            files: row.files,
            bytes: row.bytes,
            missing_files: row.missing_files,
        }
    }
}

/// The root a command acts on.
///
/// A DTO rather than a bare `i64` argument, because specta refuses to export a large integer as a
/// command argument — a bare cursor or id would be the one thing in the contract with no type on
/// the other side. The same reason `jobs::list` shows one page and takes no cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RootRef {
    /// The row id, as the list returned it.
    #[specta(type = Number)]
    pub id: i64,
}

/// One library root as the settings screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRootDto {
    /// The row id, which is the paging cursor and what a removal names.
    ///
    /// `Number` opts this `i64` into a TypeScript `number`, which specta otherwise refuses rather
    /// than silently losing precision. A row id and a Unix-millisecond instant both stay well
    /// inside the 2^53 a JavaScript number represents exactly.
    #[specta(type = Number)]
    pub id: i64,
    /// The absolute, canonical directory.
    pub path: String,
    /// What it is expected to hold.
    pub kind: RootKind,
    /// When it is scanned.
    pub scan_mode: RootScanMode,
    /// When it was last scanned, in Unix milliseconds, or nothing when it never was.
    #[specta(type = Option<Number>)]
    pub last_scan_at: Option<i64>,
    /// When it was added, in Unix milliseconds.
    #[specta(type = Number)]
    pub created_at: i64,
}

/// One page of library roots, newest first.
#[tauri::command]
#[specta::specta]
pub async fn list_roots(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<LibraryRootDto>, AppError> {
    let roots = state
        .runtime()?
        .library_roots(None, limit.unwrap_or(50))
        .await?;
    Ok(guard_response(
        "library::list_roots",
        roots
            .into_iter()
            .map(|row| LibraryRootDto {
                id: row.root.id,
                path: row.root.path.as_str().to_owned(),
                kind: row.root.kind.into(),
                scan_mode: row.root.scan_mode.into(),
                last_scan_at: row.root.last_scan_at.map(Timestamp::as_millisecond),
                created_at: row.root.created_at.as_millisecond(),
            })
            .collect(),
    ))
}

/// Adds a folder as a library root and answers with the row it wrote.
///
/// The path is text the shell's picker handed over without validating it (PG-E1-12), so the only
/// thing this command does with it is pass it to the service that decides. A folder that is
/// already a root comes back as an error naming the row that holds it, which is what lets the
/// screen point at the existing entry instead of only refusing.
#[tauri::command]
#[specta::specta]
pub async fn add_root(
    state: State<'_, RuntimeState>,
    path: String,
    kind: RootKind,
    scan_mode: RootScanMode,
) -> Result<LibraryRootDto, AppError> {
    let added = state
        .runtime()?
        .add_library_root(&path, kind.into(), scan_mode.into())
        .await?;
    Ok(guard_response(
        "library::add_root",
        LibraryRootDto {
            id: added.id,
            path: added.path.as_str().to_owned(),
            kind: added.kind.into(),
            scan_mode: added.scan_mode.into(),
            last_scan_at: added.last_scan_at.map(Timestamp::as_millisecond),
            created_at: added.created_at.as_millisecond(),
        },
    ))
}

/// A page of the titles the library holds, newest first.
///
/// The first read of the library's own content rather than of its configuration, and the one the
/// library screen is built on. A title the library holds is a *stub* until a provider sync fills it
/// in, which the DTO says plainly: a screen can then show what it has without implying it has more.
#[tauri::command]
#[specta::specta]
pub async fn list_titles(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<LibraryTitleDto>, AppError> {
    let titles = state
        .runtime()?
        .library_titles(None, limit.unwrap_or(100))
        .await?;
    let titles = titles.into_iter().map(LibraryTitleDto::from).collect();
    Ok(guard_response("library::list_titles", titles))
}

/// Starts a scan of every library root.
///
/// Answers with the job a caller watches, which is the same shape `jobs::start_hello` answers with,
/// because it is the same thing: a journal row and the scheduler's identity for the work. A scan
/// asked for while one is running is refused with `Conflict` rather than queued behind it — the
/// walk is the expensive part and a queue of identical walks is not a feature.
#[tauri::command]
#[specta::specta]
pub async fn scan_now(state: State<'_, RuntimeState>) -> Result<JobStarted, AppError> {
    let started = state.runtime()?.start_library_scan().await?;
    Ok(guard_response(
        "library::scan_now",
        JobStarted {
            job_id: started.job.to_string(),
            journal_id: started.journal_id,
        },
    ))
}

/// Removes a library root, reporting whether it was there.
///
/// This is the command whose name is the most dangerous thing about it, so the answer is explicit:
/// removing a root stops it being scanned and removes nothing else. The files stay where they are,
/// the items and their watch progress stay in the library, and a file that comes back is the same
/// item rather than a new one (PG-E14-01, PG-E14-13).
#[tauri::command]
#[specta::specta]
pub async fn remove_root(state: State<'_, RuntimeState>, root: RootRef) -> Result<bool, AppError> {
    let removed = state.runtime()?.remove_library_root(root.id).await?;
    Ok(guard_response("library::remove_root", removed))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use peregrine_app::events::{Event, EventSink, EventSinkError};
    use peregrine_app::{Observability, Runtime, RuntimeConfig};
    use tauri::Manager as _;
    use tempfile::TempDir;

    use super::*;

    /// A sink that discards every event: these tests are about the command surface, and the bus's
    /// own delivery is tested in the crate that owns it.
    struct SilentSink;

    impl EventSink for SilentSink {
        fn emit(&self, _event: &Event) -> Result<(), EventSinkError> {
            Ok(())
        }
    }

    /// Starts a runtime into a mock app and returns the app and a folder that can be added, so a
    /// command can be called the way Tauri calls it: with managed state injected.
    async fn served(directory: &TempDir) -> (tauri::App<tauri::test::MockRuntime>, String) {
        let app = tauri::test::mock_app();
        let config = RuntimeConfig::new(directory.path(), Arc::new(SilentSink));
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        app.manage(RuntimeState::for_test(Arc::new(runtime)));

        let folder = directory.path().join("Movies");
        fs::create_dir_all(&folder).unwrap();
        (app, folder.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn a_root_can_be_added_listed_and_removed() {
        let directory = TempDir::new().unwrap();
        let (app, folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();

        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 0);

        let added = add_root(
            state.clone(),
            folder.clone(),
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .unwrap();
        assert_eq!(added.path, folder);
        assert_eq!(added.kind, RootKind::Movies);
        assert_eq!(added.scan_mode, RootScanMode::Incremental);
        assert_eq!(
            added.last_scan_at, None,
            "a root that was never scanned says so"
        );
        assert!(added.created_at > 0);

        let listed = list_roots(state.clone(), None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], added);

        assert!(
            remove_root(state.clone(), RootRef { id: added.id })
                .await
                .unwrap()
        );
        assert!(
            !remove_root(state.clone(), RootRef { id: added.id })
                .await
                .unwrap(),
            "removing it twice reports that the second one did nothing"
        );
        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 0);

        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn a_folder_that_is_already_a_root_is_refused_with_a_reason() {
        let directory = TempDir::new().unwrap();
        let (app, folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();
        add_root(
            state.clone(),
            folder.clone(),
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .unwrap();

        let refused = add_root(state.clone(), folder, RootKind::Shows, RootScanMode::Manual)
            .await
            .expect_err("the same folder twice is refused");

        assert_eq!(
            refused.code,
            crate::error::ErrorCode::Conflict,
            "a folder that is already a root is a conflict, not a bad request: {refused:?}"
        );
        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 1);

        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn a_path_that_cannot_be_a_root_is_refused_without_writing_anything() {
        // The picker hands over text it has not validated, so a relative path has to be refused
        // here rather than half-written and discovered later by the scanner.
        let directory = TempDir::new().unwrap();
        let (app, _folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let refused = add_root(
            state.clone(),
            "Movies".to_owned(),
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .expect_err("a relative path cannot be a root");

        assert_eq!(
            refused.code,
            crate::error::ErrorCode::InvalidInput,
            "{refused:?}"
        );
        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 0);

        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn removing_a_root_leaves_the_folder_and_its_contents_alone() {
        // The command's answer is a bare `bool`, so the claim its documentation makes — "this
        // removes a configuration entry and nothing else" — is asserted against a filesystem.
        let directory = TempDir::new().unwrap();
        let (app, folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();
        let film = directory.path().join("Movies/Alien (1979).mkv");
        fs::write(&film, b"a film").unwrap();
        let added = add_root(
            state.clone(),
            folder,
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .unwrap();

        remove_root(state.clone(), RootRef { id: added.id })
            .await
            .unwrap();

        assert!(film.is_file(), "the user's files are theirs");
        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn a_command_with_no_running_runtime_reports_that_rather_than_panicking() {
        // Every command shares this path: `RuntimeState::failed` is the state startup leaves behind
        // (PG-E2-02), and a command that unwrapped here would take the window with it.
        let app = tauri::test::mock_app();
        app.manage(RuntimeState::failed("the database could not be opened"));
        let state = app.state::<RuntimeState>();

        let refused = list_roots(state, None)
            .await
            .expect_err("no runtime, no roots");

        assert!(
            !refused.message.is_empty(),
            "and the failure says something a user can read: {refused:?}"
        );
    }

    #[tokio::test]
    async fn roots_are_listed_newest_first() {
        // Two roots, listed newest first: the order the screen shows them in, and the order a
        // cursor would walk when the list is long enough to need one.
        let directory = TempDir::new().unwrap();
        let (app, _folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();
        for name in ["Movies", "Shows"] {
            let folder = directory.path().join(name);
            fs::create_dir_all(&folder).unwrap();
            add_root(
                state.clone(),
                folder.to_string_lossy().into_owned(),
                RootKind::Movies,
                RootScanMode::Incremental,
            )
            .await
            .unwrap();
        }

        let listed = list_roots(state.clone(), Some(10)).await.unwrap();

        assert_eq!(listed.len(), 2);
        assert!(
            listed[0].id > listed[1].id,
            "newest first, which is the order the screen shows them in"
        );

        crate::runtime::shutdown(app.handle()).await;
    }
}
