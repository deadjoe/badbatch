//! Disruptor Manager
//!
//! This module provides centralized management of Disruptor instances,
//! handling their lifecycle, state tracking, and coordination with the API layer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use uuid::Uuid;

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{DisruptorConfig, DisruptorInfo, DisruptorStatus};
use crate::disruptor::{
    BlockingWaitStrategy, BusySpinWaitStrategy, DefaultEventFactory, Disruptor, ProducerType,
    SleepingWaitStrategy, WaitStrategy, YieldingWaitStrategy,
};

/// Event type used for API-managed Disruptors
#[derive(Debug, Default, Clone)]
pub struct ApiEvent {
    pub data: serde_json::Value,
    pub metadata: Option<HashMap<String, String>>,
    pub correlation_id: Option<String>,
}

/// Managed Disruptor instance with metadata
#[derive(Debug)]
pub struct ManagedDisruptor {
    pub info: DisruptorInfo,
    pub disruptor: Arc<Mutex<Disruptor<ApiEvent>>>,
}

/// Global Disruptor manager for handling all Disruptor instances
///
/// This manager uses fine-grained locking to minimize contention:
/// - Read operations use RwLock for concurrent access
/// - Individual Disruptor operations use separate locks
/// - Metadata operations are separated from data operations
#[derive(Debug)]
pub struct DisruptorManager {
    disruptors: Arc<RwLock<HashMap<String, ManagedDisruptor>>>,
}

impl DisruptorManager {
    /// Create a new DisruptorManager
    pub fn new() -> Self {
        Self {
            disruptors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new Disruptor instance
    pub fn create_disruptor(
        &self,
        name: Option<String>,
        buffer_size: usize,
        producer_type: &str,
        wait_strategy: &str,
        config: DisruptorConfig,
    ) -> ApiResult<DisruptorInfo> {
        // Validate buffer size
        if !crate::disruptor::is_power_of_two(buffer_size) {
            return Err(ApiError::invalid_request(
                "buffer_size must be a power of 2",
            ));
        }

        if !(2..=1024 * 1024).contains(&buffer_size) {
            return Err(ApiError::invalid_request(
                "buffer_size must be between 2 and 1048576",
            ));
        }

        // Parse producer type
        let producer_type_enum = match producer_type.to_lowercase().as_str() {
            "single" => ProducerType::Single,
            "multi" => ProducerType::Multi,
            _ => {
                return Err(ApiError::invalid_request(
                    "Invalid producer_type. Must be 'single' or 'multi'",
                ))
            }
        };

        // Create wait strategy
        let wait_strategy_impl: Box<dyn WaitStrategy> = match wait_strategy.to_lowercase().as_str()
        {
            "blocking" => Box::new(BlockingWaitStrategy::new()),
            "yielding" => Box::new(YieldingWaitStrategy::new()),
            "busy_spin" => Box::new(BusySpinWaitStrategy::new()),
            "sleeping" => Box::new(SleepingWaitStrategy::new()),
            _ => return Err(ApiError::invalid_request(
                "Invalid wait_strategy. Must be 'blocking', 'yielding', 'busy_spin', or 'sleeping'",
            )),
        };

        // Create event factory
        let event_factory = DefaultEventFactory::<ApiEvent>::new();

        // Create the actual Disruptor with a default event handler
        use crate::disruptor::NoOpEventHandler;
        let disruptor = Disruptor::new(
            event_factory,
            buffer_size,
            producer_type_enum,
            wait_strategy_impl,
        )
        .map_err(|e| ApiError::internal(format!("Failed to create Disruptor: {}", e)))?
        .handle_events_with(NoOpEventHandler::<ApiEvent>::new())
        .build();

        // Generate unique ID
        let disruptor_id = Uuid::new_v4().to_string();

        // Create DisruptorInfo
        let disruptor_info = DisruptorInfo {
            id: disruptor_id.clone(),
            name: name.clone(),
            buffer_size,
            producer_type: producer_type.to_string(),
            wait_strategy: wait_strategy.to_string(),
            status: DisruptorStatus::Created,
            config,
            created_at: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
        };

        // Create managed instance
        let managed = ManagedDisruptor {
            info: disruptor_info.clone(),
            disruptor: Arc::new(Mutex::new(disruptor)),
        };

        // Store in manager
        {
            let mut disruptors = self
                .disruptors
                .write()
                .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;
            disruptors.insert(disruptor_id, managed);
        }

        Ok(disruptor_info)
    }

    /// Get Disruptor information by ID
    pub fn get_disruptor_info(&self, id: &str) -> ApiResult<DisruptorInfo> {
        let disruptors = self
            .disruptors
            .read()
            .map_err(|_| ApiError::internal("Failed to acquire read lock"))?;

        match disruptors.get(id) {
            Some(managed) => Ok(managed.info.clone()),
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// Update Disruptor status
    pub fn update_status(&self, id: &str, status: DisruptorStatus) -> ApiResult<()> {
        let mut disruptors = self
            .disruptors
            .write()
            .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;

        match disruptors.get_mut(id) {
            Some(managed) => {
                managed.info.status = status;
                managed.info.last_activity = chrono::Utc::now();
                Ok(())
            }
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// Start a Disruptor instance
    pub fn start_disruptor(&self, id: &str) -> ApiResult<DisruptorInfo> {
        let mut disruptors = self
            .disruptors
            .write()
            .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;

        match disruptors.get_mut(id) {
            Some(managed) => {
                match managed.info.status {
                    DisruptorStatus::Created
                    | DisruptorStatus::Stopped
                    | DisruptorStatus::Paused => {
                        // Actually start the Disruptor with event processors
                        let mut disruptor = managed
                            .disruptor
                            .lock()
                            .map_err(|_| ApiError::internal("Failed to acquire disruptor lock"))?;

                        // Start the disruptor (this will start all event processors)
                        disruptor.start().map_err(|e| {
                            ApiError::internal(format!("Failed to start Disruptor: {}", e))
                        })?;

                        // Update status
                        managed.info.status = DisruptorStatus::Running;
                        managed.info.last_activity = chrono::Utc::now();
                        Ok(managed.info.clone())
                    }
                    DisruptorStatus::Running => {
                        Err(ApiError::invalid_request("Disruptor is already running"))
                    }
                    DisruptorStatus::Stopping => {
                        Err(ApiError::invalid_request("Disruptor is currently stopping"))
                    }
                    DisruptorStatus::Error => Err(ApiError::invalid_request(
                        "Disruptor is in error state. Delete and recreate.",
                    )),
                }
            }
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// Stop a Disruptor instance
    pub fn stop_disruptor(&self, id: &str) -> ApiResult<DisruptorInfo> {
        let mut disruptors = self
            .disruptors
            .write()
            .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;

        match disruptors.get_mut(id) {
            Some(managed) => {
                match managed.info.status {
                    DisruptorStatus::Running | DisruptorStatus::Paused => {
                        // Set status to stopping first
                        managed.info.status = DisruptorStatus::Stopping;
                        managed.info.last_activity = chrono::Utc::now();

                        // Actually stop the Disruptor
                        let mut disruptor = managed
                            .disruptor
                            .lock()
                            .map_err(|_| ApiError::internal("Failed to acquire disruptor lock"))?;

                        // Shutdown the disruptor (this will stop all event processors)
                        disruptor.shutdown().map_err(|e| {
                            ApiError::internal(format!("Failed to stop Disruptor: {}", e))
                        })?;

                        // Update status to stopped
                        managed.info.status = DisruptorStatus::Stopped;
                        managed.info.last_activity = chrono::Utc::now();
                        Ok(managed.info.clone())
                    }
                    DisruptorStatus::Stopped => {
                        Err(ApiError::invalid_request("Disruptor is already stopped"))
                    }
                    DisruptorStatus::Created => Err(ApiError::invalid_request(
                        "Disruptor has not been started yet",
                    )),
                    DisruptorStatus::Stopping => {
                        Err(ApiError::invalid_request("Disruptor is already stopping"))
                    }
                    DisruptorStatus::Error => {
                        Err(ApiError::invalid_request("Disruptor is in error state"))
                    }
                }
            }
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// List all Disruptors
    pub fn list_disruptors(&self) -> ApiResult<Vec<DisruptorInfo>> {
        let disruptors = self
            .disruptors
            .read()
            .map_err(|_| ApiError::internal("Failed to acquire read lock"))?;

        let infos: Vec<DisruptorInfo> = disruptors
            .values()
            .map(|managed| managed.info.clone())
            .collect();

        Ok(infos)
    }

    /// Remove a Disruptor instance
    pub fn remove_disruptor(&self, id: &str) -> ApiResult<()> {
        let mut disruptors = self
            .disruptors
            .write()
            .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;

        match disruptors.get_mut(id) {
            Some(managed) => {
                // Check if it's safe to delete and stop if necessary
                match managed.info.status {
                    DisruptorStatus::Running | DisruptorStatus::Paused => {
                        // Force stop the disruptor before removal
                        let mut disruptor = managed
                            .disruptor
                            .lock()
                            .map_err(|_| ApiError::internal("Failed to acquire disruptor lock"))?;

                        // Shutdown the disruptor
                        disruptor.shutdown().map_err(|e| {
                            ApiError::internal(format!(
                                "Failed to stop Disruptor during removal: {}",
                                e
                            ))
                        })?;

                        managed.info.status = DisruptorStatus::Stopped;
                    }
                    DisruptorStatus::Stopping => {
                        return Err(ApiError::invalid_request(
                            "Cannot delete a Disruptor that is currently stopping. Wait for it to stop first."
                        ));
                    }
                    _ => {}
                }

                // Remove from storage
                disruptors.remove(id);
                Ok(())
            }
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// Get the actual Disruptor instance for event publishing
    ///
    /// This method uses a read lock for fast concurrent access and immediately
    /// clones the Arc to minimize lock holding time.
    pub fn get_disruptor(&self, id: &str) -> ApiResult<Arc<Mutex<Disruptor<ApiEvent>>>> {
        // Use a read lock for concurrent access
        let disruptors = self
            .disruptors
            .read()
            .map_err(|_| ApiError::internal("Failed to acquire read lock"))?;

        match disruptors.get(id) {
            Some(managed) => {
                // Clone the Arc immediately to release the lock quickly
                let disruptor = managed.disruptor.clone();
                // Lock is automatically released here
                Ok(disruptor)
            }
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// Fast path to check if a Disruptor exists without acquiring locks on the Disruptor itself
    pub fn disruptor_exists(&self, id: &str) -> bool {
        if let Ok(disruptors) = self.disruptors.read() {
            disruptors.contains_key(id)
        } else {
            false
        }
    }

    /// Get Disruptor status quickly without locking the Disruptor instance
    pub fn get_disruptor_status(&self, id: &str) -> ApiResult<DisruptorStatus> {
        let disruptors = self
            .disruptors
            .read()
            .map_err(|_| ApiError::internal("Failed to acquire read lock"))?;

        match disruptors.get(id) {
            Some(managed) => Ok(managed.info.status.clone()),
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }
}

impl Default for DisruptorManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let manager = DisruptorManager::new();
        let disruptors = manager.list_disruptors().unwrap();
        assert!(disruptors.is_empty());
    }

    #[test]
    fn test_create_disruptor() {
        let manager = DisruptorManager::new();
        let config = DisruptorConfig::default();

        let info = manager
            .create_disruptor(Some("test".to_string()), 1024, "single", "blocking", config)
            .unwrap();

        assert_eq!(info.buffer_size, 1024);
        assert_eq!(info.producer_type, "single");
        assert_eq!(info.wait_strategy, "blocking");
        assert_eq!(info.status, DisruptorStatus::Created);
    }

    #[test]
    fn test_invalid_buffer_size() {
        let manager = DisruptorManager::new();
        let config = DisruptorConfig::default();

        let result = manager.create_disruptor(
            None, 1023, // Not a power of 2
            "single", "blocking", config,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_disruptor_lifecycle_management() {
        let manager = DisruptorManager::new();
        let config = DisruptorConfig::default();

        // Create a disruptor
        let info = manager
            .create_disruptor(
                Some("lifecycle-test".to_string()),
                1024,
                "single",
                "blocking",
                config,
            )
            .unwrap();

        assert_eq!(info.status, DisruptorStatus::Created);
        let disruptor_id = info.id.clone();

        // Start the disruptor
        let info = manager.start_disruptor(&disruptor_id).unwrap();
        assert_eq!(info.status, DisruptorStatus::Running);

        // Try to start again (should fail)
        let result = manager.start_disruptor(&disruptor_id);
        assert!(result.is_err());

        // Stop the disruptor
        let info = manager.stop_disruptor(&disruptor_id).unwrap();
        assert_eq!(info.status, DisruptorStatus::Stopped);

        // Try to stop again (should fail)
        let result = manager.stop_disruptor(&disruptor_id);
        assert!(result.is_err());

        // Remove the disruptor
        let result = manager.remove_disruptor(&disruptor_id);
        assert!(result.is_ok());

        // Try to get removed disruptor (should fail)
        let result = manager.get_disruptor_info(&disruptor_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_running_disruptor() {
        let manager = DisruptorManager::new();
        let config = DisruptorConfig::default();

        // Create and start a disruptor
        let info = manager
            .create_disruptor(
                Some("remove-test".to_string()),
                512,
                "single",
                "blocking",
                config,
            )
            .unwrap();

        let disruptor_id = info.id.clone();
        manager.start_disruptor(&disruptor_id).unwrap();

        // Remove should work (it will force stop first)
        let result = manager.remove_disruptor(&disruptor_id);
        assert!(result.is_ok());

        // Verify it's gone
        let result = manager.get_disruptor_info(&disruptor_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_disruptor_event_publishing_integration() {
        let manager = DisruptorManager::new();
        let config = DisruptorConfig::default();

        // Create a disruptor
        let info = manager
            .create_disruptor(
                Some("event-test".to_string()),
                1024,
                "single",
                "blocking",
                config,
            )
            .unwrap();

        let disruptor_id = info.id.clone();

        // Start the disruptor
        manager.start_disruptor(&disruptor_id).unwrap();

        // Get the disruptor for event publishing
        let disruptor = manager.get_disruptor(&disruptor_id).unwrap();
        let disruptor = disruptor.lock().unwrap();

        // Test event publishing
        use crate::disruptor::event_translator::ClosureEventTranslator;
        let translator = ClosureEventTranslator::new(|event: &mut ApiEvent, _sequence: i64| {
            event.data = serde_json::json!({"test": "data"});
            event.metadata = Some(std::collections::HashMap::new());
            event.correlation_id = Some("test-correlation".to_string());
        });

        // Publish an event
        let result = disruptor.try_publish_event(translator);
        assert!(result);

        // Verify cursor advanced
        assert_eq!(disruptor.get_cursor().get(), 0);

        // Drop the lock before stopping
        drop(disruptor);

        // Stop the disruptor
        let info = manager.stop_disruptor(&disruptor_id).unwrap();
        assert_eq!(info.status, DisruptorStatus::Stopped);
    }
}
