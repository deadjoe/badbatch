# 🔥 火焰图分析指南

## 安装要求

### macOS 环境设置

1. **安装 cargo-flamegraph**：
   ```bash
   cargo install flamegraph
   ```

2. **dtrace 权限** (macOS 特殊要求)：
   - dtrace 默认在 macOS 上可用
   - 可能需要管理员权限运行
   - 如果遇到权限问题，脚本会自动尝试使用 `sudo`

## 使用方法

### 生成火焰图
```bash
# 运行火焰图分析
./scripts/run_benchmarks.sh flamegraph
```

### 交互式选择
运行后会显示选项菜单：
```
Available benchmarks for flame graph analysis:
  1. comprehensive_benchmarks
  2. throughput_comparison  
  3. latency_comparison
  a. All benchmarks

Select benchmark(s) to profile (number, 'a' for all, or Enter for comprehensive):
```

### 输出文件
- 火焰图保存在 `flamegraphs/` 目录
- 文件格式：`{benchmark_name}_flamegraph.svg`
- 自动在默认浏览器中打开

## 火焰图分析技巧

### 🔍 如何阅读火焰图
- **宽度 = CPU 时间**：函数条越宽，消耗的 CPU 时间越多
- **高度 = 调用栈深度**：显示函数调用层次关系
- **颜色**：随机生成，不代表性能好坏

### 🎯 性能分析要点
1. **寻找热点**：查找最宽的函数条
2. **调用链分析**：跟踪从底部到顶部的调用路径
3. **意外发现**：寻找不应该出现的宽函数条
4. **比较分析**：对比不同版本的火焰图

### 🛠️ 交互功能
- **点击放大**：点击函数条可放大查看详细信息
- **搜索功能**：使用 Ctrl+F 搜索特定函数名
- **重置视图**：双击空白区域重置缩放

## 常见问题解决

### macOS 权限问题
如果遇到 dtrace 权限错误：
```bash
# 方法1：使用 sudo 运行脚本
sudo ./scripts/run_benchmarks.sh flamegraph

# 方法2：临时授权 dtrace
sudo dtruss -c cargo flamegraph --bench comprehensive_benchmarks
```

### 性能优化建议
1. **运行前优化系统**：
   ```bash
   ./scripts/run_benchmarks.sh optimize && ./scripts/run_benchmarks.sh flamegraph
   ```

2. **关闭其他应用**：减少系统噪声
3. **使用 AC 电源**：避免 CPU 节能模式影响

## 示例分析场景

### 🚀 吞吐量优化
```bash
# 生成吞吐量测试的火焰图
./scripts/run_benchmarks.sh flamegraph
# 选择: 2 (throughput_comparison)
```

### ⚡ 延迟优化  
```bash
# 生成延迟测试的火焰图
./scripts/run_benchmarks.sh flamegraph
# 选择: 3 (latency_comparison)
```

### 📊 全面分析
```bash
# 生成所有基准测试的火焰图
./scripts/run_benchmarks.sh flamegraph
# 选择: a (All benchmarks)
```

---

**提示**：火焰图生成可能需要几分钟时间，请耐心等待。生成的 SVG 文件可以在任何现代浏览器中查看。