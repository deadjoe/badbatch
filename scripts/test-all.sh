#!/bin/bash

# BadBatch 综合测试脚本
# 包含代码格式检查、静态分析、测试覆盖率、安全审计和依赖管理检查

set -e  # 遇到错误立即退出
set -o pipefail

echo "🚀 BadBatch 综合测试开始..."
echo "================================"

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 函数：打印带颜色的消息
print_step() {
    echo -e "${BLUE}📋 $1${NC}"
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

# 检查必要的工具是否安装
check_tools() {
    print_step "检查必要工具..."

    local missing_tools=()

    # cargo-llvm-cov 为可选，缺失时仅提示，覆盖率步骤会自动跳过
    if ! cargo llvm-cov --version >/dev/null 2>&1; then
        print_warning "未检测到 cargo-llvm-cov，将在覆盖率步骤自动跳过 (安装: cargo install cargo-llvm-cov)"
    fi

    # 在离线或显式跳过时，不强制检查 audit/deny
    if [ -z "$BB_SKIP_AUDIT" ] && [ -z "$BB_OFFLINE" ]; then
        if ! cargo audit --version >/dev/null 2>&1; then
            missing_tools+=("cargo-audit")
        fi
    fi

    if [ -z "$BB_SKIP_DENY" ] && [ -z "$BB_OFFLINE" ]; then
        if ! cargo deny --version >/dev/null 2>&1; then
            missing_tools+=("cargo-deny")
        fi
    fi

    if [ ${#missing_tools[@]} -ne 0 ]; then
        print_error "缺少以下工具: ${missing_tools[*]}"
        echo "请运行: cargo install ${missing_tools[*]}"
        exit 1
    fi

    print_success "所有必要工具已就绪"
}

# 1. 代码格式检查
format_check() {
    print_step "1. 代码格式检查 (rustfmt)"

    if cargo fmt --check; then
        print_success "代码格式检查通过"
    else
        if [ -n "$BB_CI" ]; then
            print_error "CI 模式下格式检查失败，请先本地运行 cargo fmt 修复"
            exit 1
        else
            print_warning "代码格式不符合标准，正在自动修复..."
            cargo fmt
            print_success "代码格式已修复"
        fi
    fi
}

# 1b. 特性矩阵编译检查（快速发现特性组合问题）
feature_matrix_check() {
    print_step "1b. 特性矩阵编译检查 (cargo check)"
    cargo check --all-targets --all-features --locked
    cargo check --all-targets --no-default-features --locked || true
    # 允许无默认特性失败作为提示（部分项目不支持完全关闭默认特性）
}

# 2. 静态代码分析
clippy_check() {
    print_step "2. 静态代码分析 (clippy)"

    if cargo clippy --all-targets --all-features --locked -- -D warnings; then
        print_success "静态代码分析通过"
    else
        print_error "静态代码分析发现问题，请修复后重试"
        exit 1
    fi
}

# 3. 单元测试
unit_tests() {
    print_step "3. 单元测试"

    if cargo test --lib --locked; then
        print_success "单元测试通过"
    else
        print_error "单元测试失败"
        exit 1
    fi
}

# 9. 测试覆盖率
coverage_test() {
    print_step "9. 测试覆盖率分析 (llvm-cov)"

    if [ -n "$BB_SKIP_COVERAGE" ]; then
        print_warning "已跳过覆盖率分析 (设置 BB_SKIP_COVERAGE=1)"
        return 0
    fi

    # 检查是否安装了cargo-llvm-cov
    if ! cargo llvm-cov --version >/dev/null 2>&1; then
        print_warning "cargo-llvm-cov 未安装，跳过覆盖率测试"
        print_warning "安装命令: cargo install cargo-llvm-cov"
        return 0
    fi

    # 对于rustup安装的Rust，检查llvm-tools组件
    if command -v rustup >/dev/null 2>&1; then
        if ! rustup component list --installed | grep -q llvm-tools; then
            print_warning "llvm-tools 组件未安装，尝试安装..."
            if rustup component add llvm-tools-preview; then
                print_success "llvm-tools 组件安装成功"
            else
                print_warning "llvm-tools 组件安装失败，跳过覆盖率测试"
                return 0
            fi
        fi
    else
        # 对于非rustup安装的Rust，尝试查找LLVM工具
        local llvm_cov_found=""
        local llvm_profdata_found=""

        # 检查常见的LLVM工具位置
        local search_paths=(
            "/opt/homebrew/bin"                    # macOS Homebrew (Apple Silicon)
            "/usr/local/bin"                       # macOS Homebrew (Intel) / Linux
            "/usr/bin"                             # Linux 系统包管理器
            "/opt/homebrew/Cellar/llvm/*/bin"      # macOS Homebrew Cellar (动态版本)
            "/usr/local/Cellar/llvm/*/bin"         # macOS Homebrew Cellar (Intel)
        )

        # 首先检查直接路径
        for path in "/opt/homebrew/bin" "/usr/local/bin" "/usr/bin"; do
            if [ -f "$path/llvm-cov" ] && [ -f "$path/llvm-profdata" ]; then
                llvm_cov_found="$path/llvm-cov"
                llvm_profdata_found="$path/llvm-profdata"
                break
            fi
        done

        # 如果直接路径没找到，尝试动态查找
        if [ -z "$llvm_cov_found" ]; then
            llvm_cov_found=$(find /opt/homebrew/Cellar/llvm /usr/local/Cellar/llvm -name "llvm-cov" -type f 2>/dev/null | head -1)
            llvm_profdata_found=$(find /opt/homebrew/Cellar/llvm /usr/local/Cellar/llvm -name "llvm-profdata" -type f 2>/dev/null | head -1)
        fi

        if [ -n "$llvm_cov_found" ] && [ -n "$llvm_profdata_found" ]; then
            export LLVM_COV="$llvm_cov_found"
            export LLVM_PROFDATA="$llvm_profdata_found"
            print_success "使用系统LLVM工具: $LLVM_COV"
        else
            print_warning "未找到LLVM工具，跳过覆盖率测试"
            print_warning "安装建议:"
            print_warning "  macOS: brew install llvm"
            print_warning "  Ubuntu/Debian: sudo apt install llvm"
            print_warning "  或使用rustup: rustup component add llvm-tools-preview"
            return 0
        fi
    fi

    # 清理之前的覆盖率数据
    cargo llvm-cov clean

    # 运行覆盖率测试并生成HTML报告
    if cargo llvm-cov --lib --html --no-cfg-coverage --ignore-filename-regex="/private/tmp/.*rustc.*"; then
        print_success "测试覆盖率分析完成"
        echo "📊 覆盖率报告已生成到 target/llvm-cov/html/index.html"

        # 显示覆盖率摘要（不重新运行测试，只显示已有数据的摘要）
        echo ""
        echo "📊 覆盖率摘要:"
        cargo llvm-cov report --summary-only --ignore-filename-regex="/private/tmp/.*rustc.*"
    else
        print_warning "测试覆盖率分析失败，但继续执行其他测试"
    fi
}

# 4. 安全审计
security_audit() {
    print_step "4. 安全审计 (cargo-audit)"

    if [ -n "$BB_SKIP_AUDIT" ] || [ -n "$BB_OFFLINE" ]; then
        print_warning "已跳过安全审计 (设置 BB_SKIP_AUDIT=1 或 BB_OFFLINE=1)"
        return 0
    fi

    if cargo audit; then
        print_success "安全审计通过"
    else
        if [ -n "$BB_CI" ]; then
            print_error "CI 模式下安全审计失败（2026-07-18 审计整改：audit 硬失败）"
            exit 1
        else
            print_warning "安全审计发现问题，请检查输出（CI 中将硬失败）"
        fi
    fi
}

# 5. 依赖管理检查
dependency_check() {
    print_step "5. 依赖管理检查 (cargo-deny)"

    if [ -n "$BB_SKIP_DENY" ] || [ -n "$BB_OFFLINE" ]; then
        print_warning "已跳过依赖管理检查 (设置 BB_SKIP_DENY=1 或 BB_OFFLINE=1)"
        return 0
    fi

    if cargo deny check; then
        print_success "依赖管理检查通过"
    else
        if [ -n "$BB_CI" ]; then
            print_error "CI 模式下依赖检查失败（2026-07-18 审计整改：deny 硬失败）"
            exit 1
        fi
        print_warning "依赖管理检查发现问题，请检查输出（CI 中将硬失败）"
        # 不退出，因为可能只是警告
    fi
}

# 6. 集成测试
integration_tests() {
    print_step "6. 集成测试"

    if cargo test --test '*' --locked; then
        print_success "集成测试通过"
    else
        print_error "集成测试失败"
        exit 1
    fi
}

# 7. 文档测试
doc_tests() {
    print_step "7. 文档测试"

    if cargo test --doc --locked; then
        print_success "文档测试通过"
    else
        print_error "文档测试失败"
        exit 1
    fi
}

# 8. 文档构建（可选）
doc_build() {
    print_step "8. 文档构建 (cargo doc)"
    if cargo doc --no-deps >/dev/null 2>&1; then
        print_success "文档构建成功"
    else
        print_warning "文档构建失败，非阻断（可在本地修复文档注释或类型可见性）"
    fi
}

# 主函数
main() {
    check_tools
    format_check
    feature_matrix_check
    clippy_check
    unit_tests
    security_audit
    dependency_check
    integration_tests
    doc_tests
    doc_build
    coverage_test

    echo ""
    echo "================================"
    print_success "🎉 所有测试完成！"
    echo ""
    echo "📊 测试覆盖率报告: target/llvm-cov/html/index.html"
    echo "📋 如需查看详细的覆盖率信息，请打开上述HTML文件"
}

# 运行主函数
main "$@"
