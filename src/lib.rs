// The package-level `[lints] warnings = "deny"` (Cargo.toml) makes sense for
// the bin target, where every module is genuinely reachable from `main()`.
// It doesn't for this lib target: `filters` intentionally exposes only a
// tiny curated surface, so everything else in the shared source tree reads
// as "unreachable" to the lib's own dead-code analysis even though it's
// very much alive in the bin. Downgrading dead-code/unused lints back to
// warnings here, scoped to the lib compilation only -- the bin target's
// warnings-as-errors is untouched.
#![allow(dead_code, unused, unused_imports)]

//! RTK as a library, not just a binary.
//!
//! Added so downstream tools (starting with `codemode-cli`) can call RTK's
//! output filters as an in-process function call instead of spawning `rtk`
//! as a subprocess. Measured why this matters (thurionapp/rtk#2,
//! thurionapp/codemode-cli): `rtk`'s own process startup costs ~3.5-13ms
//! depending on path, which can exceed the entire cost of the thing being
//! filtered for a tool invoked many times in one script. A library call
//! pays none of that -- no process spawn, no arg parsing, no hook-check
//! overhead, just the filter's own (cheap, string-processing) work.
//!
//! This intentionally exposes a small, curated surface (`filters::*`), not
//! every internal module -- most of RTK's internals are still `pub(crate)`
//! or private, and stay that way until something downstream actually needs
//! them. Add to `filters` deliberately, one function at a time, not by
//! blanket-exposing `cmds`.

mod analytics;
mod cmds;
mod core;
mod discover;
mod hooks;
mod learn;
mod parser;

// Mirrors main.rs's crate-root re-exports exactly. Several internal files
// (e.g. src/cmds/git/gt_cmd.rs) reference these via absolute `crate::git::…`
// paths rather than relative `super::…` paths, which only resolves when
// these bindings exist at the crate root -- true for the bin target
// (because main.rs defines them) but not, until now, for the lib target.
// Copy-paste-exact from main.rs on purpose: if main.rs's imports change,
// this needs to change with it, and a diff makes that obvious.
use cmds::cloud::{aws_cmd, container, curl_cmd, psql_cmd, wget_cmd};
use cmds::dotnet::{binlog, dotnet_cmd, dotnet_format_report, dotnet_trx};
use cmds::git::{diff_cmd, gh_cmd, git, glab_cmd, gt_cmd};
use cmds::go::{go_cmd, golangci_cmd};
use cmds::js::{
    lint_cmd, next_cmd, npm_cmd, playwright_cmd, pnpm_cmd, prettier_cmd, prisma_cmd, tsc_cmd,
    vitest_cmd,
};
use cmds::jvm::{gradlew_cmd, mvn_cmd};
use cmds::php::{ecs_cmd, paratest_cmd, pest_cmd, php_cmd, phpstan_cmd, phpunit_cmd, pint_cmd};
use cmds::python::{mypy_cmd, pip_cmd, pytest_cmd, ruff_cmd, uv_cmd};
use cmds::ruby::{rake_cmd, rspec_cmd, rubocop_cmd};
use cmds::rust::{cargo_cmd, runner};
use cmds::scala::sbt_cmd;
use cmds::system::{
    deps, env_cmd, find_cmd, format_cmd, json_cmd, local_llm, log_cmd, ls, pipe_cmd, read, search,
    summary, tree, wc_cmd,
};

/// Minimal stand-in for the real `Commands` enum (the full `#[derive(Subcommand)]`
/// clap type, defined in `main.rs` and not reachable from the lib target).
/// Exists ONLY so `cmds::js::vitest_cmd::run_test` -- code this lib doesn't
/// call, but which lives in a module (`cmds::js`) this lib's `filters`
/// module transitively pulls in -- still type-checks. Never constructed or
/// matched against by anything in `filters`. If a future filter genuinely
/// needs the real `Commands` type, move the enum out of `main.rs` into a
/// shared module instead of growing this stub.
#[allow(dead_code)]
pub enum Commands {
    Vitest {},
    Jest {},
}

/// Curated public API. Two shapes:
///
/// - Pure `&str -> String` filters (`cargo_test`) -- caller does their own
///   spawning, this just filters output already captured.
/// - Spawn-and-filter convenience functions (`git_log`/`git_diff`/
///   `git_status`) -- these DO spawn `git` themselves, because RTK's git
///   filters are coupled to a specific invocation (e.g. `git log` needs a
///   `--pretty=format:...---END---` marker RTK injects; `filter_log_output`
///   silently misparses plain `git log` output without it). Exposing "spawn
///   git exactly the way RTK's own CLI does, then filter" is what's
///   actually correct to call from outside, not the raw filter alone.
///   Still zero `rtk`-*binary* spawn either way -- that's the cost this
///   module exists to remove (thurionapp/rtk#2, thurionapp/codemode-cli).
///
/// Kept intentionally tiny; each function here is something a real
/// downstream caller (codemode-cli) actually calls today, not a
/// speculative export.
pub mod filters {
    use crate::cmds::git::git::{compact_diff, filter_log_output, format_status_output};
    use crate::core::guard::never_worse;
    use std::path::Path;
    use std::process::Command;

    /// Same filter `rtk cargo test`/`rtk pipe -f cargo-test` uses: build
    /// errors, test results, clippy warnings, collapsed to essentials.
    pub fn cargo_test(output: &str) -> String {
        crate::cmds::rust::cargo_cmd::filter_cargo_test(output)
    }

    /// Same filter `rtk npx`/`rtk npm run` use: strips npm/npx boilerplate
    /// (lifecycle script banners, `npm WARN`/`npm notice` lines). Added
    /// after confirming real usage: `npx` appeared hundreds of times in real
    /// production transcripts, the single largest
    /// JS-tooling command by volume after `pnpm` -- which, unlike `npx`,
    /// doesn't have a real filter to migrate yet for its own highest-volume
    /// shape (`pnpm test`/`pnpm run <script>` fall through to
    /// `pnpm_cmd::run_passthrough`, unfiltered, confirmed by reading
    /// rtk's own source -- see thurionapp/rtk#3).
    pub fn npx(output: &str) -> String {
        crate::cmds::js::npm_cmd::filter_npm_output(output)
    }

    /// Filter for `pnpm test` / `pnpm run <script>` / `npm test` / `npm run
    /// <script>` output -- the real gap behind thurionapp/rtk#3: pnpm's CLI
    /// handler only covers list/outdated/install, so its highest-volume real
    /// shapes in production transcripts (mostly
    /// test/run) fell through to unfiltered passthrough. Closes it by
    /// recognizing what those scripts actually produce in this ecosystem
    /// (vitest, per real production usage): tries the existing
    /// `VitestParser` first (JSON reporter, then its regex tier for the
    /// default reporter's `Tests N passed` summary), and only when the
    /// output has no vitest shape at all falls back to the same
    /// npm-boilerplate strip `npx`/`npm run` use. Pure `&str -> String` --
    /// caller spawns pnpm/npm themselves with whatever args the script
    /// asked for, nothing about the invocation is rewritten.
    pub fn pnpm_run(output: &str) -> String {
        use crate::parser::{FormatMode, OutputParser, ParseResult, TokenFormatter};
        match crate::cmds::js::vitest_cmd::VitestParser::parse(output) {
            ParseResult::Full(data) | ParseResult::Degraded(data, _) => {
                never_worse(output, &data.format(FormatMode::from_verbosity(0))).to_string()
            }
            ParseResult::Passthrough(_) => {
                let filtered = crate::cmds::js::npm_cmd::filter_npm_output(output);
                never_worse(output, &filtered).to_string()
            }
        }
    }

    /// Filter for `make` output -- rtk's CLI has NO make handler at all
    /// (thurionapp/rtk#3: `make` is in the hook rewrite rules but has no
    /// `Commands::` variant, so `rtk make` falls into the slow
    /// zero-filtering passthrough). Built from what real `make` usage here
    /// actually produces (a make-heavy internal monorepo, hundreds of
    /// invocations per session --
    /// short custom-script check lines worth keeping verbatim, plus
    /// embedded `cargo build` progress runs in the hundreds of lines, the
    /// actual fat): collapses cargo compile-progress runs into one
    /// `[cargo: N crates compiled]` line and drops make's own
    /// Entering/Leaving-directory chatter. Deliberately conservative --
    /// keeps every line it doesn't positively recognize as noise (script
    /// ✓/❌ lines, errors, warnings, summaries all pass through untouched),
    /// because make output is arbitrary by nature and silently eating a
    /// line an agent needed is worse than under-compressing.
    pub fn make_output(output: &str) -> String {
        let mut result: Vec<String> = Vec::new();
        let mut compile_count = 0usize;
        for line in output.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("Compiling ")
                || trimmed.starts_with("Checking ")
                || trimmed.starts_with("Downloading ")
                || trimmed.starts_with("Downloaded ")
                || trimmed.starts_with("Fresh ")
            {
                compile_count += 1;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("Finished ") {
                if compile_count > 0 {
                    result.push(format!("[cargo: {compile_count} crates compiled] Finished {rest}"));
                    compile_count = 0;
                } else {
                    result.push(line.to_string());
                }
                continue;
            }
            if trimmed.starts_with("make[")
                && (trimmed.contains("Entering directory") || trimmed.contains("Leaving directory"))
            {
                continue;
            }
            result.push(line.to_string());
        }
        if compile_count > 0 {
            result.push(format!("[cargo: {compile_count} crates compiled]"));
        }
        let filtered = result.join("\n");
        never_worse(output, &filtered).to_string()
    }

    fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("failed to spawn git {}: {e}", args.join(" ")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// `git log`, condensed. Matches `rtk git log`'s defaults (10 commits,
    /// `--no-merges`, `%h %s (%ar) <%an>` + body, oldest info trimmed) --
    /// pass `limit` to override the commit count, same as `rtk git log -N`.
    pub fn git_log(cwd: &Path, limit: usize) -> Result<String, String> {
        let raw = run_git(
            cwd,
            &[
                "log",
                "--pretty=format:%h %s (%ar) <%an>%n%b%n---END---",
                "--no-merges",
                &format!("-{limit}"),
            ],
        )?;
        let filtered = filter_log_output(&raw, limit, false, false);
        Ok(never_worse(&raw, &filtered).to_string())
    }

    /// `git diff`, stat summary + compacted body -- matches `rtk git diff`'s
    /// default shape ("<stat>\n\nChanges:\n<compacted diff>").
    /// `max_lines` caps the compacted body, same as `rtk git diff` (500).
    pub fn git_diff(cwd: &Path, max_lines: usize) -> Result<String, String> {
        let stat = run_git(cwd, &["diff", "--stat"])?;
        let diff = run_git(cwd, &["diff"])?;
        if diff.is_empty() {
            return Ok(never_worse(&stat, stat.trim()).to_string());
        }
        let compacted = compact_diff(&diff, max_lines);
        let combined = format!("{}\n\nChanges:\n{}", stat.trim(), compacted);
        Ok(never_worse(&diff, &combined).to_string())
    }

    /// `git status`, condensed porcelain -- matches `rtk git status`'s
    /// default (`git status --porcelain -b`, reformatted).
    pub fn git_status(cwd: &Path) -> Result<String, String> {
        let raw = run_git(cwd, &["status", "--porcelain", "-b"])?;
        let filtered = format_status_output(&raw);
        Ok(never_worse(&raw, &filtered).to_string())
    }

    /// `gh pr diff`, compacted -- reuses the same `compact_diff` git_diff
    /// does (rtk's own `pr_diff` does exactly this). `extra_args` is
    /// whatever comes after `diff` (a PR number, `--repo owner/name`,
    /// etc.) -- passed straight through to the real `gh` invocation, same
    /// as a script would type them. Added after confirming real usage:
    /// production PR-review scripts call `gh pr diff <N> --repo <repo>`
    /// across multiple repos in one script.
    pub fn gh_pr_diff(cwd: &Path, extra_args: &[&str]) -> Result<String, String> {
        let mut args = vec!["pr", "diff"];
        args.extend(extra_args);
        let output = Command::new("gh")
            .args(&args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("failed to spawn gh {}: {e}", args.join(" ")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("gh {} failed: {}", args.join(" "), stderr.trim()));
        }
        let raw = String::from_utf8_lossy(&output.stdout).into_owned();
        if raw.trim().is_empty() {
            return Ok("No diff".to_string());
        }
        let compacted = compact_diff(&raw, 500);
        Ok(never_worse(&raw, &compacted).to_string())
    }
}
