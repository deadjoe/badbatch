# 实验证据索引

> 原始测量证据不再存放在本仓库内。它们统一归档在仓库同级目录：
> ```
> ~/github/deadjoe/badbatch_evidence/
> ```
> 本文件只保留索引、路径映射与复现说明。

## 证据树结构

```text
~/github/deadjoe/badbatch_evidence/
├── linux_vultr_20260719_b1/          # 第一批 Linux VPC（Vultr 149.28.16.26）
│   ├── results/                      # 原 badbatch/head_to_head_results/ 全部内容
│   └── handoff/                      # LINUX_VPS_PERFORMANCE_HANDOFF_20260720.md
├── linux_vultr_20260727_f2/          # F.2 三臂（Linux VPS）
│   ├── results/
│   └── binaries/
└── macos_mac16-1_20260726_f3/        # F.3（Mac mini M4 Pro）
    ├── results/
    ├── arms/
    └── build/
```

完整说明见 `~/github/deadjoe/badbatch_evidence/README.md`。

## 路径迁移对照

| 原仓库内路径（已失效） | 新外部路径 |
|---|---|
| `badbatch/head_to_head_results/linux_vultr_149.28.16.26/...` | `../badbatch_evidence/linux_vultr_20260719_b1/results/linux_vultr_149.28.16.26/...` |
| `badbatch/head_to_head_results/20260719_010308_51891/...` | `../badbatch_evidence/linux_vultr_20260719_b1/results/20260719_010308_51891/...` |
| `badbatch/head_to_head_results/full_native_20260719_180958/...` | `../badbatch_evidence/linux_vultr_20260719_b1/results/full_native_20260719_180958/...` |
| `badbatch/head_to_head_results/pipeline_fix_20260719_190407/...` | `../badbatch_evidence/linux_vultr_20260719_b1/results/pipeline_fix_20260719_190407/...` |
| `badbatch/head_to_head_results/post_align_20260719_191520/...` | `../badbatch_evidence/linux_vultr_20260719_b1/results/post_align_20260719_191520/...` |
| `badbatch/docs/private/LINUX_VPS_PERFORMANCE_HANDOFF_20260720.md` | `../badbatch_evidence/linux_vultr_20260719_b1/handoff/LINUX_VPS_PERFORMANCE_HANDOFF_20260720.md` |
| 原 `badbatch_f2_results/` / `badbatch_f2_binaries/` | `../badbatch_evidence/linux_vultr_20260727_f2/results/` / `binaries/` |
| 原 `badbatch_f3_results/` 等 | `../badbatch_evidence/macos_mac16-1_20260726_f3/results/` 等 |

## 审计与复现

1. **哈希校验**：每次迁移都生成了排序后的 SHA-256 清单并做了前后比对；详细报告见 `badbatch_evidence/README.md`。
2. **代码复现**：
   - 历史 Linux 基线实验使用 `scripts/run_head_to_head.sh`，对应源码见 `archive/*` tag（`archive/claim-lock-bypass`、`archive/causal-matrix`、`archive/linux-baseline`）。
   - F.2 三臂复现依赖 `linux_vultr_20260727_f2/binaries/` 中的精确二进制。
   - F.3 复现依赖 `macos_mac16-1_20260726_f3/build/` 与 `arms/` 中的精确二进制/源码副本。
3. **新鲜 clone 不包含证据**：`head_to_head_results/` 与 `docs/private/` 仍在 `.gitignore` 中；如需审计，请从外部证据树获取。

## 注意事项

- 公开仓库的文档（如 `DEVELOPMENT_PLAN_V3.md`）中仍使用 `<ip>` 占位符，这是为了避免把真实 VPS IP 写进公开 git 历史。本地查阅时请用实际目录名替换。
- `benches/results/*.md` 中的历史摘要路径已更新为外部相对路径；这些摘要本身仍是仓库内的 checked-in 记录。
