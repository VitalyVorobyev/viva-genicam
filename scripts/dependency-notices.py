#!/usr/bin/env python3
"""Collect the installed license/notice texts for one distribution's dependency graph.

Uses locked Cargo metadata for the requested target, excluding dev-only edges.
Studio also includes its installed production npm graph. Build dependencies are
included conservatively: this is a source dependency inventory, not a claim that
every listed crate survives linking. Run after fetching dependencies and before
packaging. Missing license texts fail the build rather than silently disappearing.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from urllib.request import Request, urlopen
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = {
    "cli": ("Cargo.toml", ["viva-camctl", "viva-service"]),
    "gentl": ("Cargo.toml", ["viva-gentl"]),
    "python": ("crates/viva-pygenicam/Cargo.toml", ["viva-pygenicam"]),
    "studio": ("studio/apps/viva-studio-tauri/src-tauri/Cargo.toml", ["viva_studio_tauri"]),
}
# Reviewed published manifests offer Apache-2.0, but neither their crate archive
# nor the repository root at their VCS revision contains a full license file.
# Preserve the manifest attribution and supply the unmodified standard terms.
APACHE_SUPPLEMENTS = {("keyed-set", "1.1.0"), ("token-cell", "2.1.1"),
                      ("validated_struct", "2.2.0"), ("validated_struct_macros", "2.2.0")}
MPL_SUPPLEMENTS = {("selectors", "0.38.0")}
REPOSITORY_OVERRIDES = {
    # The sys crate omits repository metadata; the wrapper crate at the same
    # revision identifies this upstream workspace (sys/ in path_in_vcs).
    ("libappindicator-sys", "0.9.0"): "https://github.com/tauri-apps/libappindicator-rs",
}


def license_files(directory):
    """Include nested vendored-library notices as well as package-root terms."""
    files = []
    for path in directory.rglob("*"):
        if not path.is_file():
            continue
        name = path.name.lower()
        if path.suffix.lower() in {".rs", ".py", ".c", ".h", ".cpp", ".js", ".ts"}:
            continue
        in_license_directory = any(part.lower() in {"licenses", "licences"}
                                   for part in path.relative_to(directory).parts[:-1])
        if in_license_directory or name.startswith(("license", "licence", "copying", "notice", "copyright")):
            files.append(path)
    return sorted(files)


def texts(directory, local=False):
    # Path dependencies here are our own crates: their tests/fixtures are not
    # independently bundled libraries. External crates need recursive coverage.
    files = [] if local else license_files(directory)
    if local and not files:
        files = [ROOT / "LICENSE"]
    if not files:
        raise ValueError(f"No license text in {directory}; review upstream terms before shipping")
    return [(p.relative_to(directory).as_posix() if p.is_relative_to(directory) else "workspace LICENSE",
             p.read_text(encoding="utf-8", errors="strict")) for p in files]


def upstream_texts(package, directory):
    """Some crates omit workspace license files; fetch their exact VCS revision."""
    vcs = json.loads((directory / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
    revision = vcs["git"]["sha1"]
    key = (package["name"], package["version"])
    repository = (package.get("repository") or REPOSITORY_OVERRIDES.get(key, "")).removesuffix(".git")
    if key in APACHE_SUPPLEMENTS | MPL_SUPPLEMENTS:
        apache = key in APACHE_SUPPLEMENTS
        expected = "EPL-2.0 OR Apache-2.0" if apache else "MPL-2.0"
        selected = "Apache-2.0" if apache else "MPL-2.0"
        if package.get("license") != expected:
            raise ValueError("Reviewed supplementary license declaration changed")
        attribution = (f"Selected {selected} declared by {package['name']} {package['version']}.\n"
                       f"Authors declared in Cargo.toml: {', '.join(package.get('authors', []))}\n"
                       f"Source revision: {repository}/tree/{revision}\n"
                       "Full terms omitted from the packaged source and upstream root.\n")
        copyright_lines = sorted({line.strip() for path in directory.rglob("*.rs")
                                  for line in path.read_text(encoding="utf-8", errors="replace").splitlines()
                                  if "copyright" in line.lower()})
        attribution += "\n".join(copyright_lines) + "\n"
        return [("Manifest attribution", attribution),
                ("https://www.apache.org/licenses/LICENSE-2.0.txt" if apache else "https://www.mozilla.org/MPL/2.0/",
                 (ROOT / f"docs/third-party/{selected}.txt").read_text(encoding="utf-8"))]
    parsed = urlparse(repository)
    if parsed.hostname != "github.com" or len(parsed.path.strip("/").split("/")) != 2:
        raise ValueError(f"Missing packaged license: {package['name']}; unsupported pinned source {repository}")
    slug = parsed.path.strip("/")
    cache = ROOT / "target/legal/upstream" / slug / revision
    index = cache / "index.json"
    if not index.exists():
        def fetch(url):
            headers = {"User-Agent": "viva-genicam-license-inventory"}
            if urlparse(url).hostname == "api.github.com" and os.environ.get("GITHUB_TOKEN"):
                headers["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
            with urlopen(Request(url, headers=headers), timeout=30) as response:
                return response.read()
        tree = json.loads(fetch(f"https://api.github.com/repos/{slug}/git/trees/{revision}"))
        entries = [e for e in tree["tree"] if e["type"] == "blob" and
                   e["path"].lower().startswith(("license", "licence", "copying", "notice", "copyright", "authors"))]
        if not entries:
            raise ValueError(f"No root license files at {repository}/tree/{revision}; manual review needed")
        cache.mkdir(parents=True, exist_ok=True)
        for entry in entries:
            (cache / entry["path"]).write_bytes(fetch(
                f"https://raw.githubusercontent.com/{slug}/{revision}/{entry['path']}"))
        index.write_text(json.dumps([e["path"] for e in entries]), encoding="utf-8")
    return [(f"{repository}/blob/{revision}/{name}", (cache / name).read_text(encoding="utf-8"))
            for name in json.loads(index.read_text(encoding="utf-8"))]


def rust_inventory(artifact, target):
    manifest, roots = ARTIFACTS[artifact]
    command = ["cargo", "metadata", "--locked", "--format-version", "1",
               "--manifest-path", str(ROOT / manifest), "--filter-platform", target]
    if artifact == "python":
        command += ["--features", "pyo3/extension-module"]
    metadata = json.loads(subprocess.check_output(command, cwd=ROOT))
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {p["id"]: p for p in metadata["resolve"]["nodes"]}
    pending = [p["id"] for p in packages.values() if p["name"] in roots]
    if len(pending) != len(roots):
        raise ValueError(f"Cannot resolve distribution roots {roots}")
    visited = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        for dep in nodes[package_id]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])
    result = []
    for package_id in sorted(visited, key=lambda i: (packages[i]["name"], packages[i]["version"])):
        p = packages[package_id]
        directory = Path(p["manifest_path"]).parent
        try:
            licenses = texts(directory, local=p["source"] is None)
        except ValueError:
            licenses = upstream_texts(p, directory)
        result.append((f"Rust: {p['name']} {p['version']}", p.get("license") or "See license text",
                       p.get("repository") or p.get("source") or "workspace",
                       licenses))
    return result


def npm_inventory():
    frontend = ROOT / "studio/ui/viva-studio-ui"
    pending = [(frontend, json.loads((frontend / "package.json").read_text(encoding="utf-8")))]
    visited = set()
    result = []
    while pending:
        parent, package = pending.pop()
        for name in sorted(package.get("dependencies", {})):
            directory = parent
            while not (directory / "node_modules" / name / "package.json").is_file():
                if directory == directory.parent:
                    raise ValueError(f"Install UI dependencies first: missing {name}")
                directory = directory.parent
            directory = (directory / "node_modules" / name).resolve()
            if directory in visited:
                continue
            visited.add(directory)
            dependency = json.loads((directory / "package.json").read_text(encoding="utf-8"))
            result.append((f"JavaScript: {dependency['name']} {dependency['version']}",
                           dependency.get("license", "See license text"),
                           str(dependency.get("repository", "npm")), texts(directory)))
            pending.append((directory, dependency))
    return sorted(result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact", choices=ARTIFACTS, required=True)
    parser.add_argument("--target", help="Rust target triple; defaults to rustc's host")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    target = args.target or next(line.removeprefix("host: ") for line in
                                subprocess.check_output(["rustc", "-vV"], text=True).splitlines()
                                if line.startswith("host: "))
    inventory = rust_inventory(args.artifact, target)
    if args.artifact == "studio":
        inventory += npm_inventory()
    output = [f"THIRD-PARTY NOTICES — viva-genicam {args.artifact} ({target})\n",
              "Generated from locked, target-filtered dependency metadata and installed package license files.\n"
              "Includes normal and build dependencies; excludes dev-only dependency edges.\n"
              "License expressions are upstream declarations; reproduced alternative texts do not change license choices.\n"
              "OS/framework libraries remain subject to their own terms.\n"]
    if args.artifact == "python":
        output.append("NumPy is an independently installed runtime dependency, not bundled by this wheel.\n"
                      "Its installed distribution carries its own license and bundled-library notices.\n")
    for name, license_id, source, files in inventory:
        output.append(f"\n{'=' * 72}\n{name}\nLicense: {license_id}\nSource: {source}\n")
        for name, content in files:
            output.append(f"\n--- {name} ---\n{content.rstrip()}\n")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text("\n".join(output), encoding="utf-8")
    print(f"{args.out}: {len(inventory)} packages", file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
