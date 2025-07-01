use badbatch::disruptor::*;
use std::sync::Arc;

fn main() {
    println!("Debug MPSC issue...");
    
    // Create a simple multi-producer sequencer with size 64
    let wait_strategy = Arc::new(BlockingWaitStrategy::new());
    let sequencer = MultiProducerSequencer::new(64, wait_strategy);
    
    println!("Created sequencer with buffer size: {}", sequencer.get_buffer_size());
    
    // Test publishing sequence 0
    println!("Publishing sequence 0...");
    sequencer.publish(0);
    
    // Check if sequence 0 is available
    let available = sequencer.is_available(0);
    println!("Sequence 0 available: {}", available);
    
    // Test publishing sequence 1
    println!("Publishing sequence 1...");
    sequencer.publish(1);
    
    // Check if sequence 1 is available
    let available = sequencer.is_available(1);
    println!("Sequence 1 available: {}", available);
    
    // Test highest published sequence
    let highest = sequencer.get_highest_published_sequence(0, 2);
    println!("Highest published sequence (0->2): {}", highest);
}