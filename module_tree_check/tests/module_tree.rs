//! Workspace-wide module-tree guard, run as part of `cargo test --workspace` (which
//! is what CI runs — see `.github/workflows/ci.yml`). No pipeline change is needed:
//! this test fails the build the moment an unreachable `.rs` file appears under any
//! member's `src/`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use module_tree_check::{find_unreachable_in_crate, Report};

/// Locate the workspace root by walking up from this crate until a `Cargo.toml`
/// containing a `[workspace]` table is found.
fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            if let Ok(text) = fs::read_to_string(&manifest) {
                if text.contains("[workspace]") {
                    return dir;
                }
            }
        }
        if !dir.pop() {
            panic!("could not locate workspace root from CARGO_MANIFEST_DIR");
        }
    }
}

/// Parse the `members = [ ... ]` array from the workspace `Cargo.toml`. Good enough
/// for this repo's plain single-line-per-entry list; not a general TOML parser.
fn workspace_members(root: &Path) -> Vec<String> {
    let text = fs::read_to_string(root.join("Cargo.toml")).expect("read workspace Cargo.toml");
    let start = text.find("members").expect("members key");
    let open = text[start..].find('[').expect("members open bracket") + start;
    let close = text[open..].find(']').expect("members close bracket") + open;
    text[open + 1..close]
        .split(',')
        .filter_map(|raw| {
            let t = raw.trim().trim_matches('"').trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
        .collect()
}

#[test]
fn every_workspace_member_has_a_fully_reachable_module_tree() {
    let root = workspace_root();
    let members = workspace_members(&root);
    assert!(!members.is_empty(), "parsed zero workspace members");

    let mut failures = Vec::new();
    for member in &members {
        let report = find_unreachable_in_crate(&root.join(member));
        if !report.is_clean() {
            failures.push((member.clone(), report));
        }
    }

    if !failures.is_empty() {
        let mut msg = String::from("module-tree check failed:\n");
        for (member, report) in &failures {
            for f in &report.unreachable {
                let rel = f.strip_prefix(&root).unwrap_or(f);
                msg.push_str(&format!(
                    "  [{member}] unreachable (declared by no `mod`): {}\n",
                    rel.display()
                ));
            }
            for u in &report.unresolved {
                msg.push_str(&format!(
                    "  [{member}] `mod {u};` resolves to no file (unsupported layout?)\n"
                ));
            }
        }
        panic!("{msg}");
    }
}

/// Permanent fixture: build a synthetic crate on disk with one declared and one
/// *undeclared* file, and assert the detector flags exactly the undeclared one.
/// This keeps the acceptance proof in the suite rather than as a one-off manual demo.
#[test]
fn detector_flags_an_undeclared_file_in_a_synthetic_crate() {
    let base = std::env::temp_dir().join(format!("mtc_fixture_{}", std::process::id()));
    let src = base.join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(src.join("lib.rs"), "pub mod declared;\n").unwrap();
    fs::write(src.join("declared.rs"), "pub fn ok() {}\n").unwrap();
    fs::write(src.join("orphan.rs"), "pub fn dead() {}\n").unwrap();

    let report: Report = find_unreachable_in_crate(&base);

    let flagged: BTreeSet<String> = report
        .unreachable
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    fs::remove_dir_all(&base).ok();

    assert!(
        flagged.contains("orphan.rs"),
        "expected orphan.rs to be flagged, got {flagged:?}"
    );
    assert!(
        !flagged.contains("declared.rs"),
        "declared.rs must not be flagged"
    );
    assert!(
        !flagged.contains("lib.rs"),
        "the crate root must not be flagged"
    );
}
