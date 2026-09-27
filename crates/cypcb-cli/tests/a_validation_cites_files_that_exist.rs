//! A milestone validation names files that are there, or says they are not.
//!
//! `cargo test -p cypcb-cli --test a_validation_cites_files_that_exist`
//!
//! `.gsd/milestones/*/M0*-VALIDATION.md` is the document that says a milestone
//! was delivered, and M005's ticked five criteria while citing six files no
//! commit in this clone ever added - the Web Worker that was supposed to take
//! routing off the main thread, and the two E2E suites said to prove it. The
//! quote that gave it away was arithmetic: the document reports
//! ``grep "engine\.auto_route" main.ts`` returning 0, and the same grep here
//! returns 2.
//!
//! So a file named in backticks in one of those documents has to be a file
//! this repository has, or has to appear in that document's own
//! `not_in_this_repository:` block with what happened to it. A name is matched
//! on its basename because that is how these documents write them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cypcb_fixtures::tree::{tracked_dirs_in, tracked_in, tracked_under};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels below the repo root")
        .to_path_buf()
}

/// Every file name in the repository, ignoring what a build put there.
fn basenames(root: &Path) -> BTreeSet<String> {
    let skip = ["target", "node_modules", ".git", "dist", "test-results"];
    tracked_under(root)
        .into_iter()
        .filter(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .parent()
                .is_none_or(|dirs| {
                    dirs.components()
                        .all(|part| !skip.iter().any(|dir| part.as_os_str() == *dir))
                })
        })
        .filter_map(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
        })
        .collect()
}

/// The validation documents, one per milestone that has one.
fn validations(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let milestones = root.join(".gsd").join("milestones");
    for dir in tracked_dirs_in(&milestones) {
        for path in tracked_in(&dir) {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("-VALIDATION.md"))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Every backticked file name in the text: a run between backticks that has an
/// extension this project writes and no spaces in it.
fn cited(text: &str) -> BTreeSet<String> {
    let extensions = [".ts", ".rs", ".cypcb", ".wasm", ".html"];
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else { break };
        let token = &rest[..close];
        rest = &rest[close + 1..];
        if token.contains(char::is_whitespace) || token.contains('(') {
            continue;
        }
        if extensions.iter().any(|ext| token.ends_with(ext)) {
            let name = token.rsplit('/').next().unwrap_or(token);
            names.insert(name.to_string());
        }
    }
    names
}

/// The items of a `not_in_this_repository:` block: the name, then the reason.
fn annotations(text: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim_end() == "not_in_this_repository:" {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        match line.strip_prefix("  - ") {
            Some(item) => match item.split_once(" - ") {
                Some((name, why)) => rows.push((name.trim().to_string(), why.trim().to_string())),
                None => rows.push((item.trim().to_string(), String::new())),
            },
            None => break,
        }
    }
    rows
}

#[test]
fn a_cited_file_is_in_the_tree_or_says_why_not() {
    let root = repo_root();
    let names = basenames(&root);
    let documents = validations(&root);
    assert!(
        documents.len() >= 2,
        "M004 and M005 both have a validation document and this run found {}",
        documents.len()
    );

    let mut cited_total = 0usize;
    let mut unexplained: Vec<String> = Vec::new();
    for document in &documents {
        let text = std::fs::read_to_string(document).expect("a validation document is readable");
        let excused: BTreeSet<String> = annotations(&text)
            .into_iter()
            .filter(|(_, why)| !why.is_empty())
            .map(|(name, _)| name)
            .collect();
        for name in cited(&text) {
            cited_total += 1;
            if names.contains(&name) || excused.contains(&name) {
                continue;
            }
            unexplained.push(format!(
                "{} cites {name} and no such file is in the tree",
                document
                    .strip_prefix(&root)
                    .unwrap_or(document.as_path())
                    .display()
            ));
        }
    }

    assert!(
        cited_total >= 20,
        "the two documents cite 26 files and this run read {cited_total}, so the reader stopped early"
    );
    assert!(
        unexplained.is_empty(),
        "a milestone was signed off on files nobody can open:\n{}",
        unexplained.join("\n")
    );
}

#[test]
fn an_excuse_is_about_a_file_that_is_really_missing() {
    let root = repo_root();
    let names = basenames(&root);
    let mut stale: Vec<String> = Vec::new();

    for document in validations(&root) {
        let text = std::fs::read_to_string(&document).expect("a validation document is readable");
        let at = document
            .strip_prefix(&root)
            .unwrap_or(document.as_path())
            .display()
            .to_string();
        let cites = cited(&text);
        for (name, why) in annotations(&text) {
            if why.is_empty() {
                stale.push(format!("{at} excuses {name} without saying why"));
            }
            if names.contains(&name) {
                stale.push(format!("{at} excuses {name} and it is in the tree"));
            }
            if !cites.contains(&name) {
                stale.push(format!("{at} excuses {name}, which it never cites"));
            }
        }
    }

    assert!(stale.is_empty(), "{}", stale.join("\n"));
}
