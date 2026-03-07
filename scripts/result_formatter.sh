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

# Parse latency statistics emitted by `benches/latency_comparison.rs` into rows:
#   label<TAB>mean_ns<TAB>median_ns<TAB>p95_ns<TAB>p99_ns<TAB>max_ns
parse_latency_stats() {
    local log_file="$1"

    awk '
    function flush_pending() {
        if (label != "" && mean != "" && median != "" && p95 != "" && p99 != "" && max != "") {
            print label "\t" mean "\t" median "\t" p95 "\t" p99 "\t" max;
        }
        label = "";
        mean = "";
        median = "";
        p95 = "";
        p99 = "";
        max = "";
    }

    BEGIN {
        label = "";
        mean = "";
        median = "";
        p95 = "";
        p99 = "";
        max = "";
    }

    /^[^[:space:]]+[[:space:]]Latency Statistics \(nanoseconds\):$/ {
        flush_pending();
        label = $1;
        next;
    }

    /^[[:space:]]+Mean:/ {
        if (label != "") mean = $2;
        next;
    }

    /^[[:space:]]+Median:/ {
        if (label != "") median = $2;
        next;
    }

    /^[[:space:]]+95th percentile:/ {
        if (label != "") p95 = $3;
        next;
    }

    /^[[:space:]]+99th percentile:/ {
        if (label != "") p99 = $3;
        next;
    }

    /^[[:space:]]+Max:/ {
        if (label != "") {
            max = $2;
            flush_pending();
        }
        next;
    }

    END {
        flush_pending();
    }
    ' "$log_file"
}

# Pick the latency result with the lowest mean latency.
pick_primary_latency_result() {
    local log_file="$1"

    parse_latency_stats "$log_file" | awk -F'\t' '
        {
            mean = $2 + 0;
            if (!have_best || mean < best_mean) {
                best_mean = mean;
                best_row = $0;
                have_best = 1;
            }
        }

        END {
            if (best_row != "") {
                print best_row;
            }
        }
    '
}

latency_label_to_case() {
    case "$1" in
        "Disruptor") echo "Latency/Disruptor/BusySpin" ;;
        "StdMpsc") echo "Latency/StdMpsc/sync_channel" ;;
        "Crossbeam") echo "Latency/Crossbeam/bounded" ;;
        *) echo "" ;;
    esac
}

extract_iterations_from_collect_line() {
    local collect_line="$1"
    local iterations=""

    if [ -n "$collect_line" ]; then
        iterations=$(echo "$collect_line" | sed -nE 's/.*\(([0-9.]+[KMGTBkmgtb]*) iterations\).*/\1/p')

        if [ -z "$iterations" ]; then
            iterations=$(echo "$collect_line" | grep -oE '[0-9.]+[KMGTBkmgtb]*' | tail -1)
        fi
    fi

    echo "${iterations:-N/A}"
}

normalize_throughput() {
    local throughput="$1"
    local throughput_unit="$2"

    awk -v value="$throughput" -v unit="$throughput_unit" '
        BEGIN {
            scale = 0;
            if (unit == "Gelem/s") scale = 1000000000;
            else if (unit == "Melem/s") scale = 1000000;
            else if (unit == "Kelem/s") scale = 1000;
            else if (unit == "elem/s") scale = 1;

            if (scale == 0 || value == "") {
                print 0;
            } else {
                printf "%.0f\n", value * scale;
            }
        }
    '
}

# Extract common benchmark metadata:
#   samples|iterations|warning_count
extract_benchmark_metadata() {
    local log_file="$1"
    local benchmark_case="${2:-}"
    local collect_line=""

    if [ -n "$benchmark_case" ]; then
        collect_line=$(grep -F "Benchmarking ${benchmark_case}: Collecting " "$log_file" | head -1)
    fi

    if [ -z "$collect_line" ]; then
        collect_line=$(grep -E "Collecting [0-9]+ samples" "$log_file" | head -1)
    fi

    local samples
    samples=$(echo "$collect_line" | sed -nE 's/.*Collecting ([0-9]+) samples.*/\1/p')
    local iterations
    iterations=$(extract_iterations_from_collect_line "$collect_line")

    local warning_count
    warning_count=$(grep -c "^WARNING:" "$log_file" 2>/dev/null || true)
    warning_count=${warning_count:-0}

    echo "${samples:-N/A}|${iterations}|${warning_count}"
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
    
    local metadata
    metadata="$(extract_benchmark_metadata "$log_file" "$primary_case")"
    local samples
    samples="$(echo "$metadata" | cut -d'|' -f1)"
    local iterations
    iterations="$(echo "$metadata" | cut -d'|' -f2)"
    local warning_count
    warning_count="$(echo "$metadata" | cut -d'|' -f3)"

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
            if [ "$benchmark" = "latency_comparison" ]; then
                local primary_latency_row
                primary_latency_row="$(pick_primary_latency_result "$log_file")"
                local label mean median p95 p99 max
                label="$(echo "$primary_latency_row" | cut -f1)"
                mean="$(echo "$primary_latency_row" | cut -f2)"
                median="$(echo "$primary_latency_row" | cut -f3)"
                p95="$(echo "$primary_latency_row" | cut -f4)"
                p99="$(echo "$primary_latency_row" | cut -f5)"
                max="$(echo "$primary_latency_row" | cut -f6)"
                local latency_case
                latency_case="$(latency_label_to_case "$label")"
                local metadata
                metadata="$(extract_benchmark_metadata "$log_file" "$latency_case")"
                local samples
                samples="$(echo "$metadata" | cut -d'|' -f1)"
                local iterations
                iterations="$(echo "$metadata" | cut -d'|' -f2)"
                local warning_count
                warning_count="$(echo "$metadata" | cut -d'|' -f3)"

                echo ""
                echo "🎯 Latency"
                if [ -n "$label" ]; then
                    echo "   🧪 Lowest Mean Latency: ${label}"
                fi
                if [ -n "$mean" ]; then
                    echo "   📉 Mean / Median: ${mean} ns / ${median} ns"
                    echo "   📍 P95 / P99: ${p95} ns / ${p99} ns"
                    echo "   📌 Max: ${max} ns"
                fi
                echo "   🔬 Samples: ${samples}"
                echo "   🔄 Iterations: ${iterations}"
                if [ "$warning_count" -gt 0 ]; then
                    echo "   ⚠️  Warnings: ${warning_count}"
                fi
                continue
            fi

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
    
    local metadata
    metadata="$(extract_benchmark_metadata "$log_file")"
    local warning_count
    warning_count="$(echo "$metadata" | cut -d'|' -f3)"
    if [ "$warning_count" -gt 0 ]; then
        echo "⚠️  Warnings in log: ${warning_count}"
        grep -E "^WARNING:" "$log_file" | head -5 | while read -r line; do
            echo "  $line"
        done
        echo ""
    fi

    if [ "$benchmark_name" = "latency_comparison" ]; then
        echo "📉 Latency Statistics:"
        parse_latency_stats "$log_file" | while IFS=$'\t' read -r label mean median p95 p99 max; do
            if [ -z "$label" ]; then
                continue
            fi
            echo "  • ${label}  mean: ${mean} ns  median: ${median} ns  p95: ${p95} ns  p99: ${p99} ns  max: ${max} ns"
        done

        echo ""
        echo "📈 Criterion Batch Metrics (for reference):"
    else
        echo "📈 Performance Metrics (first 10 cases):"
    fi

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
    local outlier_lines
    outlier_lines=$(grep -E "Found.*outliers" "$log_file" | head -3 || true)
    if [ -n "$outlier_lines" ]; then
        while read -r line; do
            echo "  📍 $line"
        done <<< "$outlier_lines"
    else
        echo "  📍 No outlier summary lines reported"
    fi
    
    # Performance assessment
    echo ""
    echo "🔍 Performance Assessment:"
    if [ "$benchmark_name" = "latency_comparison" ]; then
        local primary_latency_row
        primary_latency_row="$(pick_primary_latency_result "$log_file")"
        local label mean median p95 p99 max
        label="$(echo "$primary_latency_row" | cut -f1)"
        mean="$(echo "$primary_latency_row" | cut -f2)"
        median="$(echo "$primary_latency_row" | cut -f3)"
        p95="$(echo "$primary_latency_row" | cut -f4)"
        p99="$(echo "$primary_latency_row" | cut -f5)"
        max="$(echo "$primary_latency_row" | cut -f6)"

        if [ -n "$label" ]; then
            echo "  🧪 Lowest Mean Latency: ${label}"
            echo "  📉 Mean / Median: ${mean} ns / ${median} ns"
            echo "  📍 P95 / P99: ${p95} ns / ${p99} ns"
            echo "  📌 Max: ${max} ns"
        fi
    else
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
            local elements_per_second
            elements_per_second=$(normalize_throughput "$throughput" "$throughput_unit")

            if [ "$elements_per_second" -gt 1000000000 ]; then
                echo "  🟢 Excellent throughput (${throughput} ${throughput_unit})"
            elif [ "$elements_per_second" -gt 100000000 ]; then
                echo "  🟡 Good throughput (${throughput} ${throughput_unit})"
            elif [ "$elements_per_second" -gt 10000000 ]; then
                echo "  🟠 Moderate throughput (${throughput} ${throughput_unit})"
            else
                echo "  🔴 Low throughput (${throughput} ${throughput_unit})"
            fi
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
    local highest_throughput=""
    
    for benchmark in "${successful_benchmarks[@]}"; do
        case "$benchmark" in
            "comprehensive_benchmarks"|"latency_comparison")
                continue
                ;;
        esac

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

            local normalized_throughput
            normalized_throughput=$(normalize_throughput "$throughput" "$throughput_unit")

            if [ "$normalized_throughput" -gt "$best_throughput" ]; then
                best_throughput=$normalized_throughput
                highest_throughput="$benchmark:$primary_case (${throughput} ${throughput_unit})"
            fi
        fi
    done
    
    if [ -n "$highest_throughput" ]; then
        echo "  🥇 Highest Reported Throughput: $highest_throughput"
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
