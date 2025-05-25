//! Disruptor Manager
//!
//! This module provides centralized management of Disruptor instances,
//! handling their lifecycle, state tracking, and coordination with the API layer.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use crate::api::error::{ApiResult, ApiError};
use crate::api::models::{DisruptorInfo, DisruptorStatus, DisruptorConfig};
use crate::disruptor::{
    Disruptor, ProducerType, WaitStrategy, DefaultEventFactory,
    BlockingWaitStrategy, YieldingWaitStrategy, BusySpinWaitStrategy, SleepingWaitStrategy,
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
    pub disruptor: Arc<Disruptor<ApiEvent>>,
}

/// Global Disruptor manager for handling all Disruptor instances
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
            return Err(ApiError::invalid_request("buffer_size must be a power of 2"));
        }

        if buffer_size < 2 || buffer_size > 1024 * 1024 {
            return Err(ApiError::invalid_request("buffer_size must be between 2 and 1048576"));
        }

        // Parse producer type
        let producer_type_enum = match producer_type.to_lowercase().as_str() {
            "single" => ProducerType::Single,
            "multi" => ProducerType::Multi,
            _ => return Err(ApiError::invalid_request("Invalid producer_type. Must be 'single' or 'multi'")),
        };

        // Create wait strategy
        let wait_strategy_impl: Box<dyn WaitStrategy> = match wait_strategy.to_lowercase().as_str() {
            "blocking" => Box::new(BlockingWaitStrategy::new()),
            "yielding" => Box::new(YieldingWaitStrategy::new()),
            "busy_spin" => Box::new(BusySpinWaitStrategy::new()),
            "sleeping" => Box::new(SleepingWaitStrategy::new()),
            _ => return Err(ApiError::invalid_request(
                "Invalid wait_strategy. Must be 'blocking', 'yielding', 'busy_spin', or 'sleeping'"
            )),
        };

        // Create event factory
        let event_factory = DefaultEventFactory::<ApiEvent>::new();

        // Create the actual Disruptor
        let disruptor = Disruptor::new(
            event_factory,
            buffer_size,
            producer_type_enum,
            wait_strategy_impl,
        ).map_err(|e| ApiError::internal(format!("Failed to create Disruptor: {}", e)))?;

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
            disruptor: Arc::new(disruptor),
        };

        // Store in manager
        {
            let mut disruptors = self.disruptors.write()
                .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;
            disruptors.insert(disruptor_id, managed);
        }

        Ok(disruptor_info)
    }

    /// Get Disruptor information by ID
    pub fn get_disruptor_info(&self, id: &str) -> ApiResult<DisruptorInfo> {
        let disruptors = self.disruptors.read()
            .map_err(|_| ApiError::internal("Failed to acquire read lock"))?;

        match disruptors.get(id) {
            Some(managed) => Ok(managed.info.clone()),
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// Update Disruptor status
    pub fn update_status(&self, id: &str, status: DisruptorStatus) -> ApiResult<()> {
        let mut disruptors = self.disruptors.write()
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
        let mut disruptors = self.disruptors.write()
            .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;

        match disruptors.get_mut(id) {
            Some(managed) => {
                match managed.info.status {
                    DisruptorStatus::Created | DisruptorStatus::Stopped | DisruptorStatus::Paused => {
                        // TODO: Actually start the Disruptor with event processors
                        // For now, just update the status
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
                    DisruptorStatus::Error => {
                        Err(ApiError::invalid_request("Disruptor is in error state. Delete and recreate."))
                    }
                }
            }
            None => Err(ApiError::disruptor_not_found(id)),
        }
    }

    /// Stop a Disruptor instance
    pub fn stop_disruptor(&self, id: &str) -> ApiResult<DisruptorInfo> {
        let mut disruptors = self.disruptors.write()
            .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;

        match disruptors.get_mut(id) {
            Some(managed) => {
                match managed.info.status {
                    DisruptorStatus::Running | DisruptorStatus::Paused => {
                        // TODO: Actually stop the Disruptor
                        managed.info.status = DisruptorStatus::Stopped;
                        managed.info.last_activity = chrono::Utc::now();
                        Ok(managed.info.clone())
                    }
                    DisruptorStatus::Stopped => {
                        Err(ApiError::invalid_request("Disruptor is already stopped"))
                    }
                    DisruptorStatus::Created => {
                        Err(ApiError::invalid_request("Disruptor has not been started yet"))
                    }
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
        let disruptors = self.disruptors.read()
            .map_err(|_| ApiError::internal("Failed to acquire read lock"))?;

        let infos: Vec<DisruptorInfo> = disruptors.values()
            .map(|managed| managed.info.clone())
            .collect();

        Ok(infos)
    }

    /// Remove a Disruptor instance
    pub fn remove_disruptor(&self, id: &str) -> ApiResult<()> {
        let mut disruptors = self.disruptors.write()
            .map_err(|_| ApiError::internal("Failed to acquire write lock"))?;

        match disruptors.get(id) {
            Some(managed) => {
                // Check if it's safe to delete
                match managed.info.status {
                    DisruptorStatus::Running => {
                        return Err(ApiError::invalid_request(
                            "Cannot delete a running Disruptor. Stop it first."
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
    pub fn get_disruptor(&self, id: &str) -> ApiResult<Arc<Disruptor<ApiEvent>>> {
        let disruptors = self.disruptors.read()
            .map_err(|_| ApiError::internal("Failed to acquire read lock"))?;

        match disruptors.get(id) {
            Some(managed) => Ok(managed.disruptor.clone()),
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

        let info = manager.create_disruptor(
            Some("test".to_string()),
            1024,
            "single",
            "blocking",
            config,
        ).unwrap();

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
            None,
            1023, // Not a power of 2
            "single",
            "blocking",
            config,
        );

        assert!(result.is_err());
    }
}
