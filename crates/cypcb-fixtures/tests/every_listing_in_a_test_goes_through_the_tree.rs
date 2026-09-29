//! Every listing in a test goes through the tree.
//!
//! `cargo test -p cypcb-fixtures --test every_listing_in_a_test_goes_through_the_tree`
//!
//! A test that lists a directory of the repository from the disk passes in a
//! fresh clone and fails in a checkout that holds one more file. On 2026-09-27
//! 17 tests in 12 files did that, and each had been written against a clean
//! tree where the disk and git agree. `cypcb_fixtures::tree` answers from git,
//! and lists a directory a test wrote only when no tracked file is in it.
//!
//! This reads every test source - `crates/*/tests` and the `#[cfg(test)]`
//! items of `crates/*/src` - and allows no other directory listing in them:
//! no `std::fs::read_dir`, no `walkdir`, no `glob`. The helper itself is not
//! test code, so it is the one place the disk is listed.

use std::path::PathBuf;

use cypcb_fixtures::tree::{repo_root, tracked_under};

/// The names that list a directory. Built rather than written, so this file
/// does not have to be told apart from the ones it reads.
fn listing_names() -> Vec<String> {
    vec![
        ["read", "dir"].join("_"),
        ["walk", "dir"].concat(),
        ["Walk", "Dir"].concat(),
        "glob".to_owned(),
    ]
}

/// `source` with every comment, string literal and char literal blanked out,
/// line breaks kept, so a line number in the result is a line number in the
/// file and a name in prose is not a call.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            loop {
                if i >= chars.len() {
                    break;
                }
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[i]));
                    i += 1;
                }
            }
        } else if c == 'r'
            && (next == Some('"') || next == Some('#'))
            && !chars[..i]
                .last()
                .is_some_and(|p| p.is_alphanumeric() || *p == '_')
        {
            // A raw string: `r"..."`, `r#"..."#`, and `br` the same way.
            let mut j = i + 1;
            let mut hashes = 0;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            j += 1;
            loop {
                if j >= chars.len() {
                    break;
                }
                if chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                j += 1;
            }
            for &skipped in &chars[i..j.min(chars.len())] {
                out.push(blank(skipped));
            }
            i = j;
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                let step = if chars[i] == '\\' { 2 } else { 1 };
                for &skipped in &chars[i..(i + step).min(chars.len())] {
                    out.push(blank(skipped));
                }
                i += step;
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' {
            // A char literal is `'x'` or `'\..'`; anything else is a lifetime.
            let end = if next == Some('\\') {
                chars[i + 2..]
                    .iter()
                    .position(|&q| q == '\'')
                    .map(|at| i + 2 + at)
            } else if chars.get(i + 2) == Some(&'\'') {
                Some(i + 2)
            } else {
                None
            };
            match end {
                Some(end) => {
                    for _ in i..=end {
                        out.push(' ');
                    }
                    i = end + 1;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The byte ranges of `code` that only a test build compiles: each item under
/// a `cfg` that names `test` - `#[cfg(test)]`, `#[cfg(all(test, ..))]` - to its
/// closing brace or semicolon.
fn test_items(code: &str) -> Vec<(usize, usize)> {
    let bytes = code.as_bytes();
    let mut items = Vec::new();
    let mut from = 0;
    while let Some(at) = code[from..].find("#[cfg(") {
        let start = from + at;
        let Some(close) = code[start..].find(']') else {
            break;
        };
        let attribute = &code[start..start + close];
        let names_test = attribute
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|word| word == "test");
        if !names_test || attribute.contains("not(") {
            from = start + close;
            continue;
        }
        let mut i = start + close;
        let mut depth = 0usize;
        let end = loop {
            match bytes.get(i) {
                None => break bytes.len(),
                Some(b'{') => depth += 1,
                Some(b'}') => {
                    if depth <= 1 {
                        break i + 1;
                    }
                    depth -= 1;
                }
                Some(b';') if depth == 0 => break i + 1,
                _ => {}
            }
            i += 1;
        };
        items.push((start, end));
        from = end;
    }
    items
}

/// Each listing call in `code`, as (line, name). Only `ranges` are read when
/// they are given.
fn listings(code: &str, ranges: Option<&[(usize, usize)]>) -> Vec<(usize, String)> {
    let names = listing_names();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    for name in &names {
        let mut from = 0;
        while let Some(at) = code[from..].find(name.as_str()) {
            let start = from + at;
            let end = start + name.len();
            from = end;
            if start > 0 && is_ident(bytes[start - 1]) {
                continue;
            }
            if bytes.get(end).is_some_and(|&b| is_ident(b)) {
                continue;
            }
            if name == "glob" && !code[end..].trim_start().starts_with(['(', ':', '!']) {
                continue;
            }
            if ranges.is_some_and(|ranges| !ranges.iter().any(|&(a, b)| a <= start && start < b)) {
                continue;
            }
            let line = code[..start].matches('\n').count() + 1;
            found.push((line, name.clone()));
        }
    }
    found.sort();
    found
}

/// Where a test source lists a directory: the whole of a file under `tests/`,
/// only the `#[cfg(test)]` items of one under `src/`.
fn listings_in(relative: &str, source: &str) -> Vec<(usize, String)> {
    let code = code_only(source);
    if relative.split('/').nth(2) == Some("tests") {
        listings(&code, None)
    } else {
        listings(&code, Some(&test_items(&code)))
    }
}

/// Every Rust file under `crates/*/tests` and `crates/*/src`, as git tracks it.
fn test_sources() -> Vec<(String, PathBuf)> {
    let root = repo_root();
    tracked_under("crates")
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter_map(|path| {
            let relative = path
                .strip_prefix(&root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let place = relative.split('/').nth(2)?;
            matches!(place, "tests" | "src").then_some((relative, path))
        })
        .collect()
}

#[test]
fn every_listing_in_a_test_goes_through_the_tree() {
    let sources = test_sources();
    let mut test_files = 0usize;
    let mut src_files = 0usize;
    let mut offenders = Vec::new();
    for (relative, path) in &sources {
        let source = std::fs::read_to_string(path).expect("a tracked source reads");
        if relative.split('/').nth(2) == Some("tests") {
            test_files += 1;
        } else if !test_items(&code_only(&source)).is_empty() {
            src_files += 1;
        }
        for (line, name) in listings_in(relative, &source) {
            offenders.push(format!("{relative}:{line}: {name}"));
        }
    }
    eprintln!(
        "{test_files} test files and {src_files} source files with test items read, {} listings outside the tree",
        offenders.len()
    );
    assert!(
        offenders.is_empty(),
        "these list a directory from the disk - use cypcb_fixtures::tree \
         (tracked_in, tracked_under, tracked_dirs_in, or written_entries for a directory the test wrote):\n{}",
        offenders.join("\n")
    );
    assert!(
        test_files >= 300 && src_files >= 100,
        "only {test_files} test files and {src_files} source files were read, so this is not reading the tree it thinks it is"
    );
}

#[test]
fn a_listing_in_code_is_found_and_one_in_prose_is_not() {
    let call = ["std::fs::", "read", "_dir", "(&dir)"].concat();
    let source = format!(
        "fn walk(dir: &Path) {{\n    // {call} in a comment\n    let text = \"{call}\";\n    let c = '\"';\n    let n = {call};\n}}\n"
    );
    assert_eq!(
        listings_in("crates/x/tests/a.rs", &source),
        [(5, ["read", "_dir"].concat())],
        "{source}"
    );

    let raw =
        format!("let text = r#\"{call}\"#;\nlet life: &'static str = \"x\";\nlet n = {call};\n");
    assert_eq!(listings_in("crates/x/tests/a.rs", &raw).len(), 1, "{raw}");

    let crates = ["walk", "dir::", "Walk", "Dir::new(dir)"].concat();
    assert_eq!(
        listings_in("crates/x/tests/a.rs", &crates).len(),
        2,
        "{crates}"
    );

    let globbed = ["let files = ", "glob", "(\"*.rs\");\nlet glob_count = 1;\n"].concat();
    assert_eq!(
        listings_in("crates/x/tests/a.rs", &globbed).len(),
        1,
        "{globbed}"
    );
}

#[test]
fn a_listing_in_shipped_code_is_left_alone_and_one_in_a_test_item_is_not() {
    let call = ["std::fs::", "read", "_dir", "(&dir)"].concat();
    let source = format!(
        "pub fn load(dir: &Path) {{\n    let n = {call};\n}}\n\n#[cfg(test)]\nmod tests {{\n    fn walk() {{\n        let n = {call};\n    }}\n}}\n\nfn after() {{\n    let n = {call};\n}}\n"
    );
    assert_eq!(
        listings_in("crates/x/src/lib.rs", &source),
        [(8, ["read", "_dir"].concat())],
        "{source}"
    );
}

#[test]
fn a_test_item_under_a_wider_cfg_is_read_too() {
    let call = ["read", "_dir", "(&dir)"].concat();
    let source = format!(
        "#[cfg(all(test, feature = \"native\"))]\nmod tests {{\n    fn f() {{ {call}; }}\n}}\n#[cfg(not(test))]\nfn g() {{ {call}; }}\n"
    );
    assert_eq!(
        listings_in("crates/x/src/lib.rs", &source).len(),
        1,
        "{source}"
    );
}

#[test]
fn a_file_under_tests_is_read_whole() {
    let call = ["read", "_dir", "(&dir)"].concat();
    let source = format!("fn helper() {{\n    let n = {call};\n}}\n");
    assert_eq!(
        listings_in("crates/x/tests/common/mod.rs", &source).len(),
        1
    );
    assert!(listings_in("crates/x/src/lib.rs", &source).is_empty());
}
