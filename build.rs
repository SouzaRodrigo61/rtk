use std::collections::HashSet;
use std::fs;
use std::path::Path;

fn main() {
    #[cfg(windows)]
    {
        // Clap + the full command graph can exceed the default 1 MiB Windows
        // main-thread stack during process startup. Reserve a larger stack for
        // the CLI binary so `rtk.exe --version`, `--help`, and hook entry
        // points start reliably without requiring ad-hoc RUSTFLAGS.
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }

    let filters_dir = Path::new("src/filters");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo");
    let dest = Path::new(&out_dir).join("builtin_filters.toml");

    // Rebuild when any file in src/filters/ changes
    println!("cargo:rerun-if-changed=src/filters");

    let mut files: Vec<_> = fs::read_dir(filters_dir)
        .expect("src/filters/ directory must exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();

    // Sort alphabetically for deterministic filter ordering
    files.sort_by_key(|e| e.file_name());

    let mut combined = String::from("schema_version = 1\n\n");

    for entry in &files {
        let content = fs::read_to_string(entry.path())
            .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", entry.path(), e));
        combined.push_str(&format!(
            "# --- {} ---\n",
            entry.file_name().to_string_lossy()
        ));
        combined.push_str(&content);
        combined.push_str("\n\n");
    }

    // Validate: parse the combined TOML to catch errors at build time
    let parsed: toml::Value = combined.parse().unwrap_or_else(|e| {
        panic!(
            "TOML validation failed for combined filters:\n{}\n\nCheck src/filters/*.toml files",
            e
        )
    });

    // Detect duplicate filter names across files
    if let Some(filters) = parsed.get("filters").and_then(|f| f.as_table()) {
        let mut seen: HashSet<String> = HashSet::new();
        for key in filters.keys() {
            if !seen.insert(key.clone()) {
                panic!(
                    "Duplicate filter name '{}' found across src/filters/*.toml files",
                    key
                );
            }
        }
    }

    fs::write(&dest, combined).expect("Failed to write combined builtin_filters.toml");

    // Build-time first-word index of every builtin match_command pattern, so
    // run_fallback can prove "no filter can possibly match this command"
    // without loading and compiling the full 63-filter registry (issue #2:
    // that load ran on EVERY unrecognized command, ~9ms of trust checks +
    // TOML parse + regex compiles per process, all for commands that match
    // nothing). Patterns of the shape `^word\b` / `^word\s...` / `^word(\s|$)`
    // reduce to a literal first word compared for free at runtime; anything
    // else (alternations, unanchored patterns) goes into a small "complex"
    // list the runtime checks with one lazily-built RegexSet.
    let mut literal_words: Vec<String> = Vec::new();
    let mut complex_patterns: Vec<String> = Vec::new();
    if let Some(filters) = parsed.get("filters").and_then(|f| f.as_table()) {
        for (name, def) in filters {
            let pat = def
                .get("match_command")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("filter '{}' has no match_command string", name));
            match leading_literal_word(pat) {
                Some(w) => literal_words.push(w),
                None => complex_patterns.push(pat.to_string()),
            }
        }
    }
    literal_words.sort();
    literal_words.dedup();
    let index = format!(
        "pub const BUILTIN_LITERAL_FIRST_WORDS: &[&str] = &{:?};\npub const BUILTIN_COMPLEX_PATTERNS: &[&str] = &{:?};\n",
        literal_words, complex_patterns
    );
    fs::write(Path::new(&out_dir).join("builtin_match_index.rs"), index)
        .expect("Failed to write builtin_match_index.rs");
}

/// The leading literal word of an anchored pattern, if extracting one is
/// provably safe for first-word rejection. `^make\b` -> "make"; returns
/// None for anything where the char after the word could extend the match
/// (e.g. `^g(cc|\+\+)\b`, where the real commands are gcc/g++, not "g") --
/// those patterns fall into the complex list and are matched properly.
fn leading_literal_word(pat: &str) -> Option<String> {
    let rest = pat.strip_prefix('^')?;
    let word: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if word.is_empty() {
        return None;
    }
    let tail = &rest[word.len()..];
    if tail.is_empty() || tail.starts_with("\\b") || tail.starts_with("\\s") || tail.starts_with("(\\s|$)") {
        Some(word)
    } else {
        None
    }
}
