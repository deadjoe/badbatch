#!/bin/bash

# BadBatch Benchmark Result Formatter
# Functions to display friendly benchmark results

# Extract the mid estimate from a Criterion bracket:
#   [low unit mid unit high unit]  ->  "mid|unit"
extract_mid_estimate() {
    local line="$1"
    local bracket
    bracket="$(echo "$line" | sed -nE 's/.*\\[([^]]+)\\].*/\\1/p')"
    if [ -z "$bracket" ]; then
        echo "|"
        return 0
    fi

    # shellcheck disable=SC2206
    local parts=($bracket)
    if [ "${#parts[@]}" -lt 4 ]; then
        echo "|"
        return 0
    fi

    # parts: low unit mid unit high unit
    echo "${parts[2]}|${parts[3]}"
}

# Parse benchmark result blocks from a Criterion log into rows:
#   name<TAB>time_mid<TAB>time_unit<TAB>thrpt_mid<TAB>thrpt_unit
parse_criterion_results() {
    local log_file="$1"

    awk '
    function mid_estimate(line,  inside, n, a) {
        # Use only POSIX awk features (macOS awk does not support match(..., ..., array)).
        if (match(line, /\[[^]]+\]/) == 0) return "";
        inside = substr(line, RSTART + 1, RLENGTH - 2);
        n = split(inside, a, /[[:space:]]+/);
        if (n < 4) return "";
        return a[3] "\t" a[4];
    }

    function flush_pending() {
        if (current != "" && have_time == 1) {
            # time-only entry (no throughput line)
            print current "\t" time_mid "\t" time_unit "\t" "\t";
        }
        current = "";
        have_time = 0;
        time_mid = "";
        time_unit = "";
    }

    BEGIN {
        current = "";
        have_time = 0;
        time_mid = "";
        time_unit = "";
    }

    # Full line: "<name> time: [..]"
    /^[^[:space:]]+.*time:[[:space:]]*\[/ {
        flush_pending();
        current = $1;
        split(mid_estimate($0), t, "\t");
        time_mid = t[1];
        time_unit = t[2];
        have_time = 1;
        next;
    }

    # Standalone benchmark name line (followed by indented time/thrpt).
    /^[^[:space:]]+$/ {
        # Skip obvious non-benchmark tokens
        if ($1 == "Benchmarking" || $1 == "Found" || $1 == "Warning:" || $1 == "Disruptor") next;
        flush_pending();
        current = $0;
        next;
    }

    # Indented "time:" line
    /^[[:space:]]+time:[[:space:]]*\[/ {
        if (current == "") next;
        split(mid_estimate($0), t, "\t");
        time_mid = t[1];
        time_unit = t[2];
        have_time = 1;
        next;
    }

    # Indented "thrpt:" line
    /^[[:space:]]+thrpt:[[:space:]]*\[/ {
        if (current == "" || have_time == 0) next;
        split(mid_estimate($0), r, "\t");
        print current "\t" time_mid "\t" time_unit "\t" r[1] "\t" r[2];
        current = "";
        have_time = 0;
        time_mid = "";
        time_unit = "";
        next;
    }

    END {
        flush_pending();
    }
    ' "$log_file"
}

# Pick the peak non-baseline result row by throughput when available.
# Falls back to the first non-baseline row for time-only results, else first result.
pick_primary_result() {
    local log_file="$1"
    local row

    row="$(parse_criterion_results "$log_file" | awk -F'\t' '
        function unit_scale(unit) {
            if (unit == "Gelem/s") return 1000000000;
            if (unit == "Melem/s") return 1000000;
            if (unit == "Kelem/s") return 1000;
            if (unit == "elem/s") return 1;
            return 0;
        }

        function is_zero_pause(name) {
            return name ~ /pause:0(ms)?/;
        }

        $1 ~ /baseline/ { next }

        {
            if (is_zero_pause($1)) {
                zero_pause_seen = 1;
            }

            rows[++row_count] = $0;
        }

        END {
            for (i = 1; i <= row_count; i++) {
                split(rows[i], fields, "\t");
                name = fields[1];
                throughput = fields[4];
                unit = fields[5];

                if (zero_pause_seen && !is_zero_pause(name)) {
                    continue;
                }

                if (throughput != "" && unit != "") {
                    normalized = (throughput + 0) * unit_scale(unit);
                if (!have_throughput || normalized > best) {
                    best = normalized;
                        best_row = rows[i];
                    have_throughput = 1;
                }
                } else if (!have_throughput && best_row == "") {
                    best_row = rows[i];
                }
            }
            if (best_row != "") {
                print best_row;
            }
        }
    ')"
    if [ -z "$row" ]; then
        row="$(parse_criterion_results "$log_file" | head -n 1)"
    fi

    echo "$row"
}

# Function to extract performance data from log file
extract_performance_data() {
    local log_file="$1"
    local benchmark_name="$2"
    
    if [ ! -f "$log_file" ]; then
        echo "❌ No log file found for $benchmark_name"
        return 1
    fi
    
    local primary_row
    primary_row="$(pick_primary_result "$log_file")"

    local primary_case throughput throughput_unit time_mid time_unit
    primary_case="$(echo "$primary_row" | cut -f1)"
    time_mid="$(echo "$primary_row" | cut -f2)"
    time_unit="$(echo "$primary_row" | cut -f3)"
    throughput="$(echo "$primary_row" | cut -f4)"
    throughput_unit="$(echo "$primary_row" | cut -f5)"

    if [ -z "$throughput" ]; then
        throughput="N/A"
        throughput_unit=""
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
    
    local warning_count
    warning_count=$(grep -c "^WARNING:" "$log_file" 2>/dev/null || true)
    warning_count=${warning_count:-0}

    # Return structured data
    echo "$display_name|${primary_case:-N/A}|${throughput:-N/A}|${throughput_unit:-}|${time_mid:-N/A}|${time_unit:-}|${samples:-N/A}|${iterations:-N/A}|${warning_count}"
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
                local primary_case=$(echo "$result_data" | cut -d'|' -f2)
                local throughput=$(echo "$result_data" | cut -d'|' -f3)
                local throughput_unit=$(echo "$result_data" | cut -d'|' -f4)
                local time_mid=$(echo "$result_data" | cut -d'|' -f5)
                local time_unit=$(echo "$result_data" | cut -d'|' -f6)
                local samples=$(echo "$result_data" | cut -d'|' -f7)
                local iterations=$(echo "$result_data" | cut -d'|' -f8)
                local warning_count=$(echo "$result_data" | cut -d'|' -f9)
                
                echo ""
                echo "🎯 $display_name"
                if [ -n "$primary_case" ] && [ "$primary_case" != "N/A" ]; then
                    echo "   🧪 Peak Case: ${primary_case}"
                fi
                if [ "$throughput" != "N/A" ] && [ -n "$throughput_unit" ]; then
                    echo "   📈 Peak Throughput: ${throughput} ${throughput_unit}"
                fi
                if [ "$time_mid" != "N/A" ] && [ -n "$time_unit" ]; then
                    echo "   ⏱️  Time: ${time_mid} ${time_unit}"
                fi
                echo "   🔬 Samples: ${samples}"
                echo "   🔄 Iterations: ${iterations}"
                if [ "$warning_count" -gt 0 ]; then
                    echo "   ⚠️  Warnings: ${warning_count}"
                fi
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
    
    local warning_count
    warning_count=$(grep -c "^WARNING:" "$log_file" 2>/dev/null || true)
    warning_count=${warning_count:-0}
    if [ "$warning_count" -gt 0 ]; then
        echo "⚠️  Warnings in log: ${warning_count}"
        grep -E "^WARNING:" "$log_file" | head -5 | while read -r line; do
            echo "  $line"
        done
        echo ""
    fi

    echo "📈 Performance Metrics (first 10 cases):"
    parse_criterion_results "$log_file" | head -10 | while IFS=$'\t' read -r name t_mid t_unit r_mid r_unit; do
        if [ -z "$name" ]; then
            continue
        fi
        if [ -n "$r_mid" ] && [ -n "$r_unit" ]; then
            echo "  • ${name}  time: ${t_mid} ${t_unit}  thrpt: ${r_mid} ${r_unit}"
        else
            echo "  • ${name}  time: ${t_mid} ${t_unit}"
        fi
    done
    
    echo ""
    echo "📊 Statistical Analysis:"
    grep -E "Found.*outliers" "$log_file" | head -3 | while read -r line; do
        echo "  📍 $line"
    done
    
    # Performance assessment
    echo ""
    echo "🔍 Performance Assessment:"
    local primary_row
    primary_row="$(pick_primary_result "$log_file")"
    local primary_case
    primary_case="$(echo "$primary_row" | cut -f1)"
    local throughput
    throughput="$(echo "$primary_row" | cut -f4)"
    local throughput_unit
    throughput_unit="$(echo "$primary_row" | cut -f5)"

    if [ -n "$primary_case" ] && [ "$primary_case" != "N/A" ]; then
        echo "  🧪 Peak Case: ${primary_case}"
    fi

    if [ -n "$throughput" ] && [ -n "$throughput_unit" ]; then
        # Convert to elements per second for comparison
        local elements_per_second=0
        case "$throughput_unit" in
            "Gelem/s")
                elements_per_second=$(echo "$throughput * 1000000000" | bc -l 2>/dev/null || echo "$((${throughput%.*} * 1000000000))")
                ;;
            "Melem/s")
                elements_per_second=$(echo "$throughput * 1000000" | bc -l 2>/dev/null || echo "$((${throughput%.*} * 1000000))")
                ;;
            "Kelem/s")
                elements_per_second=$(echo "$throughput * 1000" | bc -l 2>/dev/null || echo "$((${throughput%.*} * 1000))")
                ;;
            "elem/s")
                elements_per_second=${throughput%.*}
                ;;
        esac

        local eps_int=${elements_per_second%.*}
        if [ "$eps_int" -gt 1000000000 ]; then
            echo "  🟢 Excellent throughput (${throughput} ${throughput_unit})"
        elif [ "$eps_int" -gt 100000000 ]; then
            echo "  🟡 Good throughput (${throughput} ${throughput_unit})"
        elif [ "$eps_int" -gt 10000000 ]; then
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
            local primary_row
            primary_row="$(pick_primary_result "$log_file")"
            local primary_case throughput throughput_unit
            primary_case="$(echo "$primary_row" | cut -f1)"
            throughput="$(echo "$primary_row" | cut -f4)"
            throughput_unit="$(echo "$primary_row" | cut -f5)"

            if [ -z "$throughput" ] || [ -z "$throughput_unit" ]; then
                continue
            fi

            local throughput_int
            throughput_int=$(echo "$throughput" | cut -d. -f1)

            # Convert to common base unit for comparison
            local normalized_throughput=0
            case "$throughput_unit" in
                "Gelem/s") normalized_throughput=$(echo "$throughput_int * 1000000000" | bc -l 2>/dev/null || echo "$((throughput_int * 1000000000))") ;;
                "Melem/s") normalized_throughput=$((throughput_int * 1000000)) ;;
                "Kelem/s") normalized_throughput=$((throughput_int * 1000)) ;;
                "elem/s") normalized_throughput=$throughput_int ;;
            esac

            if [ "$normalized_throughput" -gt "$best_throughput" ]; then
                best_throughput=$normalized_throughput
                best_benchmark="$benchmark:$primary_case (${throughput} ${throughput_unit})"
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
