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

# Method 1: Try simple cargo flamegraph with DTrace
echo "Method 1: Trying cargo-flamegraph with DTrace..."
if timeout 30s cargo flamegraph --dev --bench "$BENCHMARK" -o "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg" -- --bench --sample-size 3 --measurement-time 2; then
    echo "✅ Success! Flame graph created: $OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    open "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    exit 0
fi

echo "❌ Method 1 failed, trying Method 2..."

# Method 2: Try with sudo
echo "Method 2: Trying with sudo privileges..."
if timeout 30s sudo cargo flamegraph --dev --bench "$BENCHMARK" -o "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg" -- --bench --sample-size 3 --measurement-time 2; then
    echo "✅ Success with sudo! Flame graph created: $OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    open "$OUTPUT_DIR/${BENCHMARK}_flamegraph.svg"
    exit 0
fi

echo "❌ Method 2 failed, trying Method 3..."

# Method 3: Use perf instead of instruments
echo "Method 3: Trying alternative profiler..."
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