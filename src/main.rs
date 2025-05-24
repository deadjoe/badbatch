//! BadBatch - High-Performance Disruptor Engine
//!
//! A complete Rust implementation of the LMAX Disruptor pattern for ultra-low latency
//! inter-thread messaging with mechanical sympathy.
//!
//! This implementation strictly follows the original LMAX Disruptor design:
//! https://github.com/LMAX-Exchange/disruptor

use clap::{Arg, Command};
use std::process;

mod disruptor;

fn main() {
    let matches = Command::new("BadBatch")
        .version("1.0.0")
        .author("BadBatch Team")
        .about("High-Performance Disruptor Engine")
        .arg(
            Arg::new("buffer-size")
                .long("buffer-size")
                .value_name("SIZE")
                .help("Ring buffer size (must be power of 2)")
                .default_value("1024"),
        )
        .arg(
            Arg::new("producer-type")
                .long("producer-type")
                .value_name("TYPE")
                .help("Producer type: single or multi")
                .default_value("single"),
        )
        .arg(
            Arg::new("wait-strategy")
                .long("wait-strategy")
                .value_name("STRATEGY")
                .help("Wait strategy: blocking, yielding, busy-spin, sleeping")
                .default_value("blocking"),
        )
        .get_matches();

    let buffer_size: usize = matches
        .get_one::<String>("buffer-size")
        .unwrap()
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("Error: Invalid buffer size");
            process::exit(1);
        });

    if !is_power_of_two(buffer_size) {
        eprintln!("Error: Buffer size must be a power of 2");
        process::exit(1);
    }

    println!("BadBatch Disruptor Engine");
    println!("========================");
    println!("Buffer Size: {}", buffer_size);
    println!("Producer Type: {}", matches.get_one::<String>("producer-type").unwrap());
    println!("Wait Strategy: {}", matches.get_one::<String>("wait-strategy").unwrap());
    println!();
    println!("Starting Disruptor engine...");

    // TODO: Initialize and run the Disruptor
    println!("Disruptor engine ready!");
}

/// Check if a number is a power of 2
fn is_power_of_two(n: usize) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_power_of_two() {
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(2));
        assert!(is_power_of_two(4));
        assert!(is_power_of_two(8));
        assert!(is_power_of_two(1024));

        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(3));
        assert!(!is_power_of_two(5));
        assert!(!is_power_of_two(1023));
    }
}
