//! Builder unit tests (split by topic).
#![allow(
    unused_imports,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::items_after_statements,
    clippy::too_many_lines
)]

mod basic;
mod common;
mod failure_diagnostics;
mod handle;
mod integration_comprehensive;
mod integration_e2e;
mod pipeline_dependency;
mod pipeline_parallel;
