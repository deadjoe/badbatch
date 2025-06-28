#!/bin/bash

# BadBatch Benchmark Result Formatter
# Functions to display friendly benchmark results

# Function to extract performance data from log file
extract_performance_data() {
    local log_file="$1"
    local benchmark_name="$2"
    
    if [ ! -f "$log_file" ]; then
        echo "❌ No log file found for $benchmark_name"
        return 1
    fi
    
    # Extract key metrics with better regex patterns
    local throughput_line=$(grep -E "thrpt:" "$log_file" | head -1)
    local throughput=""
    local throughput_unit=""
    
    # Extract throughput value and unit separately
    if [ -n "$throughput_line" ]; then
        throughput=$(echo "$throughput_line" | sed -E 's/.*\[([0-9.]+) [A-Za-z]*elem\/s.*/\1/')
        throughput_unit=$(echo "$throughput_line" | sed -E 's/.*\[([0-9.]+) ([A-Za-z]*)elem\/s.*/\2elem\/s/')
        # Handle case where no unit prefix is present
        if [ "$throughput_unit" = "elem/s" ]; then
            throughput_unit="elem/s"
        fi
    fi
    
    local latency_line=$(grep -E "time:" "$log_file" | head -1)
    local latency=""
    local latency_unit=""
    
    # Extract latency value and unit more precisely
    if echo "$latency_line" | grep -q "ns"; then
        latency=$(echo "$latency_line" | sed -E 's/.*\[([0-9.]+) ns.*/\1/')
        latency_unit="ns"
    elif echo "$latency_line" | grep -q "µs"; then
        latency=$(echo "$latency_line" | sed -E 's/.*\[([0-9.]+) µs.*/\1/')
        latency_unit="µs"
    elif echo "$latency_line" | grep -q "ms"; then
        latency=$(echo "$latency_line" | sed -E 's/.*\[([0-9.]+) ms.*/\1/')
        latency_unit="ms"
    elif echo "$latency_line" | grep -q "ps"; then
        # Handle picosecond case - preserve precision
        latency=$(echo "$latency_line" | sed -E 's/.*\[([0-9.]+) ps.*/\1/')
        latency_unit="ps"
    fi
    
    local samples=$(grep -E "Collecting [0-9]+ samples" "$log_file" | head -1 | sed -E 's/.*Collecting ([0-9]+) samples.*/\1/')
    local iterations_line=$(grep -E "samples in estimated.*iterations" "$log_file" | head -1)
    local iterations=""
    
    # Improved iterations parsing with better error handling
    if [ -n "$iterations_line" ]; then
        # Try multiple patterns to extract iterations
        iterations=$(echo "$iterations_line" | sed -E 's/.*\(([0-9.]+[KMGTB]?) iterations\).*/\1/' 2>/dev/null)
        
        # If first pattern failed, try more flexible patterns
        if [ "$iterations" = "$iterations_line" ] || [ -z "$iterations" ]; then
            iterations=$(echo "$iterations_line" | sed -E 's/.*\(([0-9.]+[KMGTBkmgtb]*) iterations\).*/\1/' 2>/dev/null)
        fi
        
        # If still no match, try extracting just the number part
        if [ "$iterations" = "$iterations_line" ] || [ -z "$iterations" ]; then
            iterations=$(echo "$iterations_line" | grep -oE '[0-9.]+[KMGTBkmgtb]*' | head -1)
        fi
        
        # If all else fails, mark as unknown
        if [ -z "$iterations" ] || [ "$iterations" = "$iterations_line" ]; then
            iterations="Unknown"
        fi
    else
        iterations="N/A"
    fi
    
    # Clean up benchmark name for display
    local display_name=""
    case "$benchmark_name" in
        "comprehensive_benchmarks") display_name="Quick Test Suite" ;;
        "single_producer_single_consumer") display_name="SPSC" ;;
        "multi_producer_single_consumer") display_name="MPSC" ;;
        "pipeline_processing") display_name="Pipeline" ;;
        "latency_comparison") display_name="Latency" ;;
        "throughput_comparison") display_name="Throughput" ;;
        "buffer_size_scaling") display_name="Buffer Scaling" ;;
        *) display_name="$benchmark_name" ;;
    esac
    
    # Return structured data
    echo "$display_name|${throughput:-N/A}|${throughput_unit:-elem/s}|${latency:-N/A}|${latency_unit:-}|${samples:-N/A}|${iterations:-N/A}"
}

# Function to display benchmark results in clean list format
display_results_table() {
    local log_dir="$1"
    shift
    local benchmark_names=("$@")
    
    echo ""
    echo "📊 Benchmark Results Summary"
    echo "═══════════════════════════════════════════════════════════════════════════════"
    
    local results_found=false
    for benchmark in "${benchmark_names[@]}"; do
        local log_file="$log_dir/${benchmark}.log"
        if [ -f "$log_file" ]; then
            results_found=true
            local result_data=$(extract_performance_data "$log_file" "$benchmark")
            if [ $? -eq 0 ]; then
                # Parse the structured data
                local display_name=$(echo "$result_data" | cut -d'|' -f1)
                local throughput=$(echo "$result_data" | cut -d'|' -f2)
                local throughput_unit=$(echo "$result_data" | cut -d'|' -f3)
                local latency=$(echo "$result_data" | cut -d'|' -f4)
                local latency_unit=$(echo "$result_data" | cut -d'|' -f5)
                local samples=$(echo "$result_data" | cut -d'|' -f6)
                local iterations=$(echo "$result_data" | cut -d'|' -f7)
                
                echo ""
                echo "🎯 $display_name"
                echo "   📈 Throughput: ${throughput} ${throughput_unit}"
                if [ "$latency" != "N/A" ] && [ -n "$latency_unit" ]; then
                    echo "   ⏱️  Latency: ${latency} ${latency_unit}"
                fi
                echo "   🔬 Samples: ${samples}"
                echo "   🔄 Iterations: ${iterations}"
            fi
        fi
    done
    
    if [ "$results_found" = false ]; then
        echo ""
        echo "❌ No benchmark results found in $log_dir/"
        echo "   Make sure benchmarks have been run successfully."
    fi
    
    echo ""
    echo "═══════════════════════════════════════════════════════════════════════════════"
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
    local throughput_line=$(grep -E "thrpt:" "$log_file" | head -1)
    if [ -n "$throughput_line" ]; then
        local throughput=$(echo "$throughput_line" | sed -E 's/.*\[([0-9.]+) [A-Za-z]*elem\/s.*/\1/')
        local throughput_unit=$(echo "$throughput_line" | sed -E 's/.*\[([0-9.]+) ([A-Za-z]*)elem\/s.*/\2elem\/s/')
        if [ "$throughput_unit" = "elem/s" ]; then
            throughput_unit="elem/s"
        fi
        
        local throughput_int=$(echo "$throughput" | cut -d. -f1)
        # Adjust thresholds based on unit
        local threshold_multiplier=1
        case "$throughput_unit" in
            "Melem/s") threshold_multiplier=1 ;;
            "Kelem/s") threshold_multiplier=1000 ;;
            "elem/s") threshold_multiplier=1000000 ;;
        esac
        
        local effective_throughput=$((throughput_int * threshold_multiplier))
        if [ "$effective_throughput" -gt 10000000 ]; then
            echo "  🟢 Excellent throughput (${throughput} ${throughput_unit})"
        elif [ "$effective_throughput" -gt 5000000 ]; then
            echo "  🟡 Good throughput (${throughput} ${throughput_unit})"
        elif [ "$effective_throughput" -gt 1000000 ]; then
            echo "  🟠 Moderate throughput (${throughput} ${throughput_unit})"
        else
            echo "  🔴 Low throughput (${throughput} ${throughput_unit})"
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
            local throughput_line=$(grep -E "thrpt:" "$log_file" | head -1)
            if [ -n "$throughput_line" ]; then
                local throughput=$(echo "$throughput_line" | sed -E 's/.*\[([0-9.]+) [A-Za-z]*elem\/s.*/\1/')
                local throughput_unit=$(echo "$throughput_line" | sed -E 's/.*\[([0-9.]+) ([A-Za-z]*)elem\/s.*/\2elem\/s/')
                if [ "$throughput_unit" = "elem/s" ]; then
                    throughput_unit="elem/s"
                fi
                
                local throughput_int=$(echo "$throughput" | cut -d. -f1)
                # Convert to common base unit for comparison
                local normalized_throughput=$throughput_int
                case "$throughput_unit" in
                    "Melem/s") normalized_throughput=$((throughput_int * 1000000)) ;;
                    "Kelem/s") normalized_throughput=$((throughput_int * 1000)) ;;
                    "elem/s") normalized_throughput=$throughput_int ;;
                esac
                
                if [ "$normalized_throughput" -gt "$best_throughput" ]; then
                    best_throughput=$normalized_throughput
                    best_benchmark="$benchmark (${throughput} ${throughput_unit})"
                fi
            fi
        fi
    done
    
    if [ -n "$best_benchmark" ]; then
        echo "  🥇 Best Performance: $best_benchmark"
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