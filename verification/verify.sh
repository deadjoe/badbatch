#!/bin/bash

# BadBatch Disruptor TLA+ Verification Script
# This script runs TLA+ model checking for all BadBatch verification models

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
VERIFICATION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TLC_JAR="${TLC_JAR:-tla2tools.jar}"
JAVA_OPTS="${JAVA_OPTS:--Xmx4g -XX:+UseParallelGC}"

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

# Function to check if TLA+ tools are available
check_tlc() {
    if command -v tlc >/dev/null 2>&1; then
        TLC_CMD="tlc"
        return 0
    elif [ -f "$TLC_JAR" ]; then
        TLC_CMD="java $JAVA_OPTS -jar $TLC_JAR"
        return 0
    elif [ -f "$VERIFICATION_DIR/$TLC_JAR" ]; then
        TLC_CMD="java $JAVA_OPTS -jar $VERIFICATION_DIR/$TLC_JAR"
        return 0
    else
        return 1
    fi
}

# Function to run TLA+ model checking
run_tlc() {
    local model_file="$1"
    local config_file="$2"
    local model_name="$(basename "$model_file" .tla)"

    print_status "Verifying $model_name with config $(basename "$config_file")..."

    # Create temporary directory for TLC output
    local temp_dir=$(mktemp -d)
    local log_file="$temp_dir/tlc.log"

    # Run TLC model checker with deadlock checking enabled (-deadlock)
    cd "$VERIFICATION_DIR"
    if $TLC_CMD -deadlock -config "$config_file" "$model_file" > "$log_file" 2>&1; then
        print_success "$model_name verification completed successfully"

        # Extract and display key statistics
        if grep -q "states generated" "$log_file"; then
            local stats=$(grep "states generated\|distinct states\|No errors" "$log_file" | head -3)
            echo "$stats" | while read line; do
                echo "  $line"
            done
        fi

        return 0
    else
        print_error "$model_name verification failed"
        echo "Error details:"
        tail -20 "$log_file" | sed 's/^/  /'

        # Clean up and return error
        rm -rf "$temp_dir"
        return 1
    fi

    # Clean up
    rm -rf "$temp_dir"
}

# Function to verify a specific model
verify_model() {
    local model="$1"
    local config="$2"

    if [ ! -f "$VERIFICATION_DIR/$model.tla" ]; then
        print_error "Model file $model.tla not found"
        return 1
    fi

    if [ ! -f "$VERIFICATION_DIR/configs/$config.cfg" ]; then
        print_error "Config file configs/$config.cfg not found"
        return 1
    fi

    run_tlc "$model.tla" "configs/$config.cfg"
}

# Function to run quick verification suite
quick_verification() {
    print_status "Running quick verification suite..."

    local failed=0

    # Verify SPMC model
    if ! verify_model "BadBatchSPMC" "spmc_config"; then
        failed=$((failed + 1))
    fi

    # Verify MPMC model
    if ! verify_model "BadBatchMPMC" "mpmc_config"; then
        failed=$((failed + 1))
    fi

    if [ $failed -eq 0 ]; then
        print_success "All quick verifications passed!"
        return 0
    else
        print_error "$failed verification(s) failed"
        return 1
    fi
}

# Function to run extended verification
extended_verification() {
    print_status "Running extended verification suite..."

    local failed=0

    # Run quick verification first
    if ! quick_verification; then
        print_error "Quick verification failed, skipping extended tests"
        return 1
    fi

    # Verify with extended configuration
    print_status "Running extended MPMC verification..."
    if ! verify_model "BadBatchMPMC" "extended_config"; then
        failed=$((failed + 1))
    fi

    if [ $failed -eq 0 ]; then
        print_success "All extended verifications passed!"
        return 0
    else
        print_error "$failed extended verification(s) failed"
        return 1
    fi
}

# Function to display usage
usage() {
    echo "Usage: $0 [OPTIONS] [COMMAND]"
    echo ""
    echo "Commands:"
    echo "  quick      Run quick verification suite (default)"
    echo "  extended   Run extended verification suite"
    echo "  spmc       Verify SPMC model only"
    echo "  mpmc       Verify MPMC model only"
    echo "  help       Show this help message"
    echo ""
    echo "Options:"
    echo "  --tlc-jar PATH    Path to tla2tools.jar"
    echo "  --java-opts OPTS  Java options for TLC (default: $JAVA_OPTS)"
    echo ""
    echo "Environment Variables:"
    echo "  TLC_JAR           Path to tla2tools.jar"
    echo "  JAVA_OPTS         Java options for TLC"
    echo ""
    echo "Examples:"
    echo "  $0                    # Run quick verification"
    echo "  $0 extended           # Run extended verification"
    echo "  $0 spmc               # Verify SPMC model only"
    echo "  $0 --tlc-jar /path/to/tla2tools.jar quick"
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --tlc-jar)
            TLC_JAR="$2"
            shift 2
            ;;
        --java-opts)
            JAVA_OPTS="$2"
            shift 2
            ;;
        quick|extended|spmc|mpmc|help)
            COMMAND="$1"
            shift
            ;;
        *)
            print_error "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# Set default command
COMMAND="${COMMAND:-quick}"

# Main execution
main() {
    print_status "BadBatch Disruptor TLA+ Verification"
    print_status "======================================"

    # Check if TLA+ tools are available
    if ! check_tlc; then
        print_error "TLA+ tools not found!"
        echo ""
        echo "Please install TLA+ tools:"
        echo "1. Download tla2tools.jar from: https://github.com/tlaplus/tlaplus/releases"
        echo "2. Set TLC_JAR environment variable or use --tlc-jar option"
        echo "3. Or install TLC command line tool"
        exit 1
    fi

    print_status "Using TLC command: $TLC_CMD"
    echo ""

    case "$COMMAND" in
        quick)
            quick_verification
            ;;
        extended)
            extended_verification
            ;;
        spmc)
            verify_model "BadBatchSPMC" "spmc_config"
            ;;
        mpmc)
            verify_model "BadBatchMPMC" "mpmc_config"
            ;;
        help)
            usage
            exit 0
            ;;
        *)
            print_error "Unknown command: $COMMAND"
            usage
            exit 1
            ;;
    esac
}

# Run main function
main "$@"
