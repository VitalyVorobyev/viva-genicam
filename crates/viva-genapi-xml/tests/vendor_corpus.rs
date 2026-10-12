//! Conformance test against real-camera GenICam XML descriptions.
//!
//! Our fake cameras and hand-written fixtures only exercise the constructs we
//! already thought of. Real vendor documents are where the surprises live —
//! issue #45 (a CDATA-wrapped formula) and issue #35 (a constant-formula
//! SwissKnife) were both found in the field, on cameras we cannot buy, after
//! users had already been blocked by them.
//!
//! The corpus is not committed: the documents are vendor copyright, published
//! for interoperability by third-party projects, so we fetch rather than
//! redistribute. Populate it with:
//!
//! ```sh
//! scripts/fetch-xml-corpus.sh
//! cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture
//! ```
//!
//! Set `VIVA_GENICAM_XML_CORPUS` to test a different directory — point it at
//! XML dumped from your own hardware to check a camera before you own it in
//! production.
//!
//! When the directory is absent the test passes with a note, so CI and a fresh
//! clone stay green.

use std::path::{Path, PathBuf};

/// Default corpus location relative to the workspace root.
const DEFAULT_CORPUS: &str = "fixtures/vendor-xml";

/// Nodes we knowingly cannot represent yet, as `(document, node name)`.
///
/// Everything else that fails to parse is a regression. Keep this list short
/// and each entry tied to a `docs/backlog.md` task.
const EXPECTED_SKIPS: &[(&str, &str)] = &[];

/// Node types we cannot represent yet, as `(tag, required error substring)`.
///
/// Distinct from [`EXPECTED_SKIPS`], which allows one named declaration we
/// cannot parse. An entry here says a tag is unimplemented *for a specific
/// stated reason*, and the substring is what pins the reason down: a
/// `<Register>` skipped because of `<pLength>` is a known gap, while a
/// `<Register>` skipped for anything else is a regression this test must still
/// catch. A tag not listed here fails outright.
///
/// Use `""` as the substring to allow a tag unconditionally.
const EXPECTED_SKIP_REASONS: &[(&str, &str)] = &[
    // GA-09 phase two: `<pLength>`, a register length resolved from another
    // node at runtime. 21 of the corpus's 63 `<Register>` declarations use it;
    // the other 42 are supported and must now build.
    ("Register", "<pLength>"),
];

/// Count declarations that carry a resolved bitfield, as `(single-bit, ranged)`.
///
/// The split matters: `<Bit>` and `<LSB>`/`<MSB>` are parsed by different arms,
/// and a regression in one is invisible in a combined total.
fn count_bitfields(model: &viva_genapi_xml::XmlModel) -> (usize, usize) {
    let mut single = 0;
    let mut ranged = 0;
    for node in &model.nodes {
        let bitfield = match node {
            viva_genapi_xml::NodeDecl::Integer { bitfield, .. }
            | viva_genapi_xml::NodeDecl::Boolean { bitfield, .. } => bitfield,
            _ => continue,
        };
        match bitfield {
            Some(field) if field.bit_length > 1 => ranged += 1,
            Some(_) => single += 1,
            None => {}
        }
    }
    (single, ranged)
}

/// `<Streamable>` elements whose text is `Yes`, counted from the raw text
/// rather than through the parser under test (GA-14).
fn count_streamable_in_text(xml: &str) -> usize {
    let mut count = 0;
    let mut rest = xml;
    while let Some(start) = rest.find("<Streamable>") {
        rest = &rest[start + "<Streamable>".len()..];
        let Some(end) = rest.find("</Streamable>") else {
            break;
        };
        if rest[..end].trim() == "Yes" {
            count += 1;
        }
        rest = &rest[end..];
    }
    count
}

/// Streamable declarations inside the elements of nodes the parser skipped,
/// found by tag and name in the raw text: a `<Register>` with `<pLength>`
/// (GA-09) is a known gap, and its `<Streamable>` goes with it.
fn count_streamable_in_skipped(xml: &str, skipped: &[viva_genapi_xml::SkippedNode]) -> usize {
    skipped
        .iter()
        .filter_map(|node| {
            let name = node.name.as_deref()?;
            let open = format!("<{} Name=\"{name}\"", node.tag);
            let start = xml.find(&open)?;
            let close = format!("</{}>", node.tag);
            let end = xml[start..].find(&close)? + start;
            Some(count_streamable_in_text(&xml[start..end]))
        })
        .sum()
}

fn corpus_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VIVA_GENICAM_XML_CORPUS") {
        return PathBuf::from(dir);
    }
    // CARGO_MANIFEST_DIR is crates/viva-genapi-xml.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(DEFAULT_CORPUS)
}

fn is_expected_skip(document: &str, tag: &str, node: Option<&str>, error: &str) -> bool {
    EXPECTED_SKIP_REASONS
        .iter()
        .any(|(t, reason)| *t == tag && (reason.is_empty() || error.contains(reason)))
        || EXPECTED_SKIPS
            .iter()
            .any(|(doc, name)| *doc == document && Some(*name) == node)
}

#[test]
fn vendor_xml_corpus_parses() {
    let dir = corpus_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!(
            "corpus not present at {}; run scripts/fetch-xml-corpus.sh to enable this test",
            dir.display()
        );
        return;
    };

    let mut documents: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    documents.sort();

    if documents.is_empty() {
        println!(
            "corpus at {} is empty; run scripts/fetch-xml-corpus.sh",
            dir.display()
        );
        return;
    }

    let mut failures = Vec::new();
    let mut total_nodes = 0usize;
    let mut bitfields = 0usize;
    let mut streamable = 0usize;
    let mut streamable_documents = 0usize;

    for path in &documents {
        let document = path
            .file_name()
            .expect("directory entry has a file name")
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(path).expect("read corpus document");
        // Encoding handling is tracked separately (XML-02); lossy decoding keeps
        // this test focused on the parser.
        let xml = String::from_utf8_lossy(&bytes);

        match viva_genapi_xml::parse(&xml) {
            Ok(model) => {
                total_nodes += model.nodes.len();
                let (single, ranged) = count_bitfields(&model);
                bitfields += single + ranged;

                // A parser that drops a bit range does not fail, warn, or skip
                // the node — it returns an integer that reads the whole
                // register. That is how `parsers::numeric` came to match only
                // the mixed-case `<Lsb>` spelling while every real document
                // writes `<LSB>`, silently losing the range from 1 374 register
                // fields across this corpus until issue #120 surfaced it.
                //
                // Checked per document rather than as a corpus-wide total: the
                // fetch script warns and continues when a third-party URL is
                // unreachable, so any assertion keyed to the corpus *size*
                // would fail for a reason that has nothing to do with the
                // parser.
                if ranged == 0 && (xml.contains("<LSB>") || xml.contains("<Lsb>")) {
                    println!("FAIL  {document}: declares <LSB> but parsed no bit range");
                    failures.push(format!(
                        "{document}: declares <LSB>/<Lsb> but no multi-bit bitfield survived \
                         parsing — a bit-range spelling stopped being recognised"
                    ));
                }
                // GA-14: every `<Streamable>Yes</Streamable>` in the text is a
                // streamable node in the model, on whatever kind carries it.
                let parsed = model
                    .nodes
                    .iter()
                    .filter(|node| node.meta().streamable)
                    .count();
                let declared = count_streamable_in_text(&xml)
                    - count_streamable_in_skipped(&xml, &model.skipped);
                streamable += parsed;
                streamable_documents += usize::from(parsed > 0);
                if parsed != declared {
                    println!("FAIL  {document}: {declared} <Streamable>Yes, {parsed} parsed");
                    failures.push(format!(
                        "{document}: declares {declared} streamable features, parsed {parsed}"
                    ));
                }
                let unexpected: Vec<_> = model
                    .skipped
                    .iter()
                    .filter(|skipped| {
                        !is_expected_skip(
                            &document,
                            &skipped.tag,
                            skipped.name.as_deref(),
                            &skipped.error,
                        )
                    })
                    .collect();
                if unexpected.is_empty() {
                    println!(
                        "ok    {document}: {} nodes, {} known gaps",
                        model.nodes.len(),
                        model.skipped.len()
                    );
                } else {
                    println!("SKIP  {document}: {} unexpected", unexpected.len());
                    for skipped in &unexpected {
                        println!(
                            "        <{}> {:?}: {}",
                            skipped.tag, skipped.name, skipped.error
                        );
                        failures.push(format!(
                            "{document}: node <{}> {:?} skipped: {}",
                            skipped.tag, skipped.name, skipped.error
                        ));
                    }
                }
            }
            Err(err) => {
                println!("FAIL  {document}: {err}");
                failures.push(format!("{document}: {err}"));
            }
        }
    }

    println!(
        "=== {} documents, {total_nodes} nodes, {bitfields} bitfields, \
         {streamable} streamable in {streamable_documents} documents, {} failures ===",
        documents.len(),
        failures.len()
    );

    assert!(
        failures.is_empty(),
        "{} corpus document(s) regressed:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
