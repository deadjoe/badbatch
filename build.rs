//! Embeds reproducible build provenance for opt-in benchmark binaries.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const UNKNOWN: &str = "unknown";

fn git(repo: &Path, args: &[&str]) -> Option<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
}

fn git_text(repo: &Path, args: &[&str]) -> Option<String> {
    let output = git(repo, args)?;
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}

fn git_path(repo: &Path, path: &str) -> Option<PathBuf> {
    let value = git_text(repo, &["rev-parse", "--git-path", path])?;
    let value = PathBuf::from(value);
    Some(if value.is_absolute() {
        value
    } else {
        repo.join(value)
    })
}

fn emit_git_watch(repo: &Path, path: &str) {
    if let Some(path) = git_path(repo, path) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn emit_tracked_file_watches(repo: &Path) {
    let Some(output) = git(repo, &["ls-files", "-z"]) else {
        return;
    };
    for path in output.stdout.split(|byte| *byte == 0) {
        if path.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(path);
        println!(
            "cargo:rerun-if-changed={}",
            repo.join(path.as_ref()).display()
        );
    }
}

fn emit_source_directory_watches(repo: &Path) {
    for directory in ["src", "benches", "tests", "examples"] {
        let path = repo.join(directory);
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn main() {
    if env::var_os("CARGO_FEATURE_BENCH_TOOLS").is_none() {
        return;
    }

    let repo = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR"),
    );
    let revision = git_text(&repo, &["rev-parse", "--verify", "HEAD"])
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .unwrap_or_else(|| UNKNOWN.to_owned());
    let dirty = git(
        &repo,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )
    .map_or(UNKNOWN, |output| {
        if output.stdout.is_empty() {
            "false"
        } else {
            "true"
        }
    });

    println!("cargo:rustc-env=BADBATCH_BUILD_GIT_REV={revision}");
    println!("cargo:rustc-env=BADBATCH_BUILD_GIT_DIRTY={dirty}");

    // Cargo caches build-script output. Watch both the symbolic HEAD and its
    // concrete ref so a new commit rebuilds the embedded provenance. HEAD alone
    // is sufficient for detached worktrees, where it contains the commit ID.
    emit_git_watch(&repo, "HEAD");
    emit_git_watch(&repo, "index");
    emit_git_watch(&repo, "packed-refs");
    if let Some(reference) = git_text(&repo, &["symbolic-ref", "-q", "HEAD"]) {
        emit_git_watch(&repo, &reference);
    }

    // Dirty provenance must change when any tracked input changes, not merely
    // when Git's index changes.
    emit_tracked_file_watches(&repo);

    // Keep the stricter untracked-file dirty definition honest for directories
    // whose newly added files Cargo can automatically discover as build inputs.
    // Retain the per-file watches above as well: directory and in-place file
    // changes are independent cache invalidation cases.
    emit_source_directory_watches(&repo);
}
