//! Global Disruptor Manager
//!
//! This module provides a global singleton instance of the DisruptorManager
//! that can be shared across all API handlers.

use crate::api::manager::DisruptorManager;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

/// Global manager instance - shared across all API handlers
static GLOBAL_MANAGER: OnceLock<Arc<Mutex<DisruptorManager>>> = OnceLock::new();

/// Get the global DisruptorManager instance
pub fn get_global_manager() -> &'static Arc<Mutex<DisruptorManager>> {
    GLOBAL_MANAGER.get_or_init(|| Arc::new(Mutex::new(DisruptorManager::new())))
}

/// For testing: create a new manager instance
/// Since OnceLock doesn't support reset, we provide a way to get a fresh manager for tests
#[cfg(test)]
pub fn create_test_manager() -> Arc<Mutex<DisruptorManager>> {
    Arc::new(Mutex::new(DisruptorManager::new()))
}
