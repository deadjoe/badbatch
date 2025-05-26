//! Progress Indicators
//!
//! Progress bars and spinners for CLI operations.

use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use std::fmt::Write;

/// Create a spinner for indeterminate progress
pub fn create_spinner(message: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    spinner
}

/// Create a progress bar for determinate progress
pub fn create_progress_bar(total: u64, message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new(total);
    progress_bar.set_style(
        ProgressStyle::with_template(
            "{msg} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})",
        )
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
        })
        .progress_chars("#>-"),
    );
    progress_bar.set_message(message.to_string());
    progress_bar
}

/// Create a progress bar with custom styling for data transfer
#[allow(dead_code)]
pub fn create_transfer_progress_bar(total: u64, message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new(total);
    progress_bar.set_style(
        ProgressStyle::with_template(
            "{msg} [{elapsed_precise}] [{wide_bar:.green/red}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})"
        )
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
        })
        .progress_chars("█▉▊▋▌▍▎▏  "),
    );
    progress_bar.set_message(message.to_string());
    progress_bar
}

/// Create a progress bar for batch operations
#[allow(dead_code)]
pub fn create_batch_progress_bar(total: u64, message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new(total);
    progress_bar.set_style(
        ProgressStyle::with_template(
            "{msg} [{elapsed_precise}] [{wide_bar:.yellow/blue}] {pos}/{len} batches ({per_sec}, {eta})"
        )
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
        })
        .progress_chars("██▉▊▋▌▍▎▏ "),
    );
    progress_bar.set_message(message.to_string());
    progress_bar
}

/// Create a simple progress bar without ETA
#[allow(dead_code)]
pub fn create_simple_progress_bar(total: u64, message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new(total);
    progress_bar.set_style(
        ProgressStyle::with_template(
            "{msg} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );
    progress_bar.set_message(message.to_string());
    progress_bar
}

/// Create a multi-progress container for multiple progress bars
#[allow(dead_code)]
pub fn create_multi_progress() -> indicatif::MultiProgress {
    indicatif::MultiProgress::new()
}

/// Finish a progress bar with a success message
pub fn finish_progress_with_message(progress_bar: &ProgressBar, message: &str) {
    progress_bar.set_style(ProgressStyle::with_template("{msg}").unwrap());
    progress_bar.finish_with_message(format!("✓ {}", message));
}

/// Finish a progress bar with an error message
#[allow(dead_code)]
pub fn finish_progress_with_error(progress_bar: &ProgressBar, message: &str) {
    progress_bar.set_style(ProgressStyle::with_template("{msg}").unwrap());
    progress_bar.finish_with_message(format!("✗ {}", message));
}

/// Create a progress bar for monitoring operations
#[allow(dead_code)]
pub fn create_monitor_progress_bar(message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new_spinner();
    progress_bar.set_style(
        ProgressStyle::with_template("{spinner:.blue} {msg} [{elapsed_precise}]")
            .unwrap()
            .tick_strings(&[
                "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂",
            ]),
    );
    progress_bar.set_message(message.to_string());
    progress_bar.enable_steady_tick(std::time::Duration::from_millis(200));
    progress_bar
}

/// Create a progress bar for server operations
#[allow(dead_code)]
pub fn create_server_progress_bar(message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new_spinner();
    progress_bar.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .unwrap()
            .tick_strings(&["🌍", "🌎", "🌏", "🌍", "🌎", "🌏"]),
    );
    progress_bar.set_message(message.to_string());
    progress_bar.enable_steady_tick(std::time::Duration::from_millis(500));
    progress_bar
}

/// Update progress bar message
#[allow(dead_code)]
pub fn update_progress_message(progress_bar: &ProgressBar, message: &str) {
    progress_bar.set_message(message.to_string());
}

/// Increment progress bar by one
#[allow(dead_code)]
pub fn increment_progress(progress_bar: &ProgressBar) {
    progress_bar.inc(1);
}

/// Set progress bar position
#[allow(dead_code)]
pub fn set_progress_position(progress_bar: &ProgressBar, position: u64) {
    progress_bar.set_position(position);
}

/// Clear and hide progress bar
#[allow(dead_code)]
pub fn clear_progress(progress_bar: &ProgressBar) {
    progress_bar.finish_and_clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_spinner() {
        let spinner = create_spinner("Testing...");
        assert!(!spinner.is_finished());
        spinner.finish_and_clear();
    }

    #[test]
    fn test_create_progress_bar() {
        let progress_bar = create_progress_bar(100, "Testing progress");
        assert_eq!(progress_bar.length(), Some(100));
        progress_bar.finish_and_clear();
    }

    #[test]
    fn test_progress_operations() {
        let progress_bar = create_simple_progress_bar(10, "Test");

        increment_progress(&progress_bar);
        assert_eq!(progress_bar.position(), 1);

        set_progress_position(&progress_bar, 5);
        assert_eq!(progress_bar.position(), 5);

        update_progress_message(&progress_bar, "Updated message");

        clear_progress(&progress_bar);
        assert!(progress_bar.is_finished());
    }

    #[test]
    fn test_finish_with_messages() {
        let progress_bar = create_simple_progress_bar(1, "Test");
        finish_progress_with_message(&progress_bar, "Success");
        assert!(progress_bar.is_finished());

        let progress_bar2 = create_simple_progress_bar(1, "Test");
        finish_progress_with_error(&progress_bar2, "Error");
        assert!(progress_bar2.is_finished());
    }

    #[test]
    fn test_specialized_progress_bars() {
        let transfer_bar = create_transfer_progress_bar(1000, "Transferring");
        assert_eq!(transfer_bar.length(), Some(1000));
        transfer_bar.finish_and_clear();

        let batch_bar = create_batch_progress_bar(50, "Processing batches");
        assert_eq!(batch_bar.length(), Some(50));
        batch_bar.finish_and_clear();

        let monitor_bar = create_monitor_progress_bar("Monitoring");
        assert!(!monitor_bar.is_finished());
        monitor_bar.finish_and_clear();

        let server_bar = create_server_progress_bar("Server starting");
        assert!(!server_bar.is_finished());
        server_bar.finish_and_clear();
    }

    #[test]
    fn test_multi_progress() {
        let multi = create_multi_progress();
        let bar1 = multi.add(create_progress_bar(100, "Task 1"));
        let bar2 = multi.add(create_progress_bar(200, "Task 2"));

        bar1.inc(10);
        bar2.inc(20);

        bar1.finish_and_clear();
        bar2.finish_and_clear();
    }
}
