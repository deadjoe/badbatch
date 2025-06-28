#!/bin/bash

# macOS-specific flame graph generation script
# This script provides alternative profiling methods for macOS

set -e

BENCHMARK=${1:-comprehensive_benchmarks}
OUTPUT_DIR="flamegraphs"

mkdir -p "$OUTPUT_DIR"

echo "🔥 macOS Flame Graph Generator"
echo "Benchmark: $BENCHMARK"
echo ""

# Method 1: Use samply (recommended for macOS)
echo "Method 1: Using samply (SIP-compatible profiler)..."
if command -v samply >/dev/null 2>&1; then
    echo "✅ samply found, generating flame graph..."
    
    # Record with samply
    if timeout 60s samply record --rate 997 --output "$OUTPUT_DIR/${BENCHMARK}.json" cargo bench --bench "$BENCHMARK" -- --bench --sample-size 5 --measurement-time 3; then
        
        # Try to generate SVG flame graph
        if samply flamegraph "$OUTPUT_DIR/${BENCHMARK}.json" --output "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"; then
            echo "✅ Success! Flame graph created: $OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
            open "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
            rm -f "$OUTPUT_DIR/${BENCHMARK}.json"
            exit 0
        else
            echo "⚠️  SVG generation failed, creating HTML profile instead..."
            if samply load "$OUTPUT_DIR/${BENCHMARK}.json" --output "$OUTPUT_DIR/${BENCHMARK}_profile.html"; then
                echo "✅ Success! HTML profile created: $OUTPUT_DIR/${BENCHMARK}_profile.html"
                open "$OUTPUT_DIR/${BENCHMARK}_profile.html"
                exit 0
            fi
        fi
    fi
    echo "❌ samply profiling failed, trying Method 2..."
else
    echo "❌ samply not found. Install with: cargo install samply"
    echo "Trying Method 2..."
fi

# Method 2: Try simple cargo flamegraph with DTrace
echo "Method 2: Trying cargo-flamegraph with DTrace..."
if timeout 30s cargo flamegraph --dev --bench "$BENCHMARK" -o "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg" -- --bench --sample-size 3 --measurement-time 2; then
    echo "✅ Success! Flame graph created: $OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    open "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    exit 0
fi

echo "❌ Method 2 failed, trying Method 3..."

# Method 3: Try with sudo
echo "Method 3: Trying with sudo privileges..."
if timeout 30s sudo cargo flamegraph --dev --bench "$BENCHMARK" -o "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg" -- --bench --sample-size 3 --measurement-time 2; then
    echo "✅ Success with sudo! Flame graph created: $OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    open "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    exit 0
fi

echo "❌ Method 3 failed, trying Method 4..."

# Method 4: Use perf instead of instruments (unlikely on macOS)
echo "Method 4: Trying alternative profiler..."
if command -v perf >/dev/null 2>&1; then
    echo "Using perf profiler..."
    cargo flamegraph --freq 997 --bench "$BENCHMARK" -o "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg" -- --bench --sample-size 3
else
    echo "⚠️  perf not available on macOS"
fi

# Method 4: Manual Instruments approach
echo ""
echo "🔧 Alternative Solutions:"
echo ""
echo "1. **Use Instruments.app manually:**"
echo "   - Open Instruments.app"
echo "   - Choose 'Time Profiler' template"
echo "   - Target: cargo bench --bench $BENCHMARK"
echo ""
echo "2. **Install and use samply (modern profiler):**"
echo "   cargo install samply"
echo "   samply record cargo bench --bench $BENCHMARK"
echo ""
echo "3. **Use dtrace directly:**"
echo "   sudo dtrace -n 'profile-997 { @[ustack()] = count(); }'"
echo ""
echo "4. **Check system requirements:**"
echo "   - Xcode Command Line Tools: xcode-select --install"
echo "   - Developer Mode enabled in System Preferences"
echo "   - SIP status: csrutil status"

# Create a basic performance report
echo ""
echo "📊 Creating basic performance report..."
cat > "$OUTPUT_DIR/${BENCHMARK}_performance_info.txt" << EOF
# macOS Performance Analysis Report

Generated: $(date)
Platform: macOS $(sw_vers -productVersion) ($(uname -m))
Benchmark: $BENCHMARK

## System Information
CPU: $(sysctl -n machdep.cpu.brand_string)
Cores: $(sysctl -n hw.ncpu)
Memory: $(( $(sysctl -n hw.memsize) / 1024 / 1024 / 1024 )) GB

## Flame Graph Status
❌ Automatic flame graph generation failed
🔧 Use manual profiling methods listed above

## Quick Profiling Commands
1. Simple timing: time cargo bench --bench $BENCHMARK
2. With samply: samply record cargo bench --bench $BENCHMARK  
3. With Instruments: see instructions above

## Troubleshooting
- Ensure Xcode Command Line Tools are installed
- Check Developer Mode in System Preferences > Privacy & Security
- Consider temporarily disabling SIP for advanced profiling
EOF

echo "✅ Performance report created: $OUTPUT_DIR/${BENCHMARK}_performance_info.txt"
echo ""
echo "💡 Tip: For detailed profiling on macOS, Instruments.app is often the best choice"