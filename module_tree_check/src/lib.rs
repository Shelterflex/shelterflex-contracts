//! Module-tree reachability guard.
//!
//! Every `.rs` file under a crate's `src/` must be reachable from that crate's
//! `src/lib.rs` by following `mod`/`pub mod` declarations. A file that no module
//! tree reaches is dead code the compiler never sees — the exact defect class that
//! left insecure root stubs and unused traits sitting in this repo (issue #3).
//!
//! This module is the reusable engine; [`crate::find_unreachable_in_crate`] is the
//! entry point the workspace test in `tests/module_tree.rs` calls for each member.
//!
//! ## Scope and known limitations
//!
//! The declaration scanner is intentionally simple because the repo is simple, and
//! both facts below are asserted by the test suite rather than assumed:
//!
//! - **No `#[path]` attributes.** Declarations are resolved by name to `NAME.rs` or
//!   `NAME/mod.rs` relative to the declaring file's directory. `#[path = "..."]`
//!   would redirect that and is not honoured.
//! - **Only the `mod NAME;` (semicolon) form references a file.** Inline
//!   `mod NAME { ... }` blocks define modules in-place and are deliberately skipped.
//! - **`src/main.rs`** is treated as an additional root alongside `src/lib.rs`.
//!
//! When a declaration cannot be resolved to a file the checker fails loudly (the
//! path is reported as an error) rather than silently passing, so a future `#[path]`
//! or nested-module layout surfaces as a test failure to be handled deliberately.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Outcome of scanning one crate's `src/` tree.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// `.rs` files under `src/` that no module declaration reaches.
    pub unreachable: BTreeSet<PathBuf>,
    /// `mod` declarations that resolved to no file on disk.
    pub unresolved: BTreeSet<String>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.unreachable.is_empty() && self.unresolved.is_empty()
    }
}

/// Strip line (`//`) and block (`/* */`) comments so `mod` inside a comment is not
/// mistaken for a declaration. Deliberately naive: it does not track string literals,
/// which cannot contain a bare `mod NAME;` declaration anyway. Operates on `char`s so
/// multi-byte UTF-8 (e.g. box-drawing chars in doc comments) is handled correctly.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_line = false;
    let mut in_block = false;
    while let Some(c) = chars.next() {
        if in_line {
            if c == '\n' {
                in_line = false;
                out.push('\n');
            }
        } else if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            }
        } else if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            in_line = true;
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract the names declared by `mod NAME;` / `pub mod NAME;` (semicolon form only).
///
/// Only **top-level** declarations (brace depth 0) are considered file references:
/// - `mod NAME {` is an inline module, not a file — skipped.
/// - a `mod NAME;` nested inside an inline `mod X { ... }` block would reference
///   `X/NAME.rs`; this repo has no such layout, so those are ignored rather than
///   mis-resolved.
///
/// `cfg`-gated declarations (`#[cfg(test)] mod tests;`) still name a file and are
/// included — a test-only file is reachable, not dead.
fn declared_mods(src: &str) -> Vec<String> {
    let cleaned = strip_comments(src);
    let tokens = tokenize(&cleaned);
    let mut names = Vec::new();
    let mut depth: i32 = 0;
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            "mod" if depth == 0 => {
                // Expect `mod IDENT ;` — an inline `mod IDENT {` is not a file ref.
                if let (Some(name), Some(term)) = (tokens.get(i + 1), tokens.get(i + 2)) {
                    if is_ident(name) && term == ";" {
                        names.push(name.clone());
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    names
}

/// Split source into identifier tokens and the structural punctuation the module
/// scanner cares about (`; { }`). Everything else collapses to whitespace.
fn tokenize(src: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, tokens: &mut Vec<String>| {
        if !cur.is_empty() {
            tokens.push(std::mem::take(cur));
        }
    };
    for c in src.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            cur.push(c);
        } else {
            flush(&mut cur, &mut tokens);
            if c == ';' || c == '{' || c == '}' {
                tokens.push(c.to_string());
            }
        }
    }
    flush(&mut cur, &mut tokens);
    tokens
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.chars().next().unwrap().is_ascii_digit()
}

/// Resolve a `mod NAME;` declared inside `from` to the file it references, if any.
/// Tries `DIR/NAME.rs` then `DIR/NAME/mod.rs`, where `DIR` is `from`'s directory.
fn resolve_mod(from: &Path, name: &str) -> Option<PathBuf> {
    let dir = from.parent()?;
    let flat = dir.join(format!("{name}.rs"));
    if flat.is_file() {
        return Some(flat);
    }
    let nested = dir.join(name).join("mod.rs");
    if nested.is_file() {
        return Some(nested);
    }
    None
}

/// All `.rs` files anywhere under `src_dir`.
fn all_rs_files(src_dir: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut stack = vec![src_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.insert(path);
            }
        }
    }
    out
}

/// BFS the module tree of the crate rooted at `src_dir`, returning every file that
/// is reachable and every declaration that resolved to nothing.
fn reachable(src_dir: &Path) -> (BTreeSet<PathBuf>, BTreeSet<String>) {
    let mut visited = BTreeSet::new();
    let mut unresolved = BTreeSet::new();
    let mut queue: Vec<PathBuf> = Vec::new();

    for root in ["lib.rs", "main.rs"] {
        let p = src_dir.join(root);
        if p.is_file() {
            queue.push(p);
        }
    }

    while let Some(file) = queue.pop() {
        let file = file.canonicalize().unwrap_or(file);
        if !visited.insert(file.clone()) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        for name in declared_mods(&src) {
            match resolve_mod(&file, &name) {
                Some(target) => queue.push(target),
                None => {
                    // A nested inline module (`mod x { mod y; }`) referencing a sibling
                    // is out of scope for this repo; record unresolved so it fails loudly.
                    unresolved.insert(name);
                }
            }
        }
    }

    (visited, unresolved)
}

/// Scan one crate directory (the dir containing its `Cargo.toml`) and report any
/// `.rs` file under `src/` that the module tree does not reach.
pub fn find_unreachable_in_crate(crate_dir: &Path) -> Report {
    let src_dir = crate_dir.join("src");
    let mut report = Report::default();
    if !src_dir.is_dir() {
        return report;
    }

    let (visited, unresolved) = reachable(&src_dir);
    report.unresolved = unresolved;

    for file in all_rs_files(&src_dir) {
        let canon = file.canonicalize().unwrap_or_else(|_| file.clone());
        if !visited.contains(&canon) {
            report.unreachable.insert(file);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_declared_mods_ignoring_inline_and_comments() {
        let src = r#"
            mod alpha;
            pub mod beta;
            #[cfg(test)]
            mod gamma;
            // mod commented_out;
            /* mod also_commented; */
            mod inline { mod nested_inline; }
            fn f() { let _ = "mod not_a_decl"; }
        "#;
        let mods = declared_mods(src);
        assert!(mods.contains(&"alpha".to_string()));
        assert!(mods.contains(&"beta".to_string()));
        assert!(mods.contains(&"gamma".to_string()));
        assert!(!mods.contains(&"commented_out".to_string()));
        assert!(!mods.contains(&"also_commented".to_string()));
        // `mod inline { ... }` has no semicolon after the name, so it is not a file ref.
        assert!(!mods.contains(&"inline".to_string()));
    }

    #[test]
    fn ident_validation() {
        assert!(is_ident("foo_bar"));
        assert!(is_ident("_x"));
        assert!(!is_ident("2fast"));
        assert!(!is_ident("a::b"));
        assert!(!is_ident(""));
    }
}
