# Repository Guidelines

This repository implements a high‑performance Rust Disruptor engine. Use the following conventions to contribute efficiently and keep quality consistent.

## Project Structure & Module Organization
- Source: `src/` (core under `src/disruptor/`, crate root `src/lib.rs`).
- Tests: `tests/` for integration tests; doctests live in `src` docs.
- Benchmarks: `benches/` (Criterion); logs in `benchmark_logs/`.
- Examples: `examples/`; Docs: `docs/`, `README.md`, `DESIGN.md`.
- Scripts: `scripts/test-all.sh`, `scripts/run_benchmarks.sh` (helpers and formatters included).

## Build, Test, and Development Commands
- Build: `cargo build` (release: `cargo build --release`).
- Unit/Integration tests: `cargo test --all-targets`.
- Lint: `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings`.
- Full local suite: `bash scripts/test-all.sh` (fmt, clippy, tests, coverage, audit, deny).
- Coverage: `cargo llvm-cov --lib --html` (requires `cargo-llvm-cov`).
- Benchmarks: `cargo bench` or `bash scripts/run_benchmarks.sh` (optional `cargo-criterion`).

## Coding Style & Naming Conventions
- Language: Rust 2021, min Rust `1.70`.
- Formatting: `rustfmt` defaults (run `cargo fmt`). Indentation: 4 spaces.
- Naming: `snake_case` for functions/vars, `CamelCase` for types/traits, `SCREAMING_SNAKE_CASE` for constants.
- Lints: keep clippy clean (`-D warnings`). See `[lints.clippy]` in `Cargo.toml`.

## Testing Guidelines
- Place integration tests in `tests/*.rs` (descriptive filenames, e.g., `event_processing_integration_test.rs`).
- Prefer property tests with `proptest` where applicable; include doc tests for public APIs.
- Ensure coverage stays stable; generate HTML reports via `cargo llvm-cov --html`.

## Commit & Pull Request Guidelines
- Commits: follow Conventional Commits (`feat:`, `fix:`, `docs:`, `chore:`, scopes like `ring_buffer:`). Keep messages imperative and focused.
- PRs: include a clear description, linked issues, rationale, and benchmarks or micro‑profiles when performance‑relevant. Add tests for bug fixes and new features.
- CI parity: verify `scripts/test-all.sh` passes locally before requesting review.

## Benchmarks & Performance
- Run targeted benches: `cargo bench --bench <name>` (see `benches/`).
- For macOS flame graphs, see recent changes in history; prefer sampling tools and include summary output from `scripts/result_formatter.sh` when sharing results.

## Security & Configuration
- Run `cargo audit` and `cargo deny check` (also in `scripts/test-all.sh`).
- Logging via `tracing`/`env_logger`; avoid leaking sensitive data in logs.
