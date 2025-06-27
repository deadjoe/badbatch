//! Producer Type Implementation
//!
//! This module provides the ProducerType enum for specifying whether the Disruptor
//! should be configured for single or multiple producers. This follows the exact
//! design from the original LMAX Disruptor ProducerType enum.

/// Specifies the type of producer for the Disruptor
///
/// This enum is used to configure the Disruptor for optimal performance based on
/// whether there will be a single producer or multiple producers publishing events.
/// This matches the original LMAX Disruptor ProducerType enum exactly.
///
/// # Examples
/// ```
/// use badbatch::disruptor::ProducerType;
///
/// let single_producer = ProducerType::Single;
/// let multi_producer = ProducerType::Multi;
///
/// assert!(single_producer.is_single());
/// assert!(multi_producer.is_multi());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProducerType {
    /// Single producer - optimized for scenarios where only one thread will be publishing events
    ///
    /// This provides the best performance when you can guarantee that only one thread
    /// will be calling the publish methods on the Disruptor. The single producer sequencer
    /// uses simpler algorithms that don't require coordination between multiple producers.
    ///
    /// Use this when:
    /// - Only one thread will ever publish events
    /// - You need maximum throughput and minimum latency
    /// - You can guarantee single-threaded publishing
    Single,

    /// Multiple producers - supports scenarios where multiple threads will be publishing events
    ///
    /// This uses more complex algorithms to handle concurrent access from multiple producer threads.
    /// It has slightly lower performance than the single producer variant due to the need for
    /// coordination between producers, but it's still very fast.
    ///
    /// Use this when:
    /// - Multiple threads need to publish events concurrently
    /// - You can't guarantee single-threaded publishing
    /// - You need thread-safe publishing from multiple sources
    Multi,
}

impl ProducerType {
    /// Returns true if this is a single producer type
    ///
    /// # Returns
    /// True if this is ProducerType::Single, false otherwise
    ///
    /// # Examples
    /// ```
    /// use badbatch::disruptor::ProducerType;
    ///
    /// assert!(ProducerType::Single.is_single());
    /// assert!(!ProducerType::Multi.is_single());
    /// ```
    pub fn is_single(&self) -> bool {
        matches!(self, ProducerType::Single)
    }

    /// Returns true if this is a multi producer type
    ///
    /// # Returns
    /// True if this is ProducerType::Multi, false otherwise
    ///
    /// # Examples
    /// ```
    /// use badbatch::disruptor::ProducerType;
    ///
    /// assert!(!ProducerType::Single.is_multi());
    /// assert!(ProducerType::Multi.is_multi());
    /// ```
    pub fn is_multi(&self) -> bool {
        matches!(self, ProducerType::Multi)
    }
}

impl Default for ProducerType {
    /// Default to multi-producer for safety
    ///
    /// The default is multi-producer because it's safer - it will work correctly
    /// even if multiple threads try to publish events. Single producer mode
    /// should be explicitly chosen when you can guarantee single-threaded publishing.
    ///
    /// # Returns
    /// ProducerType::Multi
    fn default() -> Self {
        ProducerType::Multi
    }
}

impl std::fmt::Display for ProducerType {
    /// Format the producer type for display
    ///
    /// # Arguments
    /// * `f` - The formatter
    ///
    /// # Returns
    /// Result of the formatting operation
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProducerType::Single => write!(f, "Single"),
            ProducerType::Multi => write!(f, "Multi"),
        }
    }
}

impl std::str::FromStr for ProducerType {
    type Err = String;

    /// Parse a producer type from a string
    ///
    /// Accepts "single", "Single", "SINGLE", "multi", "Multi", "MULTI"
    ///
    /// # Arguments
    /// * `s` - The string to parse
    ///
    /// # Returns
    /// Result containing the parsed ProducerType or an error message
    ///
    /// # Examples
    /// ```
    /// use badbatch::disruptor::ProducerType;
    /// use std::str::FromStr;
    ///
    /// assert_eq!(ProducerType::from_str("single").unwrap(), ProducerType::Single);
    /// assert_eq!(ProducerType::from_str("Multi").unwrap(), ProducerType::Multi);
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "single" => Ok(ProducerType::Single),
            "multi" => Ok(ProducerType::Multi),
            _ => Err(format!(
                "Invalid producer type: '{s}'. Valid values are 'single' or 'multi'"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_producer_type_single() {
        let producer_type = ProducerType::Single;
        assert!(producer_type.is_single());
        assert!(!producer_type.is_multi());
        assert_eq!(producer_type.to_string(), "Single");
    }

    #[test]
    fn test_producer_type_multi() {
        let producer_type = ProducerType::Multi;
        assert!(!producer_type.is_single());
        assert!(producer_type.is_multi());
        assert_eq!(producer_type.to_string(), "Multi");
    }

    #[test]
    fn test_producer_type_default() {
        let producer_type = ProducerType::default();
        assert_eq!(producer_type, ProducerType::Multi);
        assert!(producer_type.is_multi());
    }

    #[test]
    fn test_producer_type_equality() {
        assert_eq!(ProducerType::Single, ProducerType::Single);
        assert_eq!(ProducerType::Multi, ProducerType::Multi);
        assert_ne!(ProducerType::Single, ProducerType::Multi);
    }

    #[test]
    fn test_producer_type_clone() {
        let original = ProducerType::Single;
        let cloned = original;
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_producer_type_from_str() {
        assert_eq!(
            ProducerType::from_str("single").unwrap(),
            ProducerType::Single
        );
        assert_eq!(
            ProducerType::from_str("Single").unwrap(),
            ProducerType::Single
        );
        assert_eq!(
            ProducerType::from_str("SINGLE").unwrap(),
            ProducerType::Single
        );

        assert_eq!(
            ProducerType::from_str("multi").unwrap(),
            ProducerType::Multi
        );
        assert_eq!(
            ProducerType::from_str("Multi").unwrap(),
            ProducerType::Multi
        );
        assert_eq!(
            ProducerType::from_str("MULTI").unwrap(),
            ProducerType::Multi
        );

        assert!(ProducerType::from_str("invalid").is_err());
        assert!(ProducerType::from_str("").is_err());
    }

    #[test]
    fn test_producer_type_hash() {
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert(ProducerType::Single, "single");
        map.insert(ProducerType::Multi, "multi");

        assert_eq!(map.get(&ProducerType::Single), Some(&"single"));
        assert_eq!(map.get(&ProducerType::Multi), Some(&"multi"));
    }
}
