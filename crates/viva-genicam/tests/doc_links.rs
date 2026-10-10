//! Every relative Markdown link resolves (backlog CI-17).
//!
//! mdBook checks its `{{#include}}`s (`book_includes`) but not plain links, and
//! nothing checked the Markdown outside the book: links to files moved under
//! `docs/` went stale for weeks unnoticed. This reads the documentation a
//! reader follows — the top-level Markdown, `book/src`, `docs/`, `studio/` and
//! each crate's README — and fails on a relative link whose target does not
//! exist. Web links are not checked: that needs the network, and they change
//! under us.

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

/// Directories that hold generated or third-party Markdown.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", ".git", "book/book"];

fn markdown_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if SKIP_DIRS
            .iter()
            .any(|skip| relative.ends_with(skip) || relative.starts_with(skip))
        {
            continue;
        }
        if path.is_dir() {
            markdown_files(root, &path, out);
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
        if let Some(rest) = line.trim_start().strip_prefix('[')
            && let Some((_, target)) = rest.split_once("]: ")
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
    for top in ["README.md", "CONTRIBUTING.md", "CHANGELOG.md", "CLAUDE.md"] {
        files.push(root.join(top));
    }
    for dir in ["book/src", "docs", "studio", "crates"] {
        markdown_files(&root, &root.join(dir), &mut files);
    }

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
    let md = "See [a](a.md) and [b](b.md#x \"t\").\n```\n[c](c.md)\n```\n`[d](d.md)`\n[e]: e.md\n";
    assert_eq!(link_targets(md), vec!["a.md", "b.md#x", "e.md"]);
}
