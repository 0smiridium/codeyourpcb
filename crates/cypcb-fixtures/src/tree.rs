//! The files this repository holds, as git answers them.
//!
//! A test that walks the disk to find the examples, the fixtures or the
//! sources reads whatever else is lying in the checkout it runs in. The main
//! checkout carries ignored trees under `viewer/`, and on 2026-09-27 one of
//! them turned a footprint check red there and nowhere else. A file planted
//! beside the tracked ones in every tracked directory then failed 17 tests in
//! 12 files, every one of them green on a fresh clone. What the repository
//! holds is what git answers, whatever else is on the disk.
//!
//! So a test that reads the repository lists it here: [`tracked_under`],
//! [`tracked_in`] and [`tracked_dirs_in`]. A test that lists a directory it
//! wrote itself uses [`written_entries`], which refuses a directory that holds
//! a tracked file, so the disk is not read by the back door.
//! `every_listing_in_a_test_goes_through_the_tree` keeps every other directory
//! listing out of the test sources.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The root of this checkout, two directories above this crate.
pub fn repo_root() -> PathBuf {
    normalise(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// Every file git tracks in the checkout at `root`, as a path relative to it.
pub fn files_git_tracks(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .expect("git ls-files: the suite runs from a checkout, so git must answer");
    assert!(
        output.status.success(),
        "git ls-files failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The tracked files of this checkout, asked once per test binary.
fn tracked() -> &'static [String] {
    static TRACKED: OnceLock<Vec<String>> = OnceLock::new();
    TRACKED.get_or_init(|| files_git_tracks(&repo_root()))
}

/// Every tracked file under `dir`, at any depth, sorted.
///
/// `dir` is relative to the repository root, or absolute inside it.
pub fn tracked_under(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let prefix = prefix_of(dir.as_ref());
    let root = repo_root();
    let mut found: Vec<PathBuf> = tracked()
        .iter()
        .filter(|file| file.starts_with(&prefix))
        .map(|file| root.join(file))
        .collect();
    found.sort();
    found
}

/// The tracked files directly in `dir`, not in its subdirectories, sorted.
pub fn tracked_in(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let prefix = prefix_of(dir.as_ref());
    let root = repo_root();
    let mut found: Vec<PathBuf> = tracked()
        .iter()
        .filter(|file| {
            file.strip_prefix(prefix.as_str())
                .is_some_and(|rest| !rest.contains('/'))
        })
        .map(|file| root.join(file))
        .collect();
    found.sort();
    found
}

/// The directories directly in `dir` that hold a tracked file, sorted.
pub fn tracked_dirs_in(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let prefix = prefix_of(dir.as_ref());
    let root = repo_root();
    let mut found: Vec<PathBuf> = tracked()
        .iter()
        .filter_map(|file| {
            let rest = file.strip_prefix(prefix.as_str())?;
            let (name, _) = rest.split_once('/')?;
            Some(root.join(&prefix).join(name))
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// [`std::fs::read_dir`] for a directory a test wrote.
///
/// A directory that holds a tracked file is the repository, and listing it
/// here would read the disk rather than git: that panics and names the
/// function to use instead.
pub fn written_entries(dir: impl AsRef<Path>) -> io::Result<std::fs::ReadDir> {
    let dir = dir.as_ref();
    let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    if let Some(inside) = relative_to_root(&absolute) {
        let prefix = with_slash(&inside);
        assert!(
            !tracked().iter().any(|file| file.starts_with(&prefix)),
            "{} holds files git tracks - list it with tracked_in or tracked_under, not from the disk",
            dir.display()
        );
    }
    std::fs::read_dir(dir)
}

/// `dir` as a prefix of git's paths: relative to the root, `/` at the end,
/// empty for the root itself.
fn prefix_of(dir: &Path) -> String {
    let inside = if dir.is_absolute() {
        relative_to_root(dir).unwrap_or_else(|| {
            panic!(
                "{} is not inside the repository at {}",
                dir.display(),
                repo_root().display()
            )
        })
    } else {
        normalise(dir)
    };
    with_slash(&inside)
}

fn with_slash(inside: &Path) -> String {
    let text = inside
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if text.is_empty() {
        text
    } else {
        format!("{text}/")
    }
}

/// An absolute path as a path from the repository root, if it is inside it.
///
/// Compared as written first, then with links resolved: a test that
/// canonicalised its own root hands in a path the written root is not a
/// prefix of.
fn relative_to_root(path: &Path) -> Option<PathBuf> {
    let root = repo_root();
    if let Ok(inside) = normalise(path).strip_prefix(&root) {
        return Some(inside.to_path_buf());
    }
    let root = root.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    path.strip_prefix(&root).ok().map(Path::to_path_buf)
}

/// `.` and `..` taken out without asking the disk.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_examples_are_listed_from_git() {
        let examples = tracked_in("examples");
        assert!(
            examples
                .iter()
                .any(|path| path.ends_with("examples/blink.cypcb")),
            "{examples:?}"
        );
        assert!(examples
            .iter()
            .all(|path| path.parent() == Some(&repo_root().join("examples"))));
        let deeper = tracked_under("examples");
        assert!(
            deeper.len() > examples.len(),
            "examples/lib is under examples"
        );
        assert!(tracked_dirs_in("examples").contains(&repo_root().join("examples/lib")));
    }

    #[test]
    fn an_absolute_path_and_a_relative_one_list_the_same() {
        let from_here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        assert_eq!(tracked_in(&from_here), tracked_in("examples"));
    }

    #[test]
    fn a_file_on_the_disk_that_git_does_not_track_is_not_listed() {
        let dir = crate::scratch_dir("cypcb-tree-selftest");
        let git = |args: &[&str]| {
            let run = Command::new("git")
                .args(args)
                .current_dir(&*dir)
                .output()
                .expect("git runs");
            assert!(
                run.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&run.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::create_dir_all(dir.join("examples")).expect("a directory");
        std::fs::write(dir.join("examples/kept.cypcb"), "x").expect("a tracked file");
        git(&["add", "examples/kept.cypcb"]);
        std::fs::write(dir.join("examples/stray.cypcb"), "x").expect("an untracked file");
        assert_eq!(files_git_tracks(&dir), ["examples/kept.cypcb"]);
    }

    #[test]
    #[should_panic(expected = "holds files git tracks")]
    fn a_tracked_directory_is_not_listed_as_written() {
        let _ = written_entries(repo_root().join("examples"));
    }

    #[test]
    fn a_directory_a_test_wrote_is_listed() {
        let dir = crate::scratch_dir("cypcb-tree-selftest");
        std::fs::write(dir.join("out.gbr"), "x").expect("a written file");
        let names: Vec<_> = written_entries(&*dir)
            .expect("the directory reads")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(names, ["out.gbr"]);
    }
}
