//! Conformance test that builds a runtime nodemap from real-camera GenICam XML.
//!
//! `viva-genapi-xml` has its own corpus test, but it stops at parsing. Every
//! defect behind issue #35 lived one layer above that: the formula language,
//! the address model, the integer/float split. A document can parse perfectly
//! and still be unusable.
//!
//! So this test goes further. For every document it builds a [`NodeMap`] and
//! then evaluates every node against [`NullIo`], which answers each read with
//! zeros. That will not produce meaningful values — no camera is attached —
//! but it exercises every formula, every address resolution and every
//! conversion, which is where the bugs were.
//!
//! Populate the corpus with:
//!
//! ```sh
//! scripts/fetch-xml-corpus.sh
//! cargo test -p viva-genapi --test vendor_corpus -- --nocapture
//! ```
//!
//! Set `VIVA_GENICAM_XML_CORPUS` to test a different directory — point it at
//! XML dumped from your own hardware to check a camera we have never seen.
//!
//! When the directory is absent the test passes with a note, so CI and a fresh
//! clone stay green.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use viva_genapi::{GenApiError, NodeMap, NullIo, RegisterIo};

/// Default corpus location relative to the workspace root.
const DEFAULT_CORPUS: &str = "fixtures/vendor-xml";

/// Nodes we knowingly cannot build yet, as `(document, node name)`.
///
/// Everything else that fails is a regression. Keep this list short and each
/// entry tied to a `docs/backlog.md` task.
const EXPECTED_SKIPS: &[(&str, &str)] = &[
    // XML-01: negative register address (`<Address>-4</Address>`), used for
    // offsets relative to the end of a chunk block. Our addressing model is
    // unsigned, so the node is already dropped by the XML layer.
    ("Baumer_HXG20.xml", "ChunkImageLength"),
];

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

/// Nodes that legitimately fail under the [`PatternIo`] pass, as
/// `(document, node name, required error substring)`.
///
/// Separate from [`EXPECTED_SKIPS`] on purpose: the pattern is a synthetic
/// device state, so a node can fail under it for reasons that say nothing
/// about the engine. An entry here is a statement that we looked and it is
/// honest.
const EXPECTED_PATTERN_DEFECTS: &[(&str, &str, &str)] = &[];

/// A stub transport that answers every read with a descending byte pattern
/// (`FF FE FD ...`).
///
/// [`NullIo`] returns zeros, and that is why this corpus test passed for three
/// weekly runs while 448 plain `<IntReg>` nodes decoded wrong: a byte swap of
/// zero is still zero, and an unsigned value's top bit is never set. A gate
/// that cannot fail is not a gate (GA-11).
///
/// Descending rather than all-ones on purpose. All-ones is a palindrome, so it
/// cannot distinguish a big-endian read from a little-endian one; this pattern
/// sets the top bit in *either* byte order while staying asymmetric.
///
/// What this pass catches is a conversion that *refuses* a representable
/// value. It does not catch a value that is merely wrong, because the corpus
/// test asserts no values — nothing here knows what a register should hold.
/// Per-document value expectations are the rest of GA-11, and they are what a
/// byte-order regression would need.
///
/// Deliberately test-local rather than a sibling of `NullIo` in
/// `viva-genapi/src/io.rs`: `NullIo`'s zero contract is public API that Viva
/// Studio and the wasm build depend on for offline XML browsing, and a second,
/// differently-behaved stub next to it invites the wrong one.
struct PatternIo;

impl RegisterIo for PatternIo {
    fn read(&self, _addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        Ok((0..len).map(|i| 0xFFu8.wrapping_sub(i as u8)).collect())
    }

    fn write(&self, _addr: u64, _data: &[u8]) -> Result<(), GenApiError> {
        Ok(())
    }
}

/// Which stub a pass runs against, and how strict it is about `Parse`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Reads return zeros. `Parse` can be an honest outcome here — a zero
    /// enum value with no matching entry, for instance.
    Zeros,
    /// Reads return a descending byte pattern, which sets the top bit of
    /// every register in either byte order. `Parse` is a defect in this pass:
    /// every value is representable, so a conversion that refuses one is ours.
    Pattern,
}

impl Pass {
    fn io(self) -> Box<dyn RegisterIo> {
        match self {
            Pass::Zeros => Box::new(NullIo),
            Pass::Pattern => Box::new(PatternIo),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Pass::Zeros => "zeros",
            Pass::Pattern => "pattern",
        }
    }

    fn is_defect(self, err: &GenApiError) -> bool {
        if is_engine_defect(err) {
            return true;
        }
        self == Pass::Pattern && matches!(err, GenApiError::Parse(_))
    }
}

fn corpus_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VIVA_GENICAM_XML_CORPUS") {
        return PathBuf::from(dir);
    }
    // CARGO_MANIFEST_DIR is crates/viva-genapi.
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

/// Whether an evaluation error means the formula engine is wrong, as opposed
/// to the unavoidable consequence of having no camera attached.
///
/// Reading zeros makes plenty of formulas divide by zero and plenty of enums
/// land on an undefined entry. Those are honest runtime outcomes. A formula
/// that references a variable nobody declared, or calls a function we have
/// never heard of, is a gap in this crate.
fn is_engine_defect(err: &GenApiError) -> bool {
    match err {
        GenApiError::ExprParse { .. } | GenApiError::UnknownVariable { .. } => true,
        GenApiError::ExprEval { msg, .. } => {
            msg.starts_with("unknown function") || msg.contains("expects")
        }
        _ => false,
    }
}

#[test]
fn vendor_xml_corpus_builds_nodemaps() {
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
    let mut total_evaluated = 0usize;

    for path in &documents {
        let document = path
            .file_name()
            .expect("directory entry has a file name")
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(path).expect("read corpus document");
        // Encoding handling is tracked separately (XML-02); lossy decoding
        // keeps this test focused on the model.
        let xml = String::from_utf8_lossy(&bytes);

        let model = match viva_genapi_xml::parse(&xml) {
            Ok(model) => model,
            Err(err) => {
                println!("FAIL  {document}: parse: {err}");
                failures.push(format!("{document}: parse failed: {err}"));
                continue;
            }
        };

        let nodemap = match NodeMap::try_from_xml(model.clone()) {
            Ok(nodemap) => nodemap,
            Err(err) => {
                println!("FAIL  {document}: nodemap: {err}");
                failures.push(format!("{document}: nodemap build failed: {err}"));
                continue;
            }
        };

        total_nodes += nodemap.node_names().count();

        // 1. Nothing should have been dropped on the way to the nodemap.
        for skipped in nodemap.skipped() {
            if is_expected_skip(
                &document,
                &skipped.tag,
                skipped.name.as_deref(),
                &skipped.error,
            ) {
                continue;
            }
            println!(
                "SKIP  {document}: <{}> {:?}: {}",
                skipped.tag, skipped.name, skipped.error
            );
            failures.push(format!(
                "{document}: node <{}> {:?} dropped: {}",
                skipped.tag, skipped.name, skipped.error
            ));
        }

        // 2. Every node should be reachable through the value API without the
        //    formula engine complaining — under both stub patterns. Zeros
        //    leave byte order and full-width values invisible; all-ones is
        //    what makes them observable without hardware.
        let names: Vec<String> = nodemap.node_names().map(str::to_string).collect();
        let mut clean = true;
        for pass in [Pass::Zeros, Pass::Pattern] {
            let io = pass.io();
            // A fresh nodemap per pass. The two stubs disagree about every
            // register, and node caches are per-nodemap, so sharing one would
            // have the second pass read the first pass's values.
            let nodemap = match NodeMap::try_from_xml(model.clone()) {
                Ok(nodemap) => nodemap,
                Err(err) => {
                    failures.push(format!("{document}: nodemap build failed: {err}"));
                    continue;
                }
            };
            let mut defects: BTreeMap<String, String> = BTreeMap::new();
            for name in &names {
                total_evaluated += 1;
                // A node is readable through whichever accessor matches its
                // kind; trying float first and falling back covers integer,
                // boolean and enum nodes without switching on the kind here.
                let err = match nodemap.get_float(name, io.as_ref()) {
                    Ok(_) => continue,
                    Err(err) => err,
                };
                if pass.is_defect(&err) {
                    defects.insert(name.clone(), err.to_string());
                    continue;
                }
                if let Err(err) = nodemap.get_integer(name, io.as_ref())
                    && pass.is_defect(&err)
                {
                    defects.insert(name.clone(), err.to_string());
                }
            }

            for (name, err) in &defects {
                if EXPECTED_PATTERN_DEFECTS.iter().any(|(doc, node, reason)| {
                    pass == Pass::Pattern
                        && *doc == document
                        && node == name
                        && err.contains(reason)
                }) {
                    continue;
                }
                println!("EVAL  {document} [{}]: {name}: {err}", pass.label());
                failures.push(format!(
                    "{document}: evaluating {name} under {}: {err}",
                    pass.label()
                ));
                clean = false;
            }
        }

        if clean && nodemap.skipped().is_empty() {
            println!(
                "ok    {document}: {} nodes built and evaluated twice",
                names.len()
            );
        }
    }

    println!(
        "\n{} documents, {total_nodes} nodes built, {total_evaluated} evaluated",
        documents.len()
    );

    assert!(
        failures.is_empty(),
        "vendor corpus regressions ({}):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
