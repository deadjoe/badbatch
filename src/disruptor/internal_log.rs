//! Internal logging utilities for BadBatch Disruptor
//!
//! This module provides controlled logging that doesn't pollute user output
//! by default. Logging can be enabled via the BADBATCH_LOG environment variable.

/// Internal debug logging macro
/// Only outputs if BADBATCH_LOG environment variable is set to "debug" or "trace"
#[macro_export]
macro_rules! internal_debug {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        {
            if std::env::var("BADBATCH_LOG").map(|s| s == "debug" || s == "trace").unwrap_or(false) {
                eprintln!("[BADBATCH DEBUG] {}", format!($($arg)*));
            }
        }
    };
}

/// Internal info logging macro  
/// Only outputs if BADBATCH_LOG environment variable is set to "info", "debug", or "trace"
#[macro_export]
macro_rules! internal_info {
    ($($arg:tt)*) => {
        if std::env::var("BADBATCH_LOG").map(|s| matches!(s.as_str(), "info" | "debug" | "trace")).unwrap_or(false) {
            eprintln!("[BADBATCH INFO] {}", format!($($arg)*));
        }
    };
}

/// Internal warn logging macro
/// Only outputs if BADBATCH_LOG environment variable is set to "warn", "info", "debug", or "trace"
#[macro_export]
macro_rules! internal_warn {
    ($($arg:tt)*) => {
        if std::env::var("BADBATCH_LOG").map(|s| matches!(s.as_str(), "warn" | "info" | "debug" | "trace")).unwrap_or(false) {
            eprintln!("[BADBATCH WARN] {}", format!($($arg)*));
        }
    };
}

/// Internal error logging macro
/// Always outputs to stderr in debug builds, controlled by BADBATCH_LOG in release
#[macro_export]
macro_rules! internal_error {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        {
            eprintln!("[BADBATCH ERROR] {}", format!($($arg)*));
        }
        #[cfg(not(debug_assertions))]
        {
            if std::env::var("BADBATCH_LOG").map(|s| matches!(s.as_str(), "error" | "warn" | "info" | "debug" | "trace")).unwrap_or(false) {
                eprintln!("[BADBATCH ERROR] {}", format!($($arg)*));
            }
        }
    };
}

/// Test logging macro - only enabled in test builds
#[macro_export]
macro_rules! test_log {
    ($($arg:tt)*) => {
        #[cfg(test)]
        {
            if std::env::var("BADBATCH_TEST_LOG").map(|s| s == "1" || s.to_lowercase() == "true").unwrap_or(false) {
                println!("[BADBATCH TEST] {}", format!($($arg)*));
            }
        }
    };
}
