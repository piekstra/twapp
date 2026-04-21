//! Process-wide test env lock, shared by every test module that mutates
//! environment variables like `TWAPP_MAILBOX_DIR` / `TWAPP_SHARED_DIR`.
//!
//! Each module used to define its own `OnceLock<Mutex<()>>` inside `tests`,
//! but those were independent mutexes — so tests in module A could run
//! concurrently with tests in module B even though both were mutating the
//! same global process env, producing flakes where one test read the env
//! var that another test had just clobbered. A single shared lock fixes
//! that without giving up parallelism between tests that don't touch env.
#![cfg(test)]

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Acquire the shared test env mutex. Poisoned locks are recovered — a
/// panicking test must not permanently brick the rest of the suite.
pub fn lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}
