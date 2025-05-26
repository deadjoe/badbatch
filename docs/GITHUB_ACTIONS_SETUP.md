# GitHub Actions 设置指南

这个文档将帮助你为BadBatch项目设置GitHub Actions。

## 🚀 快速开始

### 1. 推送代码到GitHub

```bash
# 如果还没有创建仓库，先在GitHub上创建
git add .
git commit -m "feat: add GitHub Actions CI/CD pipeline"
git push origin main
```

### 2. 启用GitHub Actions

GitHub Actions会在你推送代码后自动启用。你可以在仓库的"Actions"标签页查看运行状态。

## 📋 工作流说明

### CI工作流 (`.github/workflows/ci.yml`)

**触发条件：**
- 推送到`main`或`develop`分支
- 创建Pull Request到`main`分支

**功能：**
- ✅ 代码格式检查 (rustfmt)
- ✅ 静态代码分析 (clippy)
- ✅ 运行所有测试
- ✅ 多Rust版本测试 (stable + MSRV 1.70.0)
- ✅ 多平台构建 (Linux, Windows, macOS)
- ✅ 代码覆盖率报告
- ✅ 安全审计
- ✅ 文档生成和部署

### 发布工作流 (`.github/workflows/release.yml`)

**触发条件：**
- 推送版本标签 (如 `v0.1.0`)

**功能：**
- 🚀 自动创建GitHub Release
- 📦 构建多平台二进制文件
- 📤 上传Release资产
- 📦 发布到crates.io (可选)

### 依赖管理工作流 (`.github/workflows/dependencies.yml`)

**触发条件：**
- 每周一自动运行
- 手动触发

**功能：**
- 🔄 自动更新依赖
- 🔍 安全审计
- 📝 自动创建PR

### 性能基准测试 (`.github/workflows/benchmark.yml`)

**触发条件：**
- 推送到`main`分支
- Pull Request
- 手动触发

**功能：**
- 📊 运行性能基准测试
- 📈 跟踪性能变化
- ⚠️ 性能回归警告

## 🔧 必要的设置

### 1. Codecov设置 (可选)

如果你想要代码覆盖率报告：

1. 访问 [codecov.io](https://codecov.io)
2. 使用GitHub账号登录
3. 添加你的仓库
4. 复制token到GitHub Secrets (名称: `CODECOV_TOKEN`)

### 2. Crates.io发布设置 (可选)

如果你想要自动发布到crates.io：

1. 在 [crates.io](https://crates.io) 创建API token
2. 在GitHub仓库设置中添加Secret:
   - 名称: `CARGO_REGISTRY_TOKEN`
   - 值: 你的crates.io API token

### 3. GitHub Pages设置 (自动文档)

1. 进入仓库设置 → Pages
2. Source选择"GitHub Actions"
3. 文档将自动部署到 `https://yourusername.github.io/badbatch`

## 📝 使用说明

### 创建发布

```bash
# 创建并推送版本标签
git tag v0.1.0
git push origin v0.1.0
```

这将自动：
- 创建GitHub Release
- 构建多平台二进制文件
- 发布到crates.io (如果配置了token)

### 手动触发工作流

1. 进入GitHub仓库的"Actions"页面
2. 选择要运行的工作流
3. 点击"Run workflow"按钮

### 查看结果

- **CI状态**: 在README徽章中显示
- **代码覆盖率**: 在Codecov徽章中显示
- **测试结果**: 在Actions页面查看详细日志
- **性能基准**: 在Actions页面查看基准测试结果

## 🛠️ 自定义配置

### 修改测试矩阵

编辑 `.github/workflows/ci.yml` 中的 `matrix` 部分：

```yaml
strategy:
  matrix:
    rust:
      - stable
      - beta      # 添加beta测试
      - 1.70.0    # MSRV
```

### 添加更多平台

编辑 `.github/workflows/release.yml` 中的构建矩阵。

### 调整依赖更新频率

编辑 `.github/workflows/dependencies.yml` 中的 `cron` 表达式。

## 🔍 故障排除

### 常见问题

1. **测试失败**: 检查本地是否所有测试都通过
2. **格式检查失败**: 运行 `cargo fmt` 修复格式
3. **Clippy警告**: 运行 `cargo clippy --fix` 修复警告
4. **依赖问题**: 检查 `Cargo.toml` 和 `Cargo.lock`

### 查看日志

1. 进入GitHub仓库的"Actions"页面
2. 点击失败的工作流
3. 展开失败的步骤查看详细错误信息

## 📞 获取帮助

如果遇到问题：

1. 检查GitHub Actions文档
2. 查看工作流日志
3. 在项目中创建Issue

## 🎯 最佳实践

1. **小步提交**: 频繁提交小的更改
2. **测试优先**: 确保本地测试通过再推送
3. **语义化版本**: 使用语义化版本标签
4. **文档更新**: 保持CHANGELOG.md更新
5. **安全意识**: 定期检查安全审计结果

这个设置为你提供了一个完整的CI/CD流水线，无需太多维护工作！🚀
