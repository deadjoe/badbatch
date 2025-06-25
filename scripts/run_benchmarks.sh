#!/bin/bash

# BadBatch Disruptor Enhanced Benchmark Runner
# This script provides convenient ways to run different benchmark suites with safety features
# Enhanced version combining comprehensive functionality with timeout protection

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
TIMEOUT_SECONDS=600  # 10 minutes timeout per benchmark (longer for comprehensive tests)
SPSC_TIMEOUT=900     # 15 minutes for SPSC (longer due to many wait strategies)
SCALING_TIMEOUT=900  # 15 minutes for buffer scaling (tests multiple sizes)
LOG_DIR="benchmark_logs"

# Function to print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Create log directory
mkdir -p "$LOG_DIR"

# Load result formatter functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/result_formatter.sh"

# Function to check if criterion is installed
check_dependencies() {
    print_status "Checking dependencies..."
    
    # Check if cargo-criterion is installed (optional but recommended)
    if ! command -v cargo-criterion &> /dev/null; then
        print_warning "cargo-criterion not found. You can install it with: cargo install cargo-criterion"
    fi
}

# Function to run benchmark with timeout protection
run_benchmark_safe() {
    local bench_name="$1"
    local description="$2"
    local timeout_override="$3"  # Optional override timeout
    local log_file="$LOG_DIR/${bench_name}.log"
    
    print_status "Running $description..."
    print_status "Benchmark: $bench_name"
    
    # Determine timeout to use
    local timeout_value="$TIMEOUT_SECONDS"
    if [ -n "$timeout_override" ]; then
        timeout_value="$timeout_override"
        print_status "Using extended timeout: ${timeout_value}s"
    fi
    
    # Use timeout if available (GNU timeout or gtimeout on macOS)
    local timeout_cmd=""
    if command -v timeout >/dev/null 2>&1; then
        timeout_cmd="timeout ${timeout_value}s"
    elif command -v gtimeout >/dev/null 2>&1; then
        timeout_cmd="gtimeout ${timeout_value}s"
    else
        print_warning "No timeout command available, running without timeout protection"
    fi
    
    # Run the benchmark
    local start_time=$(date +%s)
    if [ -n "$timeout_cmd" ]; then
        if $timeout_cmd cargo bench --bench "$bench_name" > "$log_file" 2>&1; then
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            print_success "$bench_name completed successfully in ${duration}s"
            
            # Display friendly results
            display_single_benchmark_results "$log_file" "$bench_name"
            return 0
        else
            local exit_code=$?
            if [ $exit_code -eq 124 ]; then
                print_error "$bench_name timed out after ${timeout_value} seconds"
            else
                print_error "$bench_name failed with exit code $exit_code"
            fi
            print_warning "Check log file: $log_file"
            return $exit_code
        fi
    else
        # Run without timeout as fallback
        if time cargo bench --bench "$bench_name" 2>&1 | tee "$log_file"; then
            print_success "$bench_name completed successfully"
            
            # Display friendly results
            display_single_benchmark_results "$log_file" "$bench_name"
            return 0
        else
            print_error "$bench_name failed"
            return 1
        fi
    fi
}

# Function to run quick benchmarks (for CI/development)
run_quick() {
    print_status "Running quick benchmark suite..."
    print_status "This takes about 2-5 minutes and tests basic functionality."
    
    run_benchmark_safe "comprehensive_benchmarks" "Quick comprehensive test suite"
}

# Function to run SPSC benchmarks
run_spsc() {
    print_status "Running Single Producer Single Consumer (SPSC) benchmarks..."
    print_status "This tests different wait strategies with single producer/consumer setup."
    
    run_benchmark_safe "single_producer_single_consumer" "SPSC performance testing" "$SPSC_TIMEOUT"
}

# Function to run MPSC benchmarks
run_mpsc() {
    print_status "Running Multi Producer Single Consumer (MPSC) benchmarks..."
    print_status "This tests multi-producer coordination and scalability."
    
    run_benchmark_safe "multi_producer_single_consumer" "MPSC performance testing"
}

# Function to run pipeline benchmarks
run_pipeline() {
    print_status "Running Pipeline Processing benchmarks..."
    print_status "This tests complex event processing pipelines with dependencies."
    
    run_benchmark_safe "pipeline_processing" "Pipeline processing performance"
}

# Function to run latency comparison
run_latency() {
    print_status "Running Latency Comparison benchmarks..."
    print_status "This compares latency against other concurrency primitives."
    
    run_benchmark_safe "latency_comparison" "Latency comparison analysis"
}

# Function to run throughput comparison
run_throughput() {
    print_status "Running Throughput Comparison benchmarks..."
    print_status "This compares raw throughput against other implementations."
    
    run_benchmark_safe "throughput_comparison" "Throughput comparison analysis"
}

# Function to run buffer scaling benchmarks
run_scaling() {
    print_status "Running Buffer Size Scaling benchmarks..."
    print_status "This tests how performance scales with different buffer sizes."
    
    run_benchmark_safe "buffer_size_scaling" "Buffer size scaling analysis" "$SCALING_TIMEOUT"
}

# Function to run minimal test
run_minimal() {
    print_status "Running Minimal Test..."
    print_status "Quick debugging and hang detection test."
    
    run_benchmark_safe "minimal_test" "Minimal functionality test"
}

# Function to run all benchmarks
run_all() {
    print_status "Running ALL benchmark suites..."
    print_warning "This will take 30-60 minutes to complete!"
    read -p "Are you sure you want to continue? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        print_status "Cancelled."
        exit 0
    fi
    
    local start_time=$(date +%s)
    local failed_benchmarks=()
    local successful_benchmarks=()
    
    print_status "Starting comprehensive benchmark run..."
    
    # Run all benchmarks and track results
    for benchmark in quick spsc mpsc pipeline latency throughput scaling minimal; do
        echo ""
        print_status "=== Running $benchmark benchmark ==="
        if run_$benchmark; then
            successful_benchmarks+=("$benchmark")
        else
            failed_benchmarks+=("$benchmark")
        fi
    done
    
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    echo ""
    print_status "Benchmark run completed!"
    
    # Display comprehensive results table
    if [ ${#successful_benchmarks[@]} -gt 0 ]; then
        display_results_table "$LOG_DIR" "${successful_benchmarks[@]}"
    fi
    
    # Display summary report
    generate_summary_report "$LOG_DIR" "$duration" "${successful_benchmarks[@]}"
    
    # Basic summary
    if [ ${#failed_benchmarks[@]} -gt 0 ]; then
        echo ""
        print_error "Failed benchmarks (${#failed_benchmarks[@]}):"
        for bench in "${failed_benchmarks[@]}"; do
            echo "  ✗ $bench"
        done
        return 1
    else
        echo ""
        print_success "🎉 All benchmarks completed successfully!"
        return 0
    fi
}

# Function to generate comparison report
generate_report() {
    print_status "Generating benchmark comparison report..."
    
    # Create reports directory if it doesn't exist
    mkdir -p target/criterion/reports
    
    print_status "Running benchmarks with HTML report generation..."
    run_benchmark_safe "comprehensive_benchmarks" "Report generation benchmark"
    
    print_status "HTML reports generated in: target/criterion/"
    print_status "Open target/criterion/reports/index.html in your browser to view results."
}

# Function to run performance regression test
run_regression() {
    print_status "Running performance regression test..."
    print_warning "This compares current performance against previous benchmark results."
    
    # Run a focused set of benchmarks for regression testing
    run_benchmark_safe "comprehensive_benchmarks" "Regression test - comprehensive"
    run_benchmark_safe "minimal_test" "Regression test - minimal"
}

# Function to optimize system for benchmarking
optimize_system() {
    print_status "Applying system optimizations for benchmarking..."
    print_warning "This requires sudo access and modifies system settings."
    
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        print_status "Detected Linux - applying CPU governor optimizations..."
        
        # Set CPU frequency scaling governor to performance
        if command -v cpupower &> /dev/null; then
            sudo cpupower frequency-set -g performance
            print_status "CPU governor set to performance mode"
        else
            print_warning "cpupower not found, skipping CPU governor optimization"
        fi
        
        # Disable CPU frequency scaling (Intel)
        if [ -f /sys/devices/system/cpu/intel_pstate/no_turbo ]; then
            echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo > /dev/null
            print_status "Intel turbo boost disabled"
        fi
        
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        print_status "Detected macOS - limited optimizations available..."
        print_warning "Consider closing other applications and running on AC power"
        
    else
        print_warning "Unknown OS type, skipping system optimizations"
    fi
    
    print_status "System optimization complete"
}

# Function to compile all benchmarks without running
compile_all() {
    print_status "Compiling all benchmarks..."
    
    local benchmarks=("comprehensive_benchmarks" "single_producer_single_consumer" 
                     "multi_producer_single_consumer" "pipeline_processing" 
                     "latency_comparison" "throughput_comparison" 
                     "buffer_size_scaling" "minimal_test")
    
    local failed=0
    for benchmark in "${benchmarks[@]}"; do
        print_status "Compiling $benchmark..."
        if cargo bench --bench "$benchmark" --no-run > "$LOG_DIR/${benchmark}_compile.log" 2>&1; then
            print_success "$benchmark compiled successfully"
        else
            print_error "$benchmark compilation failed"
            failed=$((failed + 1))
        fi
    done
    
    if [ $failed -eq 0 ]; then
        print_success "All benchmarks compiled successfully!"
    else
        print_error "$failed benchmark(s) failed to compile"
        return 1
    fi
}

# Function to show help
show_help() {
    echo "BadBatch Disruptor Enhanced Benchmark Runner"
    echo "Enhanced with timeout protection and comprehensive error handling"
    echo ""
    echo "Usage: $0 [COMMAND]"
    echo ""
    echo "Commands:"
    echo "  quick      Run quick benchmark suite (2-5 minutes) - RECOMMENDED for CI"
    echo "  spsc       Run Single Producer Single Consumer benchmarks"
    echo "  mpsc       Run Multi Producer Single Consumer benchmarks"
    echo "  pipeline   Run Pipeline Processing benchmarks"
    echo "  latency    Run Latency Comparison benchmarks"
    echo "  throughput Run Throughput Comparison benchmarks"
    echo "  scaling    Run Buffer Size Scaling benchmarks"
    echo "  minimal    Run Minimal Test (quick debugging)"
    echo "  all        Run ALL benchmark suites (30-60 minutes)"
    echo "  report     Generate HTML benchmark reports"
    echo "  regression Run performance regression test"
    echo "  compile    Compile all benchmarks without running"
    echo "  optimize   Optimize system for benchmarking (requires sudo)"
    echo "  help       Show this help message"
    echo ""
    echo "Safety Features:"
    echo "  - Timeout protection (${TIMEOUT_SECONDS}s per benchmark)"
    echo "  - Error recovery and isolation"
    echo "  - Comprehensive logging to $LOG_DIR/"
    echo "  - Progress monitoring and reporting"
    echo ""
    echo "Examples:"
    echo "  $0 quick                    # Quick development test (recommended)"
    echo "  $0 minimal                  # Fastest debugging test"
    echo "  $0 spsc                     # Test SPSC performance"
    echo "  $0 compile                  # Check if all benchmarks compile"
    echo "  $0 optimize && $0 all       # Optimize system then run all tests"
    echo ""
    echo "Environment Variables:"
    echo "  TIMEOUT_SECONDS            # Override timeout (default: $TIMEOUT_SECONDS)"
    echo "  LOG_DIR                    # Override log directory (default: $LOG_DIR)"
    echo ""
    echo "Notes:"
    echo "  - All benchmarks include timeout protection to prevent hanging"
    echo "  - For consistent results, close other applications"
    echo "  - Run on AC power if using a laptop"
    echo "  - Consider using 'optimize' command for best results"
    echo "  - Check $LOG_DIR/ for detailed benchmark logs"
}

# Main script logic
main() {
    case "${1:-help}" in
        quick)
            check_dependencies
            run_quick
            ;;
        spsc)
            check_dependencies
            run_spsc
            ;;
        mpsc)
            check_dependencies
            run_mpsc
            ;;
        pipeline)
            check_dependencies
            run_pipeline
            ;;
        latency)
            check_dependencies
            run_latency
            ;;
        throughput)
            check_dependencies
            run_throughput
            ;;
        scaling)
            check_dependencies
            run_scaling
            ;;
        minimal)
            check_dependencies
            run_minimal
            ;;
        all)
            check_dependencies
            run_all
            ;;
        report)
            check_dependencies
            generate_report
            ;;
        regression)
            check_dependencies
            run_regression
            ;;
        compile)
            compile_all
            ;;
        optimize)
            optimize_system
            ;;
        help|--help|-h)
            show_help
            ;;
        *)
            print_error "Unknown command: $1"
            echo ""
            show_help
            exit 1
            ;;
    esac
}

# Run main function with all arguments
main "$@"