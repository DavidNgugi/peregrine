//! The check every IPC payload has to pass: no path, whatever machine produced it.
//!
//! ARCHITECTURE §7 rule 3 forbids an absolute home path in an error's `detail` or `cause`.
//! More than one test asserts it — the runtime error mapping in [`crate::jobs`] and the
//! contract suite in [`crate::contract_tests`] — so the fixtures live here, where neither
//! caller can weaken the check for the other by editing its own copy.
//!
//! A path can be rendered in three shapes depending on the machine that produced it, so
//! checking only `/home/` and `.local` — the weaker check this replaced — passes on macOS and
//! Windows no matter what leaks, because neither string can appear in a path there. The canary
//! is the part that matters: it sits in every fixture path, so it appears in a payload if any
//! of them is interpolated, whatever the separator. The roots are a second net for a future
//! fixture that forgets the canary.

/// A directory name carried by every fixture path, so a leaked path is visible on any OS.
const PATH_CANARY: &str = "peregrine-leak-canary";

/// The roots a path can begin with on the platforms Peregrine ships on.
const PATH_ROOTS: [&str; 5] = ["/home/", "/Users/", ".local", "AppData", "\\Users\\"];

/// Panics when `text` carries the canary or a platform path root.
///
/// `what` names the payload in the failure message, so a report says which response leaked
/// rather than only that one did.
pub(crate) fn assert_no_path(what: &str, text: &str) {
    assert!(!text.contains(PATH_CANARY), "a path reached {what}: {text}");
    for root in PATH_ROOTS {
        assert!(
            !text.contains(root),
            "the path root `{root}` reached {what}: {text}"
        );
    }
}
