//! Fixture shell scripts that stand in for XMR effect children read their
//! inherited descriptors as `/proc/self/fd/N` and hash them with
//! `sha256sum`. On Linux they run as written.
//!
//! On macOS there is no `/proc`, and `/dev/fd/N` duplicates the descriptor
//! instead of reopening the file: the offset is shared, so a second `grep` of
//! the same descriptor reads nothing. The script then first copies every
//! descriptor it inherited (a regular file's bytes, an empty marker for
//! anything else) into a private temporary directory, once, and reads the
//! copies; `test -e` on a descriptor becomes a test of its copy, so an absent
//! descriptor stays absent. `sha256sum` is `shasum -a 256`, same output.

#![allow(dead_code)]

#[cfg(target_os = "macos")]
const MACOS_PRELUDE: &str = "LEZ_FD=\"$(mktemp -d)\"\n\
trap 'rm -rf \"$LEZ_FD\"' EXIT\n\
for lez_fd in $(ls /dev/fd); do\n\
  if [ -f \"/dev/fd/$lez_fd\" ]; then cat \"/dev/fd/$lez_fd\" >\"$LEZ_FD/$lez_fd\" 2>/dev/null || : >\"$LEZ_FD/$lez_fd\";\n\
  elif [ -e \"/dev/fd/$lez_fd\" ]; then : >\"$LEZ_FD/$lez_fd\"; fi\n\
done\n";

/// The script as this platform runs it.
#[must_use]
pub fn fd_script(script: &[u8]) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    {
        script.to_vec()
    }
    #[cfg(target_os = "macos")]
    {
        let text = String::from_utf8(script.to_vec()).expect("UTF-8 fixture script");
        let (shebang, body) = text
            .split_once('\n')
            .expect("fixture script has a shebang line");
        let body = body
            .replace("/proc/self/fd/", "$LEZ_FD/")
            .replace("sha256sum", "shasum -a 256");
        format!("{shebang}\n{MACOS_PRELUDE}{body}").into_bytes()
    }
}

/// [`fd_script`] for a script built as a string.
#[must_use]
pub fn fd_script_text(script: &str) -> String {
    String::from_utf8(fd_script(script.as_bytes())).expect("UTF-8 fixture script")
}
