//! The devnet's test hooks: files in the directory `LEZ_TEST_HOOKS_DIR`
//! names, read on every use. Built only with the `test-hooks` feature, which
//! the devnet's build enables and no release build does.

use std::path::PathBuf;

// A release build (the release workflows and packagers set LEZ_RELEASE_BUILD)
// cannot carry the test hooks: it does not compile.
const _: () = assert!(
    option_env!("LEZ_RELEASE_BUILD").is_none(),
    "the devnet test hooks (feature test-hooks) are never part of a release build (LEZ_RELEASE_BUILD)"
);

fn file(name: &str) -> Option<PathBuf> {
    std::env::var_os("LEZ_TEST_HOOKS_DIR").map(|directory| PathBuf::from(directory).join(name))
}

/// Takes one from the count the file `name` holds: true, and the count less
/// one written back, when it held a count above zero.
pub(crate) fn take(name: &str) -> bool {
    let Some(path) = file(name) else {
        return false;
    };
    let Some(count) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .filter(|count| *count > 0)
    else {
        return false;
    };
    std::fs::write(&path, format!("{}\n", count - 1)).is_ok()
}
