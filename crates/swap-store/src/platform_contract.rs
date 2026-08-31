//! Compile-time contract for Linux kernel-backed authority boundaries.
//!
//! Actor configuration sealing uses `memfd_create` plus seals and private file
//! creation uses `openat2` resolution constraints. Portable fallbacks would
//! silently weaken those invariants, so the runtime package is Linux-only.
