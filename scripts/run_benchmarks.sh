#!/bin/bash

# BadBatch Disruptor Benchmark Runner
# This script provides convenient ways to run different benchmark suites

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Function to check if criterion is installed
check_dependencies() {
    print_status "Checking dependencies..."
    
    # Check if cargo-criterion is installed (optional but recommended)
    if ! command -v cargo-criterion &> /dev/null; then
        print_warning "cargo-criterion not found. Installing for better benchmark reports..."
        cargo install cargo-criterion || print_warning "Failed to install cargo-criterion, using built-in criterion"
    fi
}

# Function to run quick benchmarks (for CI/development)
run_quick() {
    print_status "Running quick benchmark suite..."
    print_status "This takes about 2-5 minutes and tests basic functionality."
    
    time cargo bench --bench comprehensive_benchmarks
}

# Function to run SPSC benchmarks
run_spsc() {
    print_status "Running Single Producer Single Consumer (SPSC) benchmarks..."
    print_status "This tests different wait strategies with single producer/consumer setup."
    
    time cargo bench --bench single_producer_single_consumer
}

# Function to run MPSC benchmarks
run_mpsc() {
    print_status "Running Multi Producer Single Consumer (MPSC) benchmarks..."
    print_status "This tests multi-producer coordination and scalability."
    
    time cargo bench --bench multi_producer_single_consumer
}

# Function to run pipeline benchmarks
run_pipeline() {
    print_status "Running Pipeline Processing benchmarks..."
    print_status "This tests complex event processing pipelines with dependencies."
    
    time cargo bench --bench pipeline_processing
}

# Function to run latency comparison
run_latency() {
    print_status "Running Latency Comparison benchmarks..."
    print_status "This compares latency against other concurrency primitives."
    
    time cargo bench --bench latency_comparison
}

# Function to run throughput comparison
run_throughput() {
    print_status "Running Throughput Comparison benchmarks..."
    print_status "This compares raw throughput against other implementations."
    
    time cargo bench --bench throughput_comparison
}

# Function to run buffer scaling benchmarks
run_scaling() {
    print_status "Running Buffer Size Scaling benchmarks..."
    print_status "This tests how performance scales with different buffer sizes."
    
    time cargo bench --bench buffer_size_scaling
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
    
    print_status "Starting comprehensive benchmark run..."
    
    run_spsc
    run_mpsc
    run_pipeline
    run_latency
    run_throughput
    run_scaling
    
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    print_status "All benchmarks completed!"
    print_status "Total time: ${duration} seconds ($((duration / 60)) minutes)"
}

# Function to generate comparison report
generate_report() {
    print_status "Generating benchmark comparison report..."
    
    # Create reports directory if it doesn't exist
    mkdir -p target/criterion/reports
    
    print_status "Running benchmarks with HTML report generation..."
    cargo bench -- --output-format html
    
    print_status "HTML reports generated in: target/criterion/"
    print_status "Open target/criterion/reports/index.html in your browser to view results."
}

# Function to run performance regression test
run_regression() {
    print_status "Running performance regression test..."
    print_warning "This compares current performance against previous benchmark results."
    
    # Run a focused set of benchmarks for regression testing
    cargo bench --bench comprehensive_benchmarks
    cargo bench --bench single_producer_single_consumer -- "BusySpin_burst:100_pause:0ms"
    cargo bench --bench throughput_comparison -- "Disruptor.*buf1024"
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

# Function to show help
show_help() {
    echo "BadBatch Disruptor Benchmark Runner"
    echo ""
    echo "Usage: $0 [COMMAND]"
    echo ""
    echo "Commands:"
    echo "  quick      Run quick benchmark suite (2-5 minutes)"
    echo "  spsc       Run Single Producer Single Consumer benchmarks"
    echo "  mpsc       Run Multi Producer Single Consumer benchmarks"
    echo "  pipeline   Run Pipeline Processing benchmarks"
    echo "  latency    Run Latency Comparison benchmarks"
    echo "  throughput Run Throughput Comparison benchmarks"
    echo "  scaling    Run Buffer Size Scaling benchmarks"
    echo "  all        Run ALL benchmark suites (30-60 minutes)"
    echo "  report     Generate HTML benchmark reports"
    echo "  regression Run performance regression test"
    echo "  optimize   Optimize system for benchmarking (requires sudo)"
    echo "  help       Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 quick                    # Quick development test"
    echo "  $0 spsc                     # Test SPSC performance"
    echo "  $0 all                      # Full benchmark suite"
    echo "  $0 optimize && $0 all       # Optimize system then run all tests"
    echo ""
    echo "Environment Variables:"
    echo "  CARGO_BENCH_ARGS           # Additional arguments to pass to cargo bench"
    echo "  CRITERION_ARGS             # Additional arguments to pass to criterion"
    echo ""
    echo "Notes:"
    echo "  - For consistent results, close other applications"
    echo "  - Run on AC power if using a laptop"
    echo "  - Consider using 'optimize' command for best results"
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