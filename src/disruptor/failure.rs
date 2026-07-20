//! Structured failure context for managed pipelines.

use std::any::Any;
use std::fmt;

/// Lifecycle or processing phase in which a managed pipeline failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailurePhase {
    /// A requested consumer-thread CPU affinity could not be applied.
    ThreadAffinity,
    /// [`crate::disruptor::EventHandler::on_start`] returned an error.
    HandlerStart,
    /// [`crate::disruptor::EventHandler::on_batch_start`] returned an error.
    BatchStart,
    /// Event processing returned an error and the selected policy stopped.
    EventProcessing,
    /// [`crate::disruptor::EventHandler::on_shutdown`] returned an error.
    HandlerShutdown,
    /// A consumer callback or consumer execution path panicked.
    ConsumerPanic,
    /// A producer update/translator panicked after claim and before publish.
    ProducerPanic,
    /// A managed processor thread returned an error or panicked while joining.
    ProcessorThread,
}

impl FailurePhase {
    /// Stable lowercase name suitable for logs and metrics labels.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThreadAffinity => "thread_affinity",
            Self::HandlerStart => "handler_start",
            Self::BatchStart => "batch_start",
            Self::EventProcessing => "event_processing",
            Self::HandlerShutdown => "handler_shutdown",
            Self::ConsumerPanic => "consumer_panic",
            Self::ProducerPanic => "producer_panic",
            Self::ProcessorThread => "processor_thread",
        }
    }
}

impl fmt::Display for FailurePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The first structured failure observed by a managed pipeline.
///
/// Built-in sequencers retain the first record for the lifetime of the
/// pipeline. Later failures are still eligible for logging, but do not replace
/// the causal record returned by `first_failure()` APIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureRecord {
    phase: FailurePhase,
    message: String,
    thread_name: Option<String>,
    stage_index: Option<usize>,
    sequence: Option<i64>,
}

impl FailureRecord {
    /// Create a record with a phase and human-readable error message.
    #[must_use]
    pub fn new(phase: FailurePhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: message.into(),
            thread_name: None,
            stage_index: None,
            sequence: None,
        }
    }

    /// Attach the managed thread name.
    #[must_use]
    pub fn with_thread_name(mut self, thread_name: impl Into<String>) -> Self {
        self.thread_name = Some(thread_name.into());
        self
    }

    /// Attach the zero-based Builder pipeline stage.
    #[must_use]
    pub fn with_stage_index(mut self, stage_index: usize) -> Self {
        self.stage_index = Some(stage_index);
        self
    }

    /// Attach the event sequence at which the failure occurred.
    #[must_use]
    pub fn with_sequence(mut self, sequence: i64) -> Self {
        self.sequence = Some(sequence);
        self
    }

    /// Failure phase.
    #[must_use]
    pub const fn phase(&self) -> FailurePhase {
        self.phase
    }

    /// Human-readable root error or panic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Managed thread name, when known.
    #[must_use]
    pub fn thread_name(&self) -> Option<&str> {
        self.thread_name.as_deref()
    }

    /// Zero-based Builder stage, when known.
    #[must_use]
    pub const fn stage_index(&self) -> Option<usize> {
        self.stage_index
    }

    /// Event sequence, when the failure is tied to one event or batch.
    #[must_use]
    pub const fn sequence(&self) -> Option<i64> {
        self.sequence
    }
}

impl fmt::Display for FailureRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "phase={}", self.phase)?;
        if let Some(thread_name) = &self.thread_name {
            write!(f, " thread={thread_name:?}")?;
        }
        if let Some(stage_index) = self.stage_index {
            write!(f, " stage={stage_index}")?;
        }
        if let Some(sequence) = self.sequence {
            write!(f, " sequence={sequence}")?;
        }
        write!(f, " error={:?}", self.message)
    }
}

pub(crate) fn current_thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string()
}

pub(crate) fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum FailureDecision {
    Poison,
    Record,
}

impl FailureDecision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Poison => "poison",
            Self::Record => "record",
        }
    }
}

pub(crate) fn log_failure(failure: &FailureRecord, decision: FailureDecision) {
    let decision = decision.as_str();
    log::error!(target: "badbatch::failure", "{failure} decision={decision}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_uses_context_without_event_payload() {
        let record = FailureRecord::new(FailurePhase::EventProcessing, "handler rejected event")
            .with_thread_name("orders")
            .with_stage_index(2)
            .with_sequence(41);

        assert_eq!(
            record.to_string(),
            "phase=event_processing thread=\"orders\" stage=2 sequence=41 error=\"handler rejected event\""
        );
    }

    #[test]
    fn extracts_string_panic_payloads() {
        assert_eq!(panic_payload_message(&"boom"), "boom");
        assert_eq!(panic_payload_message(&String::from("owned")), "owned");
        assert_eq!(panic_payload_message(&7_u8), "non-string panic payload");
    }
}
