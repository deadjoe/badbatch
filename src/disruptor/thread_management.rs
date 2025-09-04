//! Thread management and CPU affinity module inspired by disruptor-rs
//!
//! This module provides thread management capabilities including CPU core pinning,
//! thread naming, and automatic thread lifecycle management for event processors.

use core_affinity::CoreId;
use std::thread::{self, JoinHandle};

/// Thread context for managing processor threads
///
/// This follows the disruptor-rs pattern for thread configuration,
/// allowing fine-grained control over thread naming and CPU affinity.
#[derive(Debug, Clone, Default)]
pub struct ThreadContext {
    /// CPU core affinity (optional)
    affinity: Option<CoreId>,
    /// Thread name (optional)
    name: Option<String>,
    /// Thread ID counter for automatic naming
    id: usize,
}

impl ThreadContext {
    /// Create a new thread context
    pub fn new() -> Self {
        Self::default()
    }

    /// Set CPU core affinity
    ///
    /// # Arguments
    /// * `core_id` - The CPU core ID to pin the thread to
    ///
    /// # Panics
    /// Panics if the specified core ID is not available on the system
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        validate_core_id(core_id);
        self.affinity = Some(CoreId { id: core_id });
        self
    }

    /// Set thread name
    ///
    /// # Arguments
    /// * `name` - The name for the thread
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Get the thread name, generating one if not set
    pub(crate) fn get_name(&mut self) -> String {
        self.name.take().unwrap_or_else(|| {
            self.id += 1;
            format!("processor-{id}", id = self.id)
        })
    }

    /// Get the CPU affinity, consuming it
    pub(crate) fn get_affinity(&mut self) -> Option<CoreId> {
        self.affinity.take()
    }
}

/// Managed thread wrapper for event processors
///
/// This provides automatic thread lifecycle management and graceful shutdown.
pub struct ManagedThread {
    join_handle: Option<JoinHandle<()>>,
    thread_name: String,
}

impl ManagedThread {
    /// Create a new managed thread
    pub(crate) fn new(join_handle: JoinHandle<()>, thread_name: String) -> Self {
        Self {
            join_handle: Some(join_handle),
            thread_name,
        }
    }

    /// Get the thread name
    pub fn thread_name(&self) -> &str {
        &self.thread_name
    }

    /// Join the thread, waiting for it to complete
    ///
    /// This consumes the ManagedThread and waits for the underlying thread to finish.
    /// If the thread has already been joined, this is a no-op.
    pub fn join(mut self) -> thread::Result<()> {
        if let Some(handle) = self.join_handle.take() {
            handle.join()
        } else {
            Ok(())
        }
    }

    /// Check if the thread is still running
    pub fn is_running(&self) -> bool {
        self.join_handle.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for ManagedThread {
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            // Try to join the thread, but don't panic if it fails
            let _ = handle.join();
        }
    }
}

/// Thread builder with CPU affinity and naming support
///
/// This provides a convenient interface for creating threads with
/// specific CPU affinity and naming, following disruptor-rs patterns.
pub struct ThreadBuilder {
    context: ThreadContext,
}

impl ThreadBuilder {
    /// Create a new thread builder
    pub fn new() -> Self {
        Self {
            context: ThreadContext::new(),
        }
    }

    /// Set CPU core affinity
    ///
    /// # Arguments
    /// * `core_id` - The CPU core ID to pin the thread to
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.context = self.context.pin_at_core(core_id);
        self
    }

    /// Set thread name
    ///
    /// # Arguments
    /// * `name` - The name for the thread
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.context = self.context.thread_name(name);
        self
    }

    /// Spawn a thread with the configured settings
    ///
    /// # Arguments
    /// * `f` - The function to run in the thread
    ///
    /// # Returns
    /// A ManagedThread that can be used to control the thread lifecycle
    pub fn spawn<F>(mut self, f: F) -> std::io::Result<ManagedThread>
    where
        F: FnOnce() + Send + 'static,
    {
        let thread_name = self.context.get_name();
        let affinity = self.context.get_affinity();

        let builder = thread::Builder::new().name(thread_name.clone());
        let thread_name_for_closure = thread_name.clone();
        let join_handle = builder.spawn(move || {
            // Set CPU affinity if specified
            set_affinity_if_defined(affinity, &thread_name_for_closure);

            // Run the user function
            f();
        })?;

        Ok(ManagedThread::new(join_handle, thread_name))
    }
}

impl Default for ThreadBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate that a CPU core ID is available on the system
///
/// # Arguments
/// * `core_id` - The CPU core ID to validate
///
/// # Panics
/// Panics if the specified core ID is not available
fn validate_core_id(core_id: usize) {
    let available_cores: Vec<usize> = core_affinity::get_core_ids()
        .unwrap_or_default()
        .iter()
        .map(|core| core.id)
        .collect();

    if !available_cores.contains(&core_id) {
        panic!("CPU core {core_id} is not available. Available cores: {available_cores:?}");
    }
}

/// Set CPU affinity for the current thread if specified
///
/// # Arguments
/// * `affinity` - Optional CPU core affinity
/// * `thread_name` - Thread name for error reporting
fn set_affinity_if_defined(affinity: Option<CoreId>, thread_name: &str) {
    if let Some(core_id) = affinity {
        let success = core_affinity::set_for_current(core_id);
        if !success {
            crate::internal_warn!(
                "Warning: Could not pin thread '{thread_name}' to CPU core {}",
                core_id.id
            );
        } else {
            crate::internal_debug!(
                "Successfully pinned thread '{thread_name}' to CPU core {}",
                core_id.id
            );
        }
    }
}

/// Get available CPU core IDs
///
/// # Returns
/// A vector of available CPU core IDs
pub fn get_available_cores() -> Vec<usize> {
    core_affinity::get_core_ids()
        .unwrap_or_default()
        .iter()
        .map(|core| core.id)
        .collect()
}

/// Get the first available CPU core ID
///
/// This function returns the first available CPU core from the system's core list,
/// not necessarily the core that the current thread is running on.
///
/// # Returns
/// The first available CPU core ID, if any cores are available
pub fn get_first_available_core() -> Option<usize> {
    core_affinity::get_core_ids()
        .unwrap_or_default()
        .into_iter()
        .next()
        .map(|core| core.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_thread_context_creation() {
        let context = ThreadContext::new();
        assert!(context.affinity.is_none());
        assert!(context.name.is_none());
        assert_eq!(context.id, 0);
    }

    #[test]
    fn test_thread_context_configuration() {
        let available_cores = get_available_cores();
        if !available_cores.is_empty() {
            let core_id = available_cores[0];
            let context = ThreadContext::new()
                .pin_at_core(core_id)
                .thread_name("test-thread");

            assert_eq!(context.affinity.unwrap().id, core_id);
            assert_eq!(context.name.unwrap(), "test-thread");
        }
    }

    #[test]
    fn test_thread_builder() {
        let counter = Arc::new(Mutex::new(0));
        let counter_clone = counter.clone();

        // Use a barrier to ensure the thread doesn't complete too quickly
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let barrier_clone = barrier.clone();

        let managed_thread = ThreadBuilder::new()
            .thread_name("test-worker")
            .spawn(move || {
                // Wait for the main thread to check is_running()
                barrier_clone.wait();
                *counter_clone.lock().unwrap() = 42;
            })
            .expect("Failed to spawn thread");

        assert_eq!(managed_thread.thread_name(), "test-worker");

        // Check that the thread is running before it completes
        assert!(managed_thread.is_running());

        // Release the barrier to let the thread complete
        barrier.wait();

        // Wait for thread to complete
        managed_thread
            .join()
            .expect("Thread should complete successfully");

        assert_eq!(*counter.lock().unwrap(), 42);
    }

    #[test]
    fn test_get_available_cores() {
        let cores = get_available_cores();
        // Should have at least one core on any system
        assert!(!cores.is_empty());
        crate::test_log!("Available CPU cores: {cores:?}");
    }

    #[test]
    #[cfg(not(miri))] // Skip in Miri as it doesn't support CPU affinity
    fn test_cpu_affinity() {
        let available_cores = get_available_cores();
        if available_cores.len() > 1 {
            let core_id = available_cores[0];
            let result = Arc::new(Mutex::new(false));
            let result_clone = result.clone();

            let managed_thread = ThreadBuilder::new()
                .pin_at_core(core_id)
                .thread_name("affinity-test")
                .spawn(move || {
                    // Just mark that the thread ran successfully
                    *result_clone.lock().unwrap() = true;
                })
                .expect("Failed to spawn thread");

            managed_thread.join().expect("Thread should complete");
            assert!(*result.lock().unwrap());
        }
    }

    #[test]
    fn test_automatic_thread_naming() {
        let mut context = ThreadContext::new();

        let name1 = context.get_name();
        let name2 = context.get_name();

        assert_eq!(name1, "processor-1");
        assert_eq!(name2, "processor-2");
    }
}
