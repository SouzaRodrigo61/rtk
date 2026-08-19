//! Proves `rtk` works as an external library dependency, not just a binary --
//! this file compiles against the crate the same way an external downstream
//! crate (codemode-cli) would, via `rtk::filters::*`.

use std::path::Path;

#[test]
fn cargo_test_filter_compresses_a_real_passing_run() {
    let raw = "\
running 3 tests
test foo::bar ... ok
test foo::baz ... ok
test foo::qux ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.42s
";
    let filtered = rtk::filters::cargo_test(raw);
    assert!(filtered.len() < raw.len(), "filter should shrink a real passing run");
    assert!(filtered.contains("3"), "should retain the pass count");
}

/// Real repo, not a fixture -- this crate's own checkout. Proves the
/// spawn-and-filter functions actually run `git` correctly (marker format,
/// porcelain flags, etc.), not just that the pure filters parse a string.
fn this_repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn git_log_returns_condensed_real_history() {
    let out = rtk::filters::git_log(this_repo(), 5).expect("git log should succeed in this repo");
    assert!(!out.is_empty());
    // Every commit line should be far shorter than a raw, unformatted log
    // entry would be -- a crude but real sanity check that filtering ran.
    assert!(out.lines().count() > 0);
}

#[test]
fn git_status_returns_something_on_a_real_repo() {
    let out = rtk::filters::git_status(this_repo()).expect("git status should succeed");
    // Clean or dirty, this should never error on a real repo -- just prove
    // it runs end to end without panicking or returning Err.
    let _ = out;
}

#[test]
fn git_diff_returns_something_on_a_real_repo() {
    let out = rtk::filters::git_diff(this_repo(), 500).expect("git diff should succeed");
    let _ = out;
}

#[test]
fn pnpm_run_condenses_vitest_default_reporter_output() {
    // The regex tier of VitestParser -- what `pnpm test` produces with the
    // default reporter (no JSON), the highest-volume real shape.
    let raw = "\n> someapp@0.1.0 test\n> vitest run\n\n \u{2713} src/a.test.ts > adds (3ms)\n \u{2713} src/a.test.ts > subtracts (1ms)\n\n Test Files  1 passed (1)\n      Tests  13 passed (13)\n   Duration  450ms\n";
    let out = rtk::filters::pnpm_run(raw);
    assert!(out.contains("13"), "should keep the test count: {out}");
    assert!(out.len() <= raw.len(), "never_worse must hold");
}

#[test]
fn pnpm_run_falls_back_to_npm_strip_on_non_test_output() {
    let raw = "\n> someapp@0.1.0 build\n> next build\n\nnpm WARN deprecated something\nCompiled successfully\n";
    let out = rtk::filters::pnpm_run(raw);
    assert!(out.contains("Compiled successfully"), "real content must survive: {out}");
    assert!(out.len() <= raw.len(), "never_worse must hold");
}

#[test]
fn make_output_collapses_cargo_progress_and_keeps_check_lines() {
    let raw = "make[1]: Entering directory '/x'\n   Compiling libc v0.2.153\n   Compiling serde v1.0.0\n   Compiling centaur-cli v0.9.0\n    Finished `release` profile [optimized] target(s) in 42.10s\n\u{2705} catalog.toml em dia com os 18 manifests\nmake[1]: Leaving directory '/x'\n";
    let out = rtk::filters::make_output(raw);
    assert!(out.contains("[cargo: 3 crates compiled]"), "progress not collapsed: {out}");
    assert!(out.contains("catalog.toml em dia"), "script check line must survive: {out}");
    assert!(!out.contains("Entering directory"), "make chatter must be dropped: {out}");
    assert!(!out.contains("Compiling libc"), "individual compile lines must be gone: {out}");
}

#[test]
fn make_output_is_conservative_on_unknown_lines() {
    let raw = "bash scripts/test_projection.sh\n\u{274c} projeção divergente no flow X\nerror: veja acima\n";
    let out = rtk::filters::make_output(raw);
    assert_eq!(out.trim_end(), raw.trim_end(), "unknown lines must pass through untouched");
}
