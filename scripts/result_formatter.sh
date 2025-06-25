#!/bin/bash

# BadBatch Benchmark Result Formatter
# Functions to display friendly benchmark results

# Function to extract performance data from log file
extract_performance_data() {
    local log_file="$1"
    local benchmark_name="$2"
    
    if [ ! -f "$log_file" ]; then
        echo "❌ No log file found"
        return 1
    fi
    
    # Extract key metrics
    local throughput=$(grep -E "thrpt:" "$log_file" | head -1 | sed -E 's/.*\[([0-9.]+) [A-Za-z]+elem\/s.*/\1/' | head -1)
    local latency=$(grep -E "time:" "$log_file" | head -1 | sed -E 's/.*\[([0-9.]+) [a-z]+s.*/\1/' | head -1)
    local samples=$(grep -E "samples in estimated" "$log_file" | head -1 | sed -E 's/.*Collecting ([0-9]+) samples.*/\1/')
    local iterations=$(grep -E "samples in estimated" "$log_file" | head -1 | sed -E 's/.*\(([0-9.]+[KMG]?) iterations\).*/\1/')
    
    # Format and display results
    printf "│ %-25s │ %-12s │ %-10s │ %-8s │ %-12s │\n" \
        "$benchmark_name" \
        "${throughput:-N/A} Melem/s" \
        "${latency:-N/A}" \
        "${samples:-N/A}" \
        "${iterations:-N/A}"
}

# Function to display benchmark results table
display_results_table() {
    local log_dir="$1"
    shift
    local benchmark_names=("$@")
    
    echo ""
    echo "📊 Benchmark Results Summary"
    echo "═══════════════════════════════════════════════════════════════════════════════"
    printf "│ %-25s │ %-12s │ %-10s │ %-8s │ %-12s │\n" \
        "Benchmark" "Throughput" "Latency" "Samples" "Iterations"
    echo "├─────────────────────────────┼──────────────┼────────────┼──────────┼──────────────┤"
    
    for benchmark in "${benchmark_names[@]}"; do
        local log_file="$log_dir/${benchmark}.log"
        extract_performance_data "$log_file" "$benchmark"
    done
    
    echo "└─────────────────────────────┴──────────────┴────────────┴──────────┴──────────────┘"
}

# Function to display detailed results for single benchmark
display_single_benchmark_results() {
    local log_file="$1"
    local benchmark_name="$2"
    
    if [ ! -f "$log_file" ]; then
        echo "❌ No results available for $benchmark_name"
        return 1
    fi
    
    echo ""
    echo "🎯 Detailed Results for: $benchmark_name"
    echo "═══════════════════════════════════════════════════════════════════"
    
    # Extract all performance metrics
    echo "📈 Performance Metrics:"
    grep -E "(time:|thrpt:)" "$log_file" | head -5 | while read line; do
        if [[ $line == *"time:"* ]]; then
            echo "  ⏱️  $line"
        elif [[ $line == *"thrpt:"* ]]; then
            echo "  🚀 $line"
        fi
    done
    
    echo ""
    echo "📊 Statistical Analysis:"
    grep -E "(change:|Found.*outliers)" "$log_file" | head -3 | while read line; do
        if [[ $line == *"change:"* ]]; then
            echo "  📈 $line"
        elif [[ $line == *"outliers"* ]]; then
            echo "  📍 $line"
        fi
    done
    
    # Performance assessment
    echo ""
    echo "🔍 Performance Assessment:"
    local throughput=$(grep -E "thrpt:" "$log_file" | head -1 | sed -E 's/.*\[([0-9.]+) [A-Za-z]+elem\/s.*/\1/' | head -1)
    if [ -n "$throughput" ]; then
        local throughput_int=$(echo "$throughput" | cut -d. -f1)
        if [ "$throughput_int" -gt 10 ]; then
            echo "  🟢 Excellent throughput (${throughput} Melem/s)"
        elif [ "$throughput_int" -gt 5 ]; then
            echo "  🟡 Good throughput (${throughput} Melem/s)"
        elif [ "$throughput_int" -gt 1 ]; then
            echo "  🟠 Moderate throughput (${throughput} Melem/s)"
        else
            echo "  🔴 Low throughput (${throughput} Melem/s)"
        fi
    fi
    
    echo "═══════════════════════════════════════════════════════════════════"
}

# Function to generate comprehensive summary
generate_summary_report() {
    local log_dir="$1"
    local total_time="$2"
    shift 2
    local successful_benchmarks=("$@")
    
    echo ""
    echo "🏆 BadBatch Benchmark Summary Report"
    echo "═══════════════════════════════════════════════════════════════════════════════"
    echo "📅 Test Date: $(date '+%Y-%m-%d %H:%M:%S')"
    echo "⏱️  Total Time: ${total_time} seconds ($((total_time / 60)) minutes)"
    echo "✅ Successful Tests: ${#successful_benchmarks[@]}"
    echo ""
    
    # Performance highlights
    echo "🌟 Performance Highlights:"
    local best_throughput=0
    local best_benchmark=""
    
    for benchmark in "${successful_benchmarks[@]}"; do
        local log_file="$log_dir/${benchmark}.log"
        if [ -f "$log_file" ]; then
            local throughput=$(grep -E "thrpt:" "$log_file" | head -1 | sed -E 's/.*\[([0-9.]+) [A-Za-z]+elem\/s.*/\1/' | head -1)
            if [ -n "$throughput" ]; then
                local throughput_int=$(echo "$throughput" | cut -d. -f1)
                if [ "$throughput_int" -gt "$best_throughput" ]; then
                    best_throughput=$throughput_int
                    best_benchmark=$benchmark
                fi
            fi
        fi
    done
    
    if [ -n "$best_benchmark" ]; then
        echo "  🥇 Best Performance: $best_benchmark (${best_throughput}+ Melem/s)"
    fi
    
    # System info
    echo ""
    echo "💻 System Information:"
    echo "  🖥️  Platform: $(uname -s) $(uname -m)"
    echo "  🧠 CPU Cores: $(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 'Unknown')"
    echo "  📁 Log Directory: $log_dir"
    
    echo ""
    echo "📋 Next Steps:"
    echo "  • Review detailed logs in: $log_dir/"
    echo "  • Generate HTML reports: ./scripts/run_benchmarks.sh report"
    echo "  • Run regression tests: ./scripts/run_benchmarks.sh regression"
    
    echo "═══════════════════════════════════════════════════════════════════════════════"
}