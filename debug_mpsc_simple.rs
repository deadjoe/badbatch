use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use std::sync::atomic::{AtomicI64, Ordering};

use badbatch::disruptor::*;

fn main() {
    println!("Simple MPSC Debug Test...");
    
    let wait_strategy = Arc::new(BlockingWaitStrategy::new());
    let sequencer = Arc::new(MultiProducerSequencer::new(64, wait_strategy));
    
    println!("Created sequencer with buffer size: {}", sequencer.get_buffer_size());
    
    let barrier = Arc::new(Barrier::new(3)); // 2 producers + 1 main thread
    let counter = Arc::new(AtomicI64::new(0));
    
    // Producer 1
    let sequencer1 = sequencer.clone();
    let barrier1 = barrier.clone();
    let counter1 = counter.clone();
    let producer1 = thread::spawn(move || {
        barrier1.wait();
        println!("Producer 1 starting...");
        
        for i in 0..5 {
            match sequencer1.next() {
                Ok(seq) => {
                    println!("Producer 1 claimed sequence: {}", seq);
                    // Simulate some work
                    thread::sleep(Duration::from_millis(1));
                    sequencer1.publish(seq);
                    println!("Producer 1 published sequence: {}", seq);
                    counter1.fetch_add(1, Ordering::Release);
                }
                Err(e) => {
                    println!("Producer 1 failed to claim sequence {}: {:?}", i, e);
                    break;
                }
            }
        }
        println!("Producer 1 finished");
    });
    
    // Producer 2  
    let sequencer2 = sequencer.clone();
    let barrier2 = barrier.clone();
    let counter2 = counter.clone();
    let producer2 = thread::spawn(move || {
        barrier2.wait();
        println!("Producer 2 starting...");
        
        for i in 0..5 {
            match sequencer2.next() {
                Ok(seq) => {
                    println!("Producer 2 claimed sequence: {}", seq);
                    // Simulate some work
                    thread::sleep(Duration::from_millis(1));
                    sequencer2.publish(seq);
                    println!("Producer 2 published sequence: {}", seq);
                    counter2.fetch_add(1, Ordering::Release);
                }
                Err(e) => {
                    println!("Producer 2 failed to claim sequence {}: {:?}", i, e);
                    break;
                }
            }
        }
        println!("Producer 2 finished");
    });
    
    // Main thread acts as consumer
    barrier.wait();
    println!("Consumer starting...");
    
    let mut next_seq = 0;
    let mut total_consumed = 0;
    
    // Wait for producers to finish
    thread::sleep(Duration::from_millis(100));
    
    // Consumer loop
    for _ in 0..50 { // Try up to 50 times
        let cursor = sequencer.get_cursor().get();
        if cursor >= next_seq {
            let highest = sequencer.get_highest_published_sequence(next_seq, cursor);
            println!("Consumer: cursor={}, next_seq={}, highest={}", cursor, next_seq, highest);
            
            if highest >= next_seq {
                for seq in next_seq..=highest {
                    if sequencer.is_available(seq) {
                        println!("Consumer: consumed sequence {}", seq);
                        total_consumed += 1;
                    } else {
                        println!("Consumer: sequence {} not available!", seq);
                    }
                }
                next_seq = highest + 1;
            }
        }
        
        // Check if we're done
        let produced = counter.load(Ordering::Acquire);
        if produced >= 10 && total_consumed >= 10 {
            break;
        }
        
        thread::sleep(Duration::from_millis(10));
    }
    
    producer1.join().unwrap();
    producer2.join().unwrap();
    
    let final_produced = counter.load(Ordering::Acquire);
    println!("Final results: produced={}, consumed={}", final_produced, total_consumed);
    
    if total_consumed == 10 {
        println!("SUCCESS: All events were consumed!");
    } else {
        println!("FAILURE: Not all events were consumed");
    }
}