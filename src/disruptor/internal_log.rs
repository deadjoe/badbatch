//! Compatibility wrappers for BadBatch's former private environment-variable logger.
//!
//! New code should use the [`log`] facade directly. These exported macros are
//! retained so downstream code that happened to use them keeps compiling; they
//! no longer read `BADBATCH_LOG` or write directly to stderr.

use std::fmt;

#[doc(hidden)]
pub fn debug(args: fmt::Arguments<'_>) {
    log::debug!(target: "badbatch", "{args}");
}

#[doc(hidden)]
pub fn info(args: fmt::Arguments<'_>) {
    log::info!(target: "badbatch", "{args}");
}

#[doc(hidden)]
pub fn warn(args: fmt::Arguments<'_>) {
    log::warn!(target: "badbatch", "{args}");
}

#[doc(hidden)]
pub fn error(args: fmt::Arguments<'_>) {
    log::error!(target: "badbatch", "{args}");
}

/// Compatibility macro; use the `log` facade in new code.
#[macro_export]
macro_rules! internal_debug {
    ($($arg:tt)*) => {
        $crate::disruptor::internal_log::debug(format_args!($($arg)*))
    };
}

/// Compatibility macro; use the `log` facade in new code.
#[macro_export]
macro_rules! internal_info {
    ($($arg:tt)*) => {
        $crate::disruptor::internal_log::info(format_args!($($arg)*))
    };
}

/// Compatibility macro; use the `log` facade in new code.
#[macro_export]
macro_rules! internal_warn {
    ($($arg:tt)*) => {
        $crate::disruptor::internal_log::warn(format_args!($($arg)*))
    };
}

/// Compatibility macro; use the `log` facade in new code.
#[macro_export]
macro_rules! internal_error {
    ($($arg:tt)*) => {
        $crate::disruptor::internal_log::error(format_args!($($arg)*))
    };
}

/// Test logging macro retained for the existing opt-in test diagnostics.
#[macro_export]
macro_rules! test_log {
    ($($arg:tt)*) => {
        #[cfg(test)]
        {
            if std::env::var("BADBATCH_TEST_LOG")
                .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
            {
                ::std::println!("[BADBATCH TEST] {}", format_args!($($arg)*));
            }
        }
    };
}
