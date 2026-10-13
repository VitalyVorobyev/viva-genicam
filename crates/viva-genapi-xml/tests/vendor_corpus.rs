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

use std::collections::BTreeMap;
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
const EXPECTED_SKIP_REASONS: &[(&str, &str)] = &[];

/// Child elements the parser reads past without interpreting, as `(node kind,
/// element, why that is acceptable or which backlog row tracks it)`: the
/// allow-list of [`viva_genapi_xml::XmlModel::ignored`].
///
/// An element not listed fails the test: it is the class of loss that hid a
/// `<String>`'s `<pValue>` until a camera showed it (H-2). The node kind is
/// [`viva_genapi_xml::NodeDecl::kind`]'s, so `Integer` covers `<IntReg>`,
/// `<MaskedIntReg>` and a `<StructEntry>` too; an entry's child is written
/// `EnumEntry/<tag>`. The reasons are checked by nothing but a reader: keep
/// each one true.
const EXPECTED_IGNORED: &[(&str, &str, &str)] = &[
    // GenICam Standard v2.1.1 §2.8.1: "All elements placed inside the
    // <Extension> element are ignored."
    ("Boolean", "Extension", EXTENSION),
    ("Category", "Extension", EXTENSION),
    ("Command", "Extension", EXTENSION),
    ("Converter", "Extension", EXTENSION),
    ("Enum", "Extension", EXTENSION),
    ("Enum", "EnumEntry/Extension", EXTENSION),
    ("Float", "Extension", EXTENSION),
    ("Integer", "Extension", EXTENSION),
    ("String", "Extension", EXTENSION),
    // Documentation and presentation for a GUI, which change no value
    // (GA-41).
    ("Command", "IsDeprecated", DEPRECATED),
    ("Enum", "IsDeprecated", DEPRECATED),
    ("Integer", "IsDeprecated", DEPRECATED),
    ("String", "IsDeprecated", DEPRECATED),
    (
        "Enum",
        "EnumEntry/Description",
        "documentation of an entry (GA-41)",
    ),
    (
        "Enum",
        "EnumEntry/ToolTip",
        "documentation of an entry (GA-41)",
    ),
    (
        "Enum",
        "EnumEntry/Visibility",
        "the user level a GUI shows an entry at (GA-41)",
    ),
    (
        "Float",
        "DisplayNotation",
        "how a GUI formats the number (GA-41)",
    ),
    (
        "Float",
        "DisplayPrecision",
        "how a GUI formats the number (GA-41)",
    ),
    (
        "Float",
        "pAlias",
        "§2.8.1: the same feature described another way, \"mainly used in a GUI\" (GA-41)",
    ),
    ("Port", "Description", PORT_DOCUMENTATION),
    ("Port", "ToolTip", PORT_DOCUMENTATION),
    ("Port", "Visibility", PORT_DOCUMENTATION),
    (
        "Port",
        "ImposedAccessMode",
        "every occurrence in the corpus is RW, which imposes nothing",
    ),
    // Known gaps, each with its backlog row.
    ("Float", "Inc", "GA-43: a Float's increment is not enforced"),
    (
        "Float",
        "pInc",
        "GA-43: a Float's increment is not enforced",
    ),
    (
        "Float",
        "Value",
        "GA-49: a Float's constant <Value> is not read",
    ),
    (
        "Float",
        "Sign",
        "GA-18: the sign of a scaled Float's payload",
    ),
    ("Integer", "ValueIndexed", INDEXED),
    ("Integer", "ValueDefault", INDEXED),
    ("Integer", "pValueIndexed", INDEXED),
    ("Integer", "pValueDefault", INDEXED),
    ("Float", "ValueIndexed", INDEXED),
    ("Float", "ValueDefault", INDEXED),
    ("Float", "pValueIndexed", INDEXED),
    ("Float", "pValueDefault", INDEXED),
    (
        "Integer",
        "pValueCopy",
        "GA-51: a write is not copied to <pValueCopy>",
    ),
    (
        "Integer",
        "EventID",
        "GA-52: an event does not invalidate the node its <EventID> names",
    ),
    (
        "Port",
        "pInvalidator",
        "GA-53: a change of a port's <pInvalidator> does not invalidate its registers",
    ),
    (
        "Enum",
        "EnumEntry/ImposedAccessMode",
        "GA-54: an entry's <ImposedAccessMode> is not honoured",
    ),
];

const EXTENSION: &str = "vendor data, ignored by definition (§2.8.1)";
const DEPRECATED: &str = "a note for new designs (§2.8.1, GA-41)";
const PORT_DOCUMENTATION: &str = "a port is not a feature: its documentation has nowhere to go";
const INDEXED: &str = "GA-50: an indexed value (<pIndex> with <ValueIndexed>) is not supported";

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

/// The text of every `<tag>…</tag>` element in `text`, in order.
fn element_texts<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else {
            break;
        };
        found.push(rest[..end].trim());
        rest = &rest[end..];
    }
    found
}

/// The `<tag Name=…>…</tag>` blocks of `text`, and what lies between them.
fn split_blocks<'a>(text: &'a str, tag: &str) -> (Vec<&'a str>, String) {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut blocks = Vec::new();
    let mut outside = String::new();
    let mut rest = text;
    loop {
        // `<StructReg` must not match `<StructRegs…`, nor `<StructEntry`
        // match anything longer.
        let Some(start) = rest.match_indices(&open).map(|(i, _)| i).find(|&i| {
            matches!(
                rest[i + open.len()..].chars().next(),
                Some(' ' | '>' | '\n' | '\t' | '\r')
            )
        }) else {
            outside.push_str(rest);
            break;
        };
        outside.push_str(&rest[..start]);
        let Some(end) = rest[start..].find(&close) else {
            outside.push_str(&rest[start..]);
            break;
        };
        let end = start + end + close.len();
        blocks.push(&rest[start..end]);
        rest = &rest[end..];
    }
    (blocks, outside)
}

/// Nodes that declare `<tag>`, keyed by the element's text, counted from the
/// raw text rather than through the parser under test (GA-05). A
/// `<StructReg>`'s own declaration counts once for each of its entries that
/// declares none, as each entry is a node viewing the struct's register.
fn count_declared(xml: &str, tags: &[&str]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut add = |text: &str| *counts.entry(text.to_string()).or_default() += 1;
    let texts = |text: &str| -> Vec<String> {
        tags.iter()
            .flat_map(|tag| element_texts(text, tag))
            .map(str::to_string)
            .collect()
    };
    let (structs, outside) = split_blocks(xml, "StructReg");
    for value in texts(&outside) {
        add(&value);
    }
    for block in structs {
        let (entries, register) = split_blocks(block, "StructEntry");
        let inherited = texts(&register).into_iter().next();
        for entry in entries {
            if let Some(value) = texts(entry)
                .into_iter()
                .next()
                .or_else(|| inherited.clone())
            {
                add(&value);
            }
        }
    }
    counts
}

/// [`count_declared`] inside the elements of nodes the parser skipped.
fn count_declared_in_skipped(
    xml: &str,
    tags: &[&str],
    skipped: &[viva_genapi_xml::SkippedNode],
) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in skipped {
        let Some(name) = node.name.as_deref() else {
            continue;
        };
        let open = format!("<{} Name=\"{name}\"", node.tag);
        let Some(start) = xml.find(&open) else {
            continue;
        };
        let close = format!("</{}>", node.tag);
        let Some(end) = xml[start..].find(&close).map(|end| end + start) else {
            continue;
        };
        for (value, n) in count_declared(&xml[start..end], tags) {
            *counts.entry(value).or_default() += n;
        }
    }
    counts
}

/// What the parser recorded: nodes with a `<Cachable>` by mode, and nodes
/// with a `<PollingTime>`.
fn count_parsed(model: &viva_genapi_xml::XmlModel) -> (BTreeMap<String, usize>, usize) {
    let mut cachable: BTreeMap<String, usize> = BTreeMap::new();
    let mut polling = 0;
    for node in &model.nodes {
        if let Some(mode) = node.meta().cachable {
            *cachable.entry(format!("{mode:?}")).or_default() += 1;
        }
        polling += usize::from(node.meta().polling_time_ms.is_some());
    }
    (cachable, polling)
}

/// `declared` less `skipped`, dropping what reaches zero.
fn subtract(
    mut declared: BTreeMap<String, usize>,
    skipped: BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    for (value, n) in skipped {
        if let Some(count) = declared.get_mut(&value) {
            *count = count.saturating_sub(n);
        }
    }
    declared.retain(|_, n| *n > 0);
    declared
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
    // GA-05: elements in the text, and the nodes that carry them.
    let mut cachable_elements = 0usize;
    let mut cachable_documents = 0usize;
    let mut cachable_nodes: BTreeMap<String, usize> = BTreeMap::new();
    let mut polling_elements = 0usize;
    let mut polling_documents = 0usize;
    let mut polling_nodes = 0usize;
    // H-2: (kind, element) -> (occurrences, documents, an example node).
    let mut ignored: BTreeMap<(String, String), (usize, usize, String)> = BTreeMap::new();

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
                // GA-05: every `<Cachable>` and `<PollingTime>` in the text is
                // on a node in the model, with the mode the text names.
                const CACHABLE: &[&str] = &["Cachable", "Cacheable"];
                const POLLING: &[&str] = &["PollingTime"];
                let elements = element_texts(&xml, "Cachable").len();
                cachable_elements += elements;
                cachable_documents += usize::from(elements > 0);
                let elements = element_texts(&xml, "PollingTime").len();
                polling_elements += elements;
                polling_documents += usize::from(elements > 0);
                let (parsed_cachable, parsed_polling) = count_parsed(&model);
                let declared_cachable = subtract(
                    count_declared(&xml, CACHABLE),
                    count_declared_in_skipped(&xml, CACHABLE, &model.skipped),
                );
                let declared_polling: usize = subtract(
                    count_declared(&xml, POLLING),
                    count_declared_in_skipped(&xml, POLLING, &model.skipped),
                )
                .values()
                .sum();
                for (mode, n) in &parsed_cachable {
                    *cachable_nodes.entry(mode.clone()).or_default() += n;
                }
                polling_nodes += parsed_polling;
                if parsed_cachable != declared_cachable {
                    println!(
                        "FAIL  {document}: <Cachable> on {declared_cachable:?}, parsed {parsed_cachable:?}"
                    );
                    failures.push(format!(
                        "{document}: declares <Cachable> on {declared_cachable:?} nodes, parsed {parsed_cachable:?}"
                    ));
                }
                if parsed_polling != declared_polling {
                    println!(
                        "FAIL  {document}: <PollingTime> on {declared_polling} nodes, parsed {parsed_polling}"
                    );
                    failures.push(format!(
                        "{document}: declares <PollingTime> on {declared_polling} nodes, parsed {parsed_polling}"
                    ));
                }
                let mut seen_here = std::collections::BTreeSet::new();
                for element in model.ignored() {
                    let key = (element.kind.clone(), element.element.clone());
                    let entry = ignored
                        .entry(key.clone())
                        .or_insert_with(|| (0, 0, format!("{document}: {}", element.node)));
                    entry.0 += 1;
                    if seen_here.insert(key) {
                        entry.1 += 1;
                    }
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

    let mut ignored_total = 0usize;
    for ((kind, element), (count, docs, example)) in &ignored {
        ignored_total += count;
        let allowed = EXPECTED_IGNORED
            .iter()
            .find(|(k, e, _)| k == kind && e == element);
        match allowed {
            Some((_, _, reason)) => {
                println!(
                    "ignored {kind:<12} {element:<28} {count:>6} in {docs:>2} documents: {reason}"
                )
            }
            None => {
                println!(
                    "IGNORED {kind:<12} {element:<28} {count:>6} in {docs:>2} documents, not allowed (e.g. {example})"
                );
                failures.push(format!(
                    "<{element}> on {kind} is read past without interpretation \
                     ({count} in {docs} documents, e.g. {example}): implement it, or allow it \
                     in EXPECTED_IGNORED with the reason"
                ));
            }
        }
    }
    println!(
        "=== ignored: {ignored_total} elements of {} kinds ===",
        ignored.len()
    );
    println!(
        "=== {} documents, {total_nodes} nodes, {bitfields} bitfields, \
         {streamable} streamable in {streamable_documents} documents, {} failures ===",
        documents.len(),
        failures.len()
    );
    println!(
        "=== <Cachable>: {cachable_elements} elements in {cachable_documents} documents, \
         on nodes {cachable_nodes:?}; <PollingTime>: {polling_elements} elements in \
         {polling_documents} documents, on {polling_nodes} nodes ==="
    );

    assert!(
        failures.is_empty(),
        "{} corpus document(s) regressed:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
