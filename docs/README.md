# Documentation layout

| Path | Status | Purpose |
|------|--------|---------|
| [`MODERNIZATION.md`](MODERNIZATION.md) | Tracked | Active engineering alignment (phases, architecture decisions). Agent/developer working map. |
| `training/` | Gitignored (optional) | English curriculum modules if present locally; publish only with an intentional commit. |
| `private/` | **Gitignored** | Local review drafts, Chinese audit notes, personal working copies. Never rely on this path in CI. |

Public product docs live primarily in the repo root (`README.md`, `DESIGN.md`) and in rustdoc.
