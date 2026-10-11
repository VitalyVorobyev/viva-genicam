//! Every relative Markdown link resolves (backlog CI-17).
//!
//! mdBook checks its `{{#include}}`s (`book_includes`) but not plain links, and
//! nothing checked the Markdown outside the book: links to files moved under
//! `docs/` went stale for weeks unnoticed. This reads every Markdown file in
//! the repository and fails on a relative link whose target does not exist.
//! Web links are not checked: that needs the network, and they change under us.
//!
//! It walks the whole tree rather than a list of directories because a list
//! goes stale the same way the links did: `changelog/`, `.github/` and
//! `.claude/skills/` all held Markdown that a fixed list never read.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/viva-genicam.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Book pages link into the rustdoc that `publish-docs.yml` copies into the
/// published site's `api/`, which exists only once published.
const PUBLISHED_ONLY: &[(&str, &str)] = &[("book/src", "api/")];

/// Directories that hold generated, fetched or third-party Markdown, matched
/// against the end of the path from the repository root: a single name
/// (`target`) at any depth, a longer path (`book/book`) where it appears.
/// `.claude/worktrees` holds whole checkouts of other branches.
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "book/book",
    "fixtures/vendor-xml",
    ".claude/worktrees",
];

/// Hidden directories are tool state (`.git`, `.venv`, `.codegraph`,
/// `.pytest_cache`) and are skipped — except these, which hold Markdown a
/// contributor follows: issue templates and the project skills.
const HIDDEN_DIRS_READ: &[&str] = &[".github", ".claude"];

fn skipped_dir(relative: &Path) -> bool {
    let name = relative.file_name().and_then(|n| n.to_str()).unwrap_or("");
    (name.starts_with('.') && !HIDDEN_DIRS_READ.contains(&name))
        || SKIP_DIRS.iter().any(|skip| relative.ends_with(skip))
}

fn markdown_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `file_type` does not follow symlinks, so a link cycle cannot recurse.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if !skipped_dir(path.strip_prefix(root).unwrap_or(&path)) {
                markdown_files(root, &path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
}

/// The targets of `[text](target)` and `[label]: target` links outside code
/// fences and inline code.
fn link_targets(markdown: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Drop inline code spans.
        let line: String = line
            .split('`')
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, part)| part)
            .collect::<Vec<_>>()
            .join(" ");
        // A reference definition: `[label]:` and the target, after optional
        // whitespace (Codex review of #188).
        if let Some(rest) = line.trim_start().strip_prefix('[')
            && let Some((_, target)) = rest.split_once("]:")
        {
            targets.push(target.split_whitespace().next().unwrap_or("").to_string());
        }
        let mut rest = line.as_str();
        while let Some(start) = rest.find("](") {
            let after = &rest[start + 2..];
            let end = after.find(')').unwrap_or(after.len());
            let target = after[..end].split_whitespace().next().unwrap_or("");
            targets.push(target.trim_matches(['<', '>']).to_string());
            rest = &after[end..];
        }
    }
    targets
}

/// Whether `target` is a link this test can check: relative, to a file.
fn is_relative_file_link(target: &str) -> bool {
    !(target.is_empty()
        || target.starts_with('#')
        || target.contains("://")
        || target.starts_with("mailto:")
        || target.starts_with("{{"))
}

#[test]
fn every_relative_markdown_link_resolves() {
    let root = repo_root();
    let mut files = Vec::new();
    markdown_files(&root, &root, &mut files);
    files.sort();
    // A walk that silently found nothing would pass vacuously.
    assert!(
        files.iter().any(|f| f.ends_with("README.md")),
        "the walk from {} found no README.md",
        root.display()
    );

    let mut broken = Vec::new();
    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let base = file.parent().expect("a file has a parent");
        for target in link_targets(&text) {
            if !is_relative_file_link(&target) {
                continue;
            }
            let path = target.split(['#', '?']).next().unwrap_or_default();
            if path.is_empty() {
                continue;
            }
            let relative_file = file.strip_prefix(&root).unwrap_or(file);
            if PUBLISHED_ONLY
                .iter()
                .any(|(dir, prefix)| relative_file.starts_with(dir) && path.starts_with(prefix))
            {
                continue;
            }
            let resolved = match path.strip_prefix('/') {
                Some(from_root) => root.join(from_root),
                None => base.join(path),
            };
            if !resolved.exists() {
                broken.push(format!(
                    "{}: {target}",
                    file.strip_prefix(&root).unwrap_or(file).display()
                ));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "{} relative links point at nothing:\n  {}",
        broken.len(),
        broken.join("\n  ")
    );
}

#[test]
fn link_targets_are_found_outside_code() {
    let md = "See [a](a.md) and [b](b.md#x \"t\").\n```\n[c](c.md)\n```\n`[d](d.md)`\n[e]: e.md\n[f]:f.md\n[g]:\tg.md\n";
    assert_eq!(
        link_targets(md),
        vec!["a.md", "b.md#x", "e.md", "f.md", "g.md"]
    );
}
