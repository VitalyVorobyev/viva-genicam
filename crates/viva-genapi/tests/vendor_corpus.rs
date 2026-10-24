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

use viva_genapi::swissknife::EvalError;
use viva_genapi::{GenApiError, NodeMap, NullIo, RegisterIo};

/// Default corpus location relative to the workspace root.
const DEFAULT_CORPUS: &str = "fixtures/vendor-xml";

/// Nodes we knowingly cannot build yet, as `(document, node name)`.
///
/// Everything else that fails is a regression. Keep this list short and each
/// entry tied to a `docs/backlog.md` task.
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

/// Which stub a pass runs against, and how strict it is about conversions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Reads return zeros. A refused conversion can be an honest outcome
    /// here: a zero raw value through a formula that divides by it.
    Zeros,
    /// Reads return a descending byte pattern, which sets the top bit of
    /// every register in either byte order. A register that does not convert
    /// is a defect in this pass: every value is representable, so a
    /// conversion that refuses one is ours. So is a declaration the node
    /// map cannot evaluate.
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
        self == Pass::Pattern
            && matches!(
                err,
                GenApiError::RegisterEncoding { .. } | GenApiError::Declaration { .. }
            )
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
        GenApiError::ExprEval { source, .. } => matches!(
            source,
            EvalError::UnknownFunction(_) | EvalError::ArityMismatch { .. }
        ),
        _ => false,
    }
}

/// What a failed read says about the document, for the corpus summary: a
/// register whose bytes do not convert is the device's data (or the width
/// the node declares) meeting our codec; a declaration the node map cannot
/// evaluate is the XML. Before backlog GA-30 both were `Parse`, so a failed
/// device read read as bad XML and the summary could not tell them apart.
fn failure_class(err: &GenApiError) -> &'static str {
    match err {
        GenApiError::RegisterEncoding { .. } => "register encoding",
        GenApiError::Declaration { .. } => "declaration",
        GenApiError::ExprParse { .. }
        | GenApiError::ExprEval { .. }
        | GenApiError::UnknownVariable { .. } => "formula",
        GenApiError::Range { .. } => "range",
        GenApiError::EnumValueUnknown { .. } => "enum value",
        GenApiError::Access { .. } => "access",
        GenApiError::Type { .. } => "kind",
        GenApiError::NotAttached { .. } => "no chunk or event attached",
        // A `<pLength>` node that reads no sane length: the pattern's
        // `FF FE FD FC` is 4 GiB.
        GenApiError::BadLength { .. } => "length",
        GenApiError::Unavailable(_) => "unavailable",
        GenApiError::NoValueSource { .. } => "no register, pValue or Value",
        // An index the stub's bytes give that no entry names, on a node with
        // no default.
        GenApiError::NoIndexedValue { .. } => "no indexed value",
        _ => "other",
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
    let mut total_ignored = 0usize;
    // Reads that failed for a reason the pass allows, by pass and class.
    let mut allowed: BTreeMap<(&'static str, &'static str), usize> = BTreeMap::new();

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
                // A string or a raw register has an accessor of its own: a
                // `<String>` reads through its `<pValue>`, and a register
                // reads the length its `<pLength>` node gives (H-1, H-3).
                let own = match nodemap.node(name) {
                    Some(viva_genapi::Node::String(_)) => {
                        Some(nodemap.get_string(name, io.as_ref()).map(drop))
                    }
                    Some(viva_genapi::Node::Register(_)) => {
                        Some(nodemap.get_register(name, io.as_ref()).map(drop))
                    }
                    _ => None,
                };
                if let Some(result) = own {
                    match result {
                        Ok(()) => {}
                        Err(err) if pass.is_defect(&err) => {
                            defects.insert(name.clone(), format!("{}: {err}", failure_class(&err)));
                        }
                        Err(err) => {
                            *allowed
                                .entry((pass.label(), failure_class(&err)))
                                .or_default() += 1;
                        }
                    }
                    continue;
                }
                // Any other node is readable through whichever accessor
                // matches its kind; trying float first and falling back
                // covers integer, boolean and enum nodes without switching on
                // the kind here.
                let err = match nodemap.get_float(name, io.as_ref()) {
                    Ok(_) => continue,
                    Err(err) => err,
                };
                if pass.is_defect(&err) {
                    defects.insert(name.clone(), format!("{}: {err}", failure_class(&err)));
                    continue;
                }
                match nodemap.get_integer(name, io.as_ref()) {
                    Ok(_) => {}
                    Err(err) if pass.is_defect(&err) => {
                        defects.insert(name.clone(), format!("{}: {err}", failure_class(&err)));
                    }
                    Err(err) => {
                        *allowed
                            .entry((pass.label(), failure_class(&err)))
                            .or_default() += 1;
                    }
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

        // H-2: what the parser read past, carried by the nodemap. Classified
        // by `viva-genapi-xml`'s corpus test (`EXPECTED_IGNORED`); listed
        // here so that a consumer holding only the nodemap sees the same.
        if nodemap.ignored() != model.ignored().as_slice() {
            failures.push(format!(
                "{document}: the nodemap's ignored elements differ from the model's"
            ));
        }
        total_ignored += nodemap.ignored().len();

        if clean && nodemap.skipped().is_empty() {
            println!(
                "ok    {document}: {} nodes built and evaluated twice",
                names.len()
            );
        }
    }

    println!(
        "\n{} documents, {total_nodes} nodes built, {total_evaluated} evaluated, \
         {total_ignored} child elements read past",
        documents.len()
    );
    // Reads that fail without a camera, by why: a kind with no value, a
    // write-only node, an enum value the stub's bytes do not name. A register
    // that does not convert and a declaration that does not evaluate are
    // counted apart (GA-30); under the pattern pass either is a defect.
    for ((pass, class), count) in &allowed {
        println!("  {pass}: {count} reads refused ({class})");
    }

    assert!(
        failures.is_empty(),
        "vendor corpus regressions ({}):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// `FLIR_BFS_PGE_31S4C_C` imposes `RO` on `SensorWidth`, an `RW` register:
/// a camera's own word that the sensor's width cannot be written, which the
/// GenApi layer ignored until it parsed `<ImposedAccessMode>` (backlog GA-23).
#[test]
fn an_imposed_read_only_feature_is_read_only() {
    let path = corpus_dir().join("FLIR_BFS_PGE_31S4C_C.xml");
    let Ok(bytes) = std::fs::read(&path) else {
        println!(
            "{} not present; run scripts/fetch-xml-corpus.sh",
            path.display()
        );
        return;
    };
    let xml = String::from_utf8_lossy(&bytes);
    let model = viva_genapi_xml::parse(&xml).expect("parse");
    let nodemap = NodeMap::try_from_xml(model).expect("build");
    assert_eq!(
        nodemap.declared_access_mode("SensorWidth"),
        Some(viva_genapi_xml::AccessMode::RO)
    );
}

// ── Per-document value expectations (GA-11) ─────────────────────────────────
//
// The evaluation pass above proves every node *evaluates*; it cannot tell a
// right value from a wrong one. The cases below pin what real documents make
// us write and read, byte for byte. Every literal was derived by hand from the
// document's XML — the node names and line numbers are cited beside it — and
// none from the code under test. Minified documents (FLIR, Point Grey) keep a
// whole category on one line, so their citations name the nodes.

use std::cell::RefCell;
use std::collections::HashMap;

/// Register memory for one expectation: the reads the camera answers with,
/// and a log of every read and write in order. A register nothing set reads
/// as zeros.
#[derive(Default)]
struct DeviceIo {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
    reads: RefCell<Vec<u64>>,
    writes: RefCell<Vec<(u64, Vec<u8>)>>,
}

impl DeviceIo {
    fn with(regs: &[(u64, &[u8])]) -> Self {
        let io = Self::default();
        for (addr, bytes) in regs {
            io.regs.borrow_mut().insert(*addr, bytes.to_vec());
        }
        io
    }
}

impl RegisterIo for DeviceIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        self.reads.borrow_mut().push(addr);
        Ok(self
            .regs
            .borrow()
            .get(&addr)
            .cloned()
            .unwrap_or_else(|| vec![0; len]))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        self.regs.borrow_mut().insert(addr, data.to_vec());
        self.writes.borrow_mut().push((addr, data.to_vec()));
        Ok(())
    }
}

/// A value written before the command runs.
#[derive(Clone, Copy)]
enum Set {
    /// An enumeration entry, by name.
    Entry(&'static str, &'static str),
    /// An integer.
    Int(&'static str, i64),
}

/// One command executed on one document, with every write it must cause.
struct CommandCase {
    document: &'static str,
    /// Registers the camera answers reads of: the inquiry bits that make the
    /// command implemented, and the like.
    device: &'static [(u64, &'static [u8])],
    /// Features written first, in order.
    set: &'static [Set],
    command: &'static str,
    /// Every write the whole sequence makes, in order: the selector's, then
    /// the command's.
    writes: &'static [(u64, &'static [u8])],
}

/// The FLIR / Point Grey IIDC-style user-set block, shared by four documents
/// with the same node names: `UserSetSelector` writes its value through
/// `UserSetVal_Int` (`<IntConverter>`, `FormulaTo` `FROM << 28`) into
/// `MemSaveCh_Reg` at `CamRegBaseAddress` 0xF0F00000 + 0x620; `UserSetLoad`
/// takes `<pCommandValue>UserSetVal_Int</pCommandValue>` — `FormulaFrom`
/// `TO >> 28` — and writes it into bits 0..3 (`<MSB>0</MSB><LSB>3</LSB>`,
/// big-endian: the top nibble) of `UserSetLoad_CtrlValueReg` at 0xF0F00000 +
/// 0x5114. `UserSetSelector`'s `<pIsImplemented>` reads `MemoryChannelMax_Reg`
/// at 0xF0F00000 + 0x400, bits 28..31 (the low nibble): 2 channels.
const PGR_USER_SET_LOAD: CommandCase = CommandCase {
    document: "",
    device: &[(0xF0F0_0400, &[0x00, 0x00, 0x00, 0x02])],
    set: &[Set::Entry("UserSetSelector", "UserSet2")],
    command: "UserSetLoad",
    writes: &[
        (0xF0F0_0620, &[0x20, 0x00, 0x00, 0x00]),
        (0xF0F0_5114, &[0x20, 0x00, 0x00, 0x00]),
    ],
};

/// The Allied Vision `TriggerSoftware` block, shared by the Manta and
/// Prosilica documents: `TriggerSelector` writes the floating
/// `IntTriggerSelector` (no register); the command value is
/// `IntTriggerSoftwareCommand`, `(1 << VALUE)` of that selector, written to
/// `RegSoftwareTrigger` at 0x13160 (big-endian); the command's predicate is
/// `RegTriggerSourceInqSoftware`, `<Bit>25</Bit>` (big-endian: 0x00000040) of
/// 0x13408 + selector × 64. `AcquisitionStart` is selector value 3, so the
/// predicate lives at 0x13408 + 0xC0 = 0x134C8 and the command writes 1 << 3.
/// The entry itself is implemented by `RegTriggerInqAcquisitionStart`,
/// `<Bit>20</Bit>` (big-endian: 0x00000800) of 0x13000; an entry not
/// implemented cannot be selected (H-4).
const AVT_TRIGGER_SOFTWARE: CommandCase = CommandCase {
    document: "",
    device: &[
        (0x13000, &[0x00, 0x00, 0x08, 0x00]),
        (0x134C8, &[0x00, 0x00, 0x00, 0x40]),
    ],
    set: &[Set::Entry("TriggerSelector", "AcquisitionStart")],
    command: "TriggerSoftware",
    writes: &[(0x13160, &[0x00, 0x00, 0x00, 0x08])],
};

const COMMAND_CASES: &[CommandCase] = &[
    // Hikrobot_MV-CS050-10GC.xml: `UserSetSave` (23033-23043) writes
    // `UserSetSave_CmdValueFao` = `UserSetSelector_Reg` (23083-23086) to
    // `UserSetSave_Reg` (23091-23100, BigEndian) at `UserSetSave_RegAddr`
    // 0x0003180C (1323-1325). `UserSetSelector_Reg` is at 0x00031804
    // (1326-1328); `UserSet2` is 2 (23122). The inquiry `<StructReg>` at
    // `UserSetInq_RegAddr` 0x00010800 (2271-2273) has `UserSetLoad_Inq` bit 0,
    // `UserSetSave_Inq` bit 1, `UserSetSelector_Inq` bit 2 (22956-22964), and
    // the entry's own `UserSetSelector_UserSet2_Inq` bit 5 (22971-22973): an
    // entry not implemented cannot be selected (H-4).
    CommandCase {
        document: "Hikrobot_MV-CS050-10GC.xml",
        device: &[(0x0001_0800, &[0xE4, 0x00, 0x00, 0x00])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetSave",
        writes: &[
            (0x0003_1804, &[0x00, 0x00, 0x00, 0x02]),
            (0x0003_180C, &[0x00, 0x00, 0x00, 0x02]),
        ],
    },
    // Same document: `UserSetLoad` (23009-23019) writes
    // `UserSetLoad_CmdValueFao` (23020-23023), the same selector value, to
    // `UserSetLoad_Reg` (23024-23032) at 0x00031808 (1320-1322).
    CommandCase {
        document: "Hikrobot_MV-CS050-10GC.xml",
        device: &[(0x0001_0800, &[0xE4, 0x00, 0x00, 0x00])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetLoad",
        writes: &[
            (0x0003_1804, &[0x00, 0x00, 0x00, 0x02]),
            (0x0003_1808, &[0x00, 0x00, 0x00, 0x02]),
        ],
    },
    // Basler_acA1600_20gm.xml: `UserSetLoad` (4945-4954) takes
    // `UserSetLoad_CmdValueFao`, the constant `1` (14619-14621), whatever is
    // selected, and writes it to `UserSetLoad_CtrlValueFao` at 0x60004
    // (12933-12941). The selector writes `UserSetSelector_CtrlValueFao` at
    // 0x60044 (12990-13000); `UserSet2` is 2 (4989). Presence bits:
    // `UserSetLoad_InqPresenceFao` 0x60000 bit 0 (12942-12953),
    // `UserSetSelector_InqPresenceFao` 0x60040 bit 0 (12978-12989), and the
    // entry's `UserSetSelector_UserSet2_InqPresenceFao` 0x6004C bit 2
    // (13025-13036).
    CommandCase {
        document: "Basler_acA1600_20gm.xml",
        device: &[
            (0x60000, &[0x80, 0x00, 0x00, 0x00]),
            (0x60040, &[0x80, 0x00, 0x00, 0x00]),
            (0x6004C, &[0x20, 0x00, 0x00, 0x00]),
        ],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetLoad",
        writes: &[
            (0x60044, &[0x00, 0x00, 0x00, 0x02]),
            (0x60004, &[0x00, 0x00, 0x00, 0x01]),
        ],
    },
    // Basler_piA640_210gm.xml: the selector writes bits 0..3 (big-endian: the
    // top nibble) of `UserSetSelector_CtrlValueFao` at 0xF0F00620
    // (6988-6998); `UserSetLoad` (3314-3322) reads them back through
    // `UserSetLoad_CmdValueFao`, the same bits of the same register
    // (6954-6965), and writes them through `UserSetLoad_CtrlValue`
    // (`<IntConverter>`, `FormulaTo` `FROM`, 9958-9963) into bits 0..3 of
    // `UserSetLoad_CtrlValueFao` at 0xF0F00624 (6966-6976). Both presence
    // checks are `(0 < NUM_USERSETS)` over bits 28..31 (the low nibble) of
    // 0xF0F00400 (8065-8087): 3 sets.
    CommandCase {
        document: "Basler_piA640_210gm.xml",
        device: &[(0xF0F0_0400, &[0x00, 0x00, 0x00, 0x03])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetLoad",
        writes: &[
            (0xF0F0_0620, &[0x20, 0x00, 0x00, 0x00]),
            (0xF0F0_0624, &[0x20, 0x00, 0x00, 0x00]),
        ],
    },
    // FLIR_BFLY_PGE_13E4C.xml, line 110: see `PGR_USER_SET_LOAD`.
    CommandCase {
        document: "FLIR_BFLY_PGE_13E4C.xml",
        ..PGR_USER_SET_LOAD
    },
    // FLIR_BFLY_PGE_31S4C.xml, line 107: see `PGR_USER_SET_LOAD`.
    CommandCase {
        document: "FLIR_BFLY_PGE_31S4C.xml",
        ..PGR_USER_SET_LOAD
    },
    // PGR_Blackfly_20E4C.xml, line 106: see `PGR_USER_SET_LOAD`.
    CommandCase {
        document: "PGR_Blackfly_20E4C.xml",
        ..PGR_USER_SET_LOAD
    },
    // PGR_GS3_U3_23S6M.xml, line 115: the `PGR_USER_SET_LOAD` nodes on a
    // USB3 camera — `CamRegBaseAddress` 0xFFFFF0F00000 (line 19) and every
    // register `LittleEndian`. `MemoryChannelMax_Reg` is bits 0..3 (the low
    // nibble of byte 0), `MemSaveCh_Reg` takes 0x20000000 least significant
    // byte first, and `UserSetLoad_CtrlValueReg`'s bits 28..31 are the top
    // nibble of the last byte. A byte-swapped register gets every one wrong.
    CommandCase {
        document: "PGR_GS3_U3_23S6M.xml",
        device: &[(0xFFFF_F0F0_0400, &[0x02, 0x00, 0x00, 0x00])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetLoad",
        writes: &[
            (0xFFFF_F0F0_0620, &[0x00, 0x00, 0x00, 0x20]),
            (0xFFFF_F0F0_5114, &[0x00, 0x00, 0x00, 0x20]),
        ],
    },
    // PGR_Blackfly_13E4C-CS.xml, line 96: `UserSetSave` writes
    // `MemSaveVal_Int`, `((CHAN & 0xF0000000) = 0) ? 0x87654321 : 0x80000000`
    // over `MemSaveCh_Reg` (0xF0F00620), to `MemorySave_Reg` at 0xF0F00618 —
    // a key, not a set number. With `UserSet2` the channel is 0x20000000, so
    // the key is 0x80000000.
    CommandCase {
        document: "PGR_Blackfly_13E4C-CS.xml",
        device: &[(0xF0F0_0400, &[0x00, 0x00, 0x00, 0x02])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetSave",
        writes: &[
            (0xF0F0_0620, &[0x20, 0x00, 0x00, 0x00]),
            (0xF0F0_0618, &[0x80, 0x00, 0x00, 0x00]),
        ],
    },
    // Same document: its `UserSetLoad_CtrlValueReg` is at 0xF0F00000 + 0x4014.
    CommandCase {
        document: "PGR_Blackfly_13E4C-CS.xml",
        device: &[(0xF0F0_0400, &[0x00, 0x00, 0x00, 0x02])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetLoad",
        writes: &[
            (0xF0F0_0620, &[0x20, 0x00, 0x00, 0x00]),
            (0xF0F0_4014, &[0x20, 0x00, 0x00, 0x00]),
        ],
    },
    // PGR_Flea3.xml, line 96: the `MemSaveVal_Int` key to `MemorySave_Reg`
    // at 0xF0F00618, as on the Blackfly above.
    CommandCase {
        document: "PGR_Flea3.xml",
        device: &[(0xF0F0_0400, &[0x00, 0x00, 0x00, 0x02])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetSave",
        writes: &[
            (0xF0F0_0620, &[0x20, 0x00, 0x00, 0x00]),
            (0xF0F0_0618, &[0x80, 0x00, 0x00, 0x00]),
        ],
    },
    // Same document: `UserSetLoad` writes `UserSetVal_Int` into bits 0..3 of
    // `LoadMemCh_Reg` at 0xF0F00000 + 0x4014.
    CommandCase {
        document: "PGR_Flea3.xml",
        device: &[(0xF0F0_0400, &[0x00, 0x00, 0x00, 0x02])],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetLoad",
        writes: &[
            (0xF0F0_0620, &[0x20, 0x00, 0x00, 0x00]),
            (0xF0F0_4014, &[0x20, 0x00, 0x00, 0x00]),
        ],
    },
    // JAI_RM-6740GE.xml: the selector has `Default` and `UserSet1` only
    // (13080-13090) and writes `UserSetSelectorReg` at 0xD1AC (13091-13101).
    // `UserSetSave` (13124-13132) writes `UserSetSaveCmdValExpr`, the selector
    // register (13143-13146), to `UserSetSaveReg` at 0xD1B4 (13133-13142).
    CommandCase {
        document: "JAI_RM-6740GE.xml",
        device: &[],
        set: &[Set::Entry("UserSetSelector", "UserSet1")],
        command: "UserSetSave",
        writes: &[
            (0xD1AC, &[0x00, 0x00, 0x00, 0x01]),
            (0xD1B4, &[0x00, 0x00, 0x00, 0x01]),
        ],
    },
    // Same document: loading `Default` writes 0 to `UserSetLoadReg` at 0xD1B0
    // (13102-13123). A constant 1 would load `UserSet1` instead.
    CommandCase {
        document: "JAI_RM-6740GE.xml",
        device: &[],
        set: &[Set::Entry("UserSetSelector", "Default")],
        command: "UserSetLoad",
        writes: &[
            (0xD1AC, &[0x00, 0x00, 0x00, 0x00]),
            (0xD1B0, &[0x00, 0x00, 0x00, 0x00]),
        ],
    },
    // Sony_CVC_EH6300.xml: the selector's `<pValue>` is the floating
    // `UserSetSelectorValue` (4742-4786), so selecting writes nothing.
    // `UserSetSave` (4732-4741) writes it to `RegUserSetSave` at 0x1000004C
    // (3924-3934); its lock, `IsUserSetSaveLocked`, is `SEL=0` (2315-2319).
    CommandCase {
        document: "Sony_CVC_EH6300.xml",
        device: &[],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetSave",
        writes: &[(0x1000_004C, &[0x00, 0x00, 0x00, 0x02])],
    },
    // Same document: `UserSetLoad` (4722-4731) to `RegUserSetLoad` at
    // 0x10000048 (3914-3923).
    CommandCase {
        document: "Sony_CVC_EH6300.xml",
        device: &[],
        set: &[Set::Entry("UserSetSelector", "UserSet2")],
        command: "UserSetLoad",
        writes: &[(0x1000_0048, &[0x00, 0x00, 0x00, 0x02])],
    },
    // AVT_Manta_G125B.xml: `TriggerSoftware` 927-934, `IntTriggerSoftwareCommand`
    // 936-939, `RegSoftwareTrigger` 3418-3425, `RegTriggerSourceInqSoftware`
    // 3526-3534, `AcquisitionStart` = 3 (857). See `AVT_TRIGGER_SOFTWARE`.
    CommandCase {
        document: "AVT_Manta_G125B.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // AVT_Manta_G125B_1.44.0.xml: 1011-1018, 1020-1023, 3968-3975, 4076-4084, 934.
    CommandCase {
        document: "AVT_Manta_G125B_1.44.0.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // AVT_Manta_G125B_1.44.4.xml: 1025-1032, 1034-1037, 3991-3998, 4099-4107, 948.
    CommandCase {
        document: "AVT_Manta_G125B_1.44.4.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // AVT_Manta_G125B_1.44.28514.xml: 1065-1072, 1074-1077, 4069-4076, 4177-4185, 988.
    CommandCase {
        document: "AVT_Manta_G125B_1.44.28514.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // AVT_Manta_G146B.xml: 927-934, 936-939, 3418-3425, 3526-3534, 857.
    CommandCase {
        document: "AVT_Manta_G146B.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // Prosilica_GC1020.xml: 903-909, 911-914, 3164-3171, 3272-3280, 828.
    CommandCase {
        document: "Prosilica_GC1020.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // Prosilica_GC1020C.xml: the same lines as the GC1020.
    CommandCase {
        document: "Prosilica_GC1020C.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // Prosilica_GC1280M.xml: 511-516 (the predicate is `<pIsAvailable>`
    // here), 518-521, 1783-1790, 1881-1889, 483.
    CommandCase {
        document: "Prosilica_GC1280M.xml",
        ..AVT_TRIGGER_SOFTWARE
    },
    // SVS_evo4050.xml: `DefectPixelCorrection_Delete` (823-829) writes the
    // floating `DefectPixelCorrection_MapIndex` (761-772) to
    // `DefectPixelCorrection_Delete_Reg` at 0x10004024 (831-839). The index is
    // bounded by `<pMax>` `DefectPixelCorrection_MapSize_Reg` at 0x1000401C
    // (695-704) and implemented by `hasDefectPixelCorrection`, `<Bit>13</Bit>`
    // of 0xB0DC (554-562; big-endian: 0x00040000).
    CommandCase {
        document: "SVS_evo4050.xml",
        device: &[
            (0xB0DC, &[0x00, 0x04, 0x00, 0x00]),
            (0x1000_401C, &[0x00, 0x00, 0x00, 0x0A]),
        ],
        set: &[Set::Int("DefectPixelCorrection_MapIndex", 5)],
        command: "DefectPixelCorrection_Delete",
        writes: &[(0x1000_4024, &[0x00, 0x00, 0x00, 0x05])],
    },
];

/// A command its own `<pIsAvailable>` refuses in the selected state: nothing
/// may be written past the selector.
struct RefusedCase {
    document: &'static str,
    device: &'static [(u64, &'static [u8])],
    set: &'static [Set],
    command: &'static str,
    /// The selector's own writes, which still happen.
    writes: &'static [(u64, &'static [u8])],
}

const REFUSED_CASES: &[RefusedCase] = &[
    // Hikrobot_MV-CS050-10GC.xml: `UserSetSave`'s `<pIsAvailable>IsDefaultSet`
    // (23039) is `(VAL = 0)? 0:1` over the selector register (23087-23090).
    // `UserSetSelector_Default_Inq` is bit 3 (22965-22967).
    RefusedCase {
        document: "Hikrobot_MV-CS050-10GC.xml",
        device: &[(0x0001_0800, &[0xF0, 0x00, 0x00, 0x00])],
        set: &[Set::Entry("UserSetSelector", "Default")],
        command: "UserSetSave",
        writes: &[(0x0003_1804, &[0x00, 0x00, 0x00, 0x00])],
    },
    // JAI_RM-6740GE.xml: `UserSetSaveAvailExpr` (13147-13151) is
    // `(VAR_USERSETSELECTOR > 0) || (VAR_ISPWMODE)`; `IsPWModeVal` is 0 (3144-3146).
    RefusedCase {
        document: "JAI_RM-6740GE.xml",
        device: &[],
        set: &[Set::Entry("UserSetSelector", "Default")],
        command: "UserSetSave",
        writes: &[(0xD1AC, &[0x00, 0x00, 0x00, 0x00])],
    },
];

/// Features written on one document, with every write they must cause.
struct WriteCase {
    document: &'static str,
    device: &'static [(u64, &'static [u8])],
    set: &'static [Set],
    writes: &'static [(u64, &'static [u8])],
}

const WRITE_CASES: &[WriteCase] = &[
    // Basler_a2A4096_44g5m.xml (#210): `TriggerSelector` (1205) writes the
    // floating `d_88` (1046-1048), so selecting writes nothing. `TriggerMode`
    // (1270) delegates to `d_90` (1059), whose `<pIndex>d_88</pIndex>` selects
    // `<pValueIndexed Index="3">d_257</pValueIndexed>`: `FrameStart` is 3, and
    // `d_257` is at 0x10002060 (11399, BigEndian). `On` is available through
    // `d_150` (26211), entry 3 `d_1613` at 0x10001FE4 (19866); the lock
    // `d_90_lockedMux` (1049) is `d_890` at 0x100087B4 (8188), zero. Read as an
    // address term, the `<pIndex>` put `TriggerMode` at 0 + 3 × 4 = 0x000C.
    WriteCase {
        document: "Basler_a2A4096_44g5m.xml",
        device: &[(0x1000_1FE4, &[0x00, 0x00, 0x00, 0x01])],
        set: &[
            Set::Entry("TriggerSelector", "FrameStart"),
            Set::Entry("TriggerMode", "On"),
        ],
        writes: &[(0x1000_2060, &[0x00, 0x00, 0x00, 0x01])],
    },
    // Same document: `FrameBurstStart` is 0, which no `d_90` entry names, so
    // `<pValueDefault>d_1049</pValueDefault>` at 0x10001FBC (11367); its lock
    // is `d_90_lockedMux`'s `<ValueDefault>0</ValueDefault>`.
    WriteCase {
        document: "Basler_a2A4096_44g5m.xml",
        device: &[],
        set: &[
            Set::Entry("TriggerSelector", "FrameBurstStart"),
            Set::Entry("TriggerMode", "Off"),
        ],
        writes: &[(0x1000_1FBC, &[0x00, 0x00, 0x00, 0x00])],
    },
];

/// A register read whose bytes decode to a different number in the other
/// byte order.
struct ReadCase {
    document: &'static str,
    device: &'static [(u64, &'static [u8])],
    feature: &'static str,
    value: i64,
}

const READ_CASES: &[ReadCase] = &[
    // Hikrobot_MV-CS050-10GC.xml: `PayloadSize` (23304-23311) is
    // `PayloadSize_Reg` (23312-23334, BigEndian, Unsigned) at
    // `PayloadSize_RegAddr` 0x00031900 (1053-1055). 0x004C8000 = 5 013 504,
    // a 2448 × 2048 Mono8 frame; read little-endian it would be 0x00804C00.
    ReadCase {
        document: "Hikrobot_MV-CS050-10GC.xml",
        device: &[(0x0003_1900, &[0x00, 0x4C, 0x80, 0x00])],
        feature: "PayloadSize",
        value: 5_013_504,
    },
    // PGR_GS3_U3_23S6M.xml, line 130: `PayloadSize` is `PayloadSizeVal_Reg`,
    // `LittleEndian`, at `CamRegBaseAddress` 0xFFFFF0F00000 + 0x5410. The same
    // number, least significant byte first.
    ReadCase {
        document: "PGR_GS3_U3_23S6M.xml",
        device: &[(0xFFFF_F0F0_5410, &[0x00, 0x80, 0x4C, 0x00])],
        feature: "PayloadSize",
        value: 5_013_504,
    },
];

/// Load `document` from the corpus, or `None` with a note when it is absent —
/// the corpus is fetched, and `VIVA_GENICAM_XML_CORPUS` may point at a
/// directory of the user's own.
fn corpus_nodemap(document: &str) -> Option<NodeMap> {
    let path = corpus_dir().join(document);
    let Ok(bytes) = std::fs::read(&path) else {
        println!("{} not present; skipping its expectations", path.display());
        return None;
    };
    let xml = String::from_utf8_lossy(&bytes);
    let model = viva_genapi_xml::parse(&xml).expect("parse");
    Some(NodeMap::try_from_xml(model).expect("build"))
}

fn apply(nodemap: &mut NodeMap, set: &[Set], io: &DeviceIo) -> Result<(), String> {
    for step in set {
        match *step {
            Set::Entry(name, entry) => nodemap
                .set_enum(name, entry, io)
                .map_err(|e| format!("{name} = {entry}: {e}"))?,
            Set::Int(name, value) => nodemap
                .set_integer(name, value, io)
                .map_err(|e| format!("{name} = {value}: {e}"))?,
        }
    }
    Ok(())
}

fn owned(writes: &[(u64, &[u8])]) -> Vec<(u64, Vec<u8>)> {
    writes
        .iter()
        .map(|(addr, bytes)| (*addr, bytes.to_vec()))
        .collect()
}

/// GA-11 and GA-35: what executing a command writes, byte for byte, on every
/// corpus document whose commands take their value from `<pCommandValue>` —
/// 19 of 38 — with a non-default selector value wherever the document has one.
/// A constant `1` saves or loads the wrong user set on most of them.
#[test]
fn corpus_commands_write_the_value_their_document_names() {
    let mut failures = Vec::new();
    for case in COMMAND_CASES {
        let Some(mut nodemap) = corpus_nodemap(case.document) else {
            continue;
        };
        let io = DeviceIo::with(case.device);
        let label = format!("{} {}", case.document, case.command);
        if let Err(err) = apply(&mut nodemap, case.set, &io) {
            failures.push(format!("{label}: selecting: {err}"));
            continue;
        }
        if let Err(err) = nodemap.exec_command(case.command, &io) {
            failures.push(format!("{label}: {err}"));
            continue;
        }
        let writes = io.writes.borrow().clone();
        if writes != owned(case.writes) {
            failures.push(format!(
                "{label}: wrote {writes:02X?}, expected {:02X?}",
                owned(case.writes)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// GA-10: a command whose own `<pIsAvailable>` is false in the selected state
/// is refused before anything reaches its register.
#[test]
fn corpus_commands_are_refused_where_their_document_says_so() {
    let mut failures = Vec::new();
    for case in REFUSED_CASES {
        let Some(mut nodemap) = corpus_nodemap(case.document) else {
            continue;
        };
        let io = DeviceIo::with(case.device);
        let label = format!("{} {}", case.document, case.command);
        if let Err(err) = apply(&mut nodemap, case.set, &io) {
            failures.push(format!("{label}: selecting: {err}"));
            continue;
        }
        match nodemap.exec_command(case.command, &io) {
            Err(GenApiError::Unavailable(_)) => {}
            other => failures.push(format!("{label}: expected Unavailable, got {other:?}")),
        }
        let writes = io.writes.borrow().clone();
        if writes != owned(case.writes) {
            failures.push(format!(
                "{label}: wrote {writes:02X?}, expected only the selector's {:02X?}",
                owned(case.writes)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What writing a feature sends, byte for byte, where its document routes the
/// write through something other than a plain `<pValue>` chain.
#[test]
fn corpus_writes_reach_the_register_their_document_names() {
    let mut failures = Vec::new();
    for case in WRITE_CASES {
        let Some(mut nodemap) = corpus_nodemap(case.document) else {
            continue;
        };
        let io = DeviceIo::with(case.device);
        if let Err(err) = apply(&mut nodemap, case.set, &io) {
            failures.push(format!("{}: {err}", case.document));
            continue;
        }
        let writes = io.writes.borrow().clone();
        if writes != owned(case.writes) {
            failures.push(format!(
                "{}: wrote {writes:02X?}, expected {:02X?}",
                case.document,
                owned(case.writes)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Basler_a2A4096_44g5m.xml (#210): `TriggerSoftware` (1479-1487) writes
/// through `d_138` (1124-1135), whose `<pIndex>d_88</pIndex>` (1046-1048, 3:
/// `FrameStart`) selects `d_1261` at 0x100023D8 (14891, `WO`). Every node
/// `d_138` can select is written by the command and holds no value: the
/// `WO` registers `d_1349` (16001), `d_1261`, `d_1350` (16008), `d_999`
/// (9544), `d_1254` (14783) and `d_1260` (14884), and the constants `d_131`,
/// `d_134`, `d_137` (1108-1116). The command imposes `WO` (1484), so it is
/// done at once and nothing is read back.
#[test]
fn corpus_indexed_command_targets_are_command_targets() {
    let Some(mut nodemap) = corpus_nodemap("Basler_a2A4096_44g5m.xml") else {
        return;
    };
    let mut missing: Vec<&str> = [
        "d_138", "d_1349", "d_131", "d_1261", "d_1350", "d_134", "d_999", "d_1254", "d_137",
        "d_1260",
    ]
    .into_iter()
    .filter(|name| !nodemap.command_targets().contains(*name))
    .collect();
    missing.sort();
    assert!(missing.is_empty(), "not command targets: {missing:?}");
    assert!(
        !nodemap.command_targets().contains("d_88"),
        "the index is the trigger selector, not a target"
    );

    let io = DeviceIo::default();
    nodemap
        .exec_command("TriggerSoftware", &io)
        .expect("execute");
    assert_eq!(
        io.writes.borrow().clone(),
        owned(&[(0x1000_23D8, &[0x00, 0x00, 0x00, 0x01])])
    );
    assert!(nodemap.is_done("TriggerSoftware", &io).expect("done"));
    assert!(
        !io.reads.borrow().contains(&0x1000_23D8),
        "{:X?}",
        io.reads.borrow()
    );
}

/// Whether `name` takes its length from a `<pLength>` node.
fn has_length_node(nodemap: &NodeMap, name: &str) -> bool {
    match nodemap.node(name) {
        Some(viva_genapi::Node::Register(reg)) => reg.length_node().is_some(),
        Some(viva_genapi::Node::String(string)) => string.length_node().is_some(),
        _ => false,
    }
}

/// GA-11: a register reads in the byte order its document declares.
#[test]
fn corpus_registers_read_in_their_declared_byte_order() {
    let mut failures = Vec::new();
    for case in READ_CASES {
        let Some(nodemap) = corpus_nodemap(case.document) else {
            continue;
        };
        let io = DeviceIo::with(case.device);
        match nodemap.get_integer(case.feature, &io) {
            Ok(value) if value == case.value => {}
            other => failures.push(format!(
                "{} {}: expected {}, got {other:?}",
                case.document, case.feature, case.value
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// GA-12: registers on chunk and event ports
// ---------------------------------------------------------------------------

/// What a `<Port>` gives access to, as this test reads the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PortClass {
    /// `<ChunkID>`: data inside a stream buffer.
    Chunk,
    /// `<EventID>`: data inside an event message.
    Event,
}

/// A register bound by `<pPort>` to a chunk or event port.
#[derive(Debug)]
struct PortRegister {
    /// Element name: `IntReg`, `MaskedIntReg`, `FloatReg`, `StringReg`,
    /// `Register`, or `StructEntry`.
    tag: String,
    name: String,
    port: String,
    class: PortClass,
    /// The sum of the literal `<Address>` terms, when the register has no
    /// `<pAddress>` or `<pIndex>`: the address a device-port read would use.
    literal_address: Option<i64>,
}

/// Find every register bound to a chunk or event port, with a reader of its
/// own rather than through `viva-genapi-xml`, so that the test can disagree
/// with the parser about which registers those are.
fn port_registers(xml: &str) -> Vec<PortRegister> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    #[derive(Default)]
    struct Pending {
        tag: String,
        name: String,
        port: Option<String>,
        address: i64,
        indirect: bool,
        entries: Vec<String>,
    }

    let xml = xml.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    reader.config_mut().allow_dangling_amp = true;
    let mut ports: HashMap<String, PortClass> = HashMap::new();
    let mut registers: Vec<Pending> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut port: Option<String> = None;
    let mut current: Option<Pending> = None;
    let mut text = String::new();

    let name_of = |e: &quick_xml::events::BytesStart<'_>| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == "Name")
            .and_then(|a| {
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.trim().to_string())
            })
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = e.name().as_ref().to_string();
                match tag.as_str() {
                    "Port" => port = name_of(&e),
                    "IntReg" | "MaskedIntReg" | "FloatReg" | "StringReg" | "Register"
                    | "StructReg"
                        if current.is_none() =>
                    {
                        current = Some(Pending {
                            tag: tag.clone(),
                            name: name_of(&e).unwrap_or_default(),
                            ..Pending::default()
                        });
                    }
                    "StructEntry" => {
                        if let (Some(reg), Some(name)) = (current.as_mut(), name_of(&e)) {
                            reg.entries.push(name);
                        }
                    }
                    _ => {}
                }
                stack.push(tag);
                text.clear();
            }
            Ok(Event::Text(t)) => text.push_str(&t),
            Ok(Event::End(_)) => {
                let tag = stack.pop().unwrap_or_default();
                let value = text.trim().to_string();
                text.clear();
                let parent = stack.last().map(String::as_str);
                match (tag.as_str(), parent) {
                    ("ChunkID", Some("Port")) => {
                        if let Some(p) = &port {
                            ports.insert(p.clone(), PortClass::Chunk);
                        }
                    }
                    ("EventID", Some("Port")) => {
                        if let Some(p) = &port {
                            ports.insert(p.clone(), PortClass::Event);
                        }
                    }
                    ("Port", _) => port = None,
                    ("pPort", _) => {
                        if let Some(reg) = current.as_mut() {
                            reg.port = Some(value);
                        }
                    }
                    ("Address", _) => {
                        if let Some(reg) = current.as_mut() {
                            let literal = if let Some(hex) = value.strip_prefix("0x") {
                                u64::from_str_radix(hex, 16).map(|v| v as i64).ok()
                            } else {
                                value.parse::<i64>().ok()
                            };
                            match literal {
                                Some(v) => reg.address = reg.address.wrapping_add(v),
                                None => reg.indirect = true,
                            }
                        }
                    }
                    ("pAddress" | "pIndex", _) => {
                        if let Some(reg) = current.as_mut() {
                            reg.indirect = true;
                        }
                    }
                    (t, _) if current.as_ref().is_some_and(|reg| reg.tag == t) => {
                        registers.push(current.take().expect("checked"));
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    let mut found = Vec::new();
    for reg in registers {
        let Some(port) = reg.port else { continue };
        let Some(&class) = ports.get(&port) else {
            continue;
        };
        let literal_address = (!reg.indirect).then_some(reg.address);
        if reg.tag == "StructReg" {
            for entry in reg.entries {
                found.push(PortRegister {
                    tag: "StructEntry".into(),
                    name: entry,
                    port: port.clone(),
                    class,
                    literal_address,
                });
            }
        } else {
            found.push(PortRegister {
                tag: reg.tag,
                name: reg.name,
                port,
                class,
                literal_address,
            });
        }
    }
    found
}

/// A device that answers every read with zeros and remembers each address.
#[derive(Default)]
struct RecordingIo {
    reads: RefCell<Vec<(u64, usize)>>,
}

impl RegisterIo for RecordingIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        self.reads.borrow_mut().push((addr, len));
        Ok(vec![0; len])
    }

    fn write(&self, _addr: u64, _data: &[u8]) -> Result<(), GenApiError> {
        Ok(())
    }
}

/// Read `name` through whichever accessor its node kind takes.
fn read_any(nodemap: &NodeMap, name: &str, io: &dyn RegisterIo) -> Result<(), GenApiError> {
    use viva_genapi::Node;
    match nodemap.node(name) {
        Some(Node::String(_)) => nodemap.get_string(name, io).map(drop),
        Some(Node::Register(_)) => nodemap.get_register(name, io).map(drop),
        Some(_) => nodemap
            .get_float(name, io)
            .map(drop)
            .or_else(|_| nodemap.get_integer(name, io).map(drop)),
        None => Err(GenApiError::NodeNotFound(name.to_string())),
    }
}

/// GA-12: a register on a chunk or event port addresses the chunk or the
/// event, not the device. With nothing attached, reading one must fail — and
/// must not reach device memory at its port-relative address, which is where
/// such a read used to land: 294 event-port and 229 chunk-port registers in
/// 20 vendor documents, `EventExposureEndTimestamp` among them, read GVCP
/// bootstrap registers at offsets like 0x8 and called it a timestamp.
#[test]
fn corpus_port_registers_never_read_device_memory() {
    let dir = corpus_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("corpus not present at {}; skipping", dir.display());
        return;
    };
    let mut documents: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    documents.sort();

    let mut failures = Vec::new();
    let mut with_ports = 0usize;
    let mut counts: BTreeMap<(PortClass, String), usize> = BTreeMap::new();
    let mut not_built = 0usize;
    for path in &documents {
        let document = path
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned();
        let xml = String::from_utf8_lossy(&std::fs::read(path).expect("read")).into_owned();
        let registers = port_registers(&xml);
        if registers.is_empty() {
            continue;
        }
        with_ports += 1;
        let model = viva_genapi_xml::parse(&xml).expect("parse");
        let mut nodemap = NodeMap::try_from_xml(model).expect("build");
        let mut device_reads = 0usize;
        for reg in &registers {
            *counts.entry((reg.class, reg.tag.clone())).or_default() += 1;
            if nodemap.node(&reg.name).is_none() {
                // Dropped for a reason the build test already polices.
                not_built += 1;
                continue;
            }
            let io = RecordingIo::default();
            let result = read_any(&nodemap, &reg.name, &io);
            let reads = io.reads.borrow().clone();
            device_reads += reads.len();
            let at_own_address = reg
                .literal_address
                .is_some_and(|addr| reads.iter().any(|(a, _)| *a as i64 == addr));
            if at_own_address {
                failures.push(format!(
                    "{document}: <{}> {} on {:?} port {} read device memory at {:#x}",
                    reg.tag,
                    reg.name,
                    reg.class,
                    reg.port,
                    reg.literal_address.unwrap_or_default()
                ));
            } else if result.is_ok() {
                failures.push(format!(
                    "{document}: <{}> {} on {:?} port {} read a value with nothing attached",
                    reg.tag, reg.name, reg.class, reg.port
                ));
            }
        }
        println!(
            "{document}: {} port registers, {device_reads} device reads while reading them",
            registers.len()
        );

        // With data attached to every chunk and event port, every one of
        // those registers reads — from the attached bytes, still without
        // touching the device at its own address.
        let pattern = bytes::Bytes::from((0..1024u32).map(|i| i as u8).collect::<Vec<u8>>());
        let mut chunks = viva_genapi::ChunkData::new();
        let kinds: Vec<viva_genapi::PortKind> =
            nodemap.ports().into_iter().map(|(_, kind)| kind).collect();
        for kind in kinds {
            match kind {
                viva_genapi::PortKind::Chunk { chunk_id } => chunks.push(chunk_id, pattern.clone()),
                viva_genapi::PortKind::Event { event_id } => {
                    nodemap.attach_event(event_id, pattern.clone())
                }
                _ => {}
            }
        }
        nodemap.attach_chunks(&chunks);
        for reg in &registers {
            if nodemap.node(&reg.name).is_none() {
                continue;
            }
            let io = RecordingIo::default();
            let result = read_any(&nodemap, &reg.name, &io);
            let at_own_address = reg
                .literal_address
                .is_some_and(|addr| io.reads.borrow().iter().any(|(a, _)| *a as i64 == addr));
            // A register whose `<pLength>` node reads the pattern gets the
            // pattern's length, which no 1 024-byte chunk holds: out of
            // range is the honest answer (Baumer's `ChunkImage`, FLIR's
            // `ChunkSerialData`).
            let length_from_data = matches!(result, Err(GenApiError::PortOutOfRange { .. }))
                && has_length_node(&nodemap, &reg.name);
            if at_own_address || (result.is_err() && !length_from_data) {
                failures.push(format!(
                    "{document}: <{}> {} with data attached: {result:?}, device read at own \
                     address: {at_own_address}",
                    reg.tag, reg.name
                ));
            }
        }
    }

    let chunk: usize = counts
        .iter()
        .filter(|((class, _), _)| *class == PortClass::Chunk)
        .map(|(_, n)| n)
        .sum();
    let event: usize = counts
        .iter()
        .filter(|((class, _), _)| *class == PortClass::Event)
        .map(|(_, n)| n)
        .sum();
    println!(
        "\n{with_ports} documents declare chunk or event ports: {chunk} chunk-port and \
         {event} event-port registers ({not_built} not built); by tag: {counts:?}"
    );
    assert!(
        failures.is_empty(),
        "{} port registers reached device memory or read without data:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// GA-14: the save plan of every document
// ---------------------------------------------------------------------------

/// Byte-addressed memory that keeps what is written, so a selector set during
/// a save reads back as set, and one put back reads back as put back.
#[derive(Default)]
struct ByteMemory {
    bytes: RefCell<HashMap<u64, u8>>,
}

impl RegisterIo for ByteMemory {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let bytes = self.bytes.borrow();
        Ok((0..len as u64)
            .map(|i| bytes.get(&(addr + i)).copied().unwrap_or(0))
            .collect())
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        let mut bytes = self.bytes.borrow_mut();
        for (i, byte) in data.iter().enumerate() {
            bytes.insert(addr + i as u64, *byte);
        }
        Ok(())
    }
}

/// Whether `to` is reachable from `from` along the plan's own edges (each
/// feature to the features listed `after` it): an edge whose target reaches
/// back to its source is part of a cycle, which the plan orders by document.
fn reaches(successors: &HashMap<&str, Vec<&str>>, from: &str, to: &str) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut pending = vec![from];
    while let Some(current) = pending.pop() {
        if current == to {
            return true;
        }
        if seen.insert(current) {
            pending.extend(successors.get(current).into_iter().flatten().copied());
        }
    }
    false
}

/// GA-14: for every document, the save plan built from its XML, and a save
/// against a memory stub.
///
/// - The restore order respects every dependency except within a cycle. The
///   dependencies are found here from the public API — a selector's
///   `<pSelected>`, a `pIsImplemented`/`pIsAvailable`/`pIsLocked` provider, a
///   direct `dependents` link — independently of how the plan finds them, and
///   each must also be among the plan's own (`after`).
/// - The save terminates, and every selector it iterated reads back as it
///   was before. Each selector is first put somewhere a walk does not end —
///   an enumeration on its second-to-last entry, an integer on its minimum —
///   and in range: memory starts at zero, which is outside many selectors'
///   bounds, and a value out of range cannot be written back.
#[test]
fn corpus_save_plans_respect_dependencies_and_put_selectors_back() {
    let dir = corpus_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("corpus not present at {}", dir.display());
        return;
    };
    let mut documents: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    documents.sort();

    let mut failures = Vec::new();
    let (mut planned, mut selected, mut edges, mut cyclic) = (0usize, 0, 0, 0);
    let (mut values, mut omitted, mut selectors_checked) = (0usize, 0, 0);
    let mut with_plan = 0usize;
    for path in &documents {
        let document = path.file_name().unwrap().to_string_lossy().into_owned();
        let xml = String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned();
        let Ok(model) = viva_genapi_xml::parse(&xml) else {
            continue;
        };
        let Ok(mut map) = NodeMap::try_from_xml(model) else {
            continue;
        };
        let plan = map.persistence_plan();
        if plan.features().is_empty() {
            continue;
        }
        with_plan += 1;
        planned += plan.features().len();
        selected += plan
            .features()
            .iter()
            .filter(|f| !f.selectors.is_empty())
            .count();

        // The plan's own graph, for telling a cycle from a misordering.
        let mut successors: HashMap<&str, Vec<&str>> = HashMap::new();
        for feature in plan.features() {
            for provider in &feature.after {
                successors
                    .entry(provider.as_str())
                    .or_default()
                    .push(feature.name.as_str());
            }
        }
        for feature in plan.features() {
            let name = feature.name.as_str();
            let node = map.node(name).expect("planned node exists");
            let mut providers: Vec<String> = map
                .selectors_of(name)
                .into_iter()
                .map(str::to_string)
                .collect();
            providers.extend(node.predicates().references().map(str::to_string));
            providers.extend(
                plan.features()
                    .iter()
                    .filter(|p| map.dependents(&p.name).iter().any(|d| d == name))
                    .map(|p| p.name.clone()),
            );
            providers.sort();
            providers.dedup();
            for provider in providers {
                let (Some(p), Some(f)) = (plan.position(&provider), plan.position(name)) else {
                    continue;
                };
                if p == f {
                    continue;
                }
                edges += 1;
                if !feature.after.contains(&provider) {
                    failures.push(format!(
                        "{document}: {name} depends on {provider}, which the plan misses"
                    ));
                }
                if reaches(&successors, name, &provider) {
                    cyclic += 1;
                } else if p > f {
                    failures.push(format!(
                        "{document}: {provider} must be written before {name}"
                    ));
                }
            }
        }

        // Save against memory, each selector first moved off its first entry.
        let io = ByteMemory::default();
        let mut iterated: Vec<String> = plan
            .features()
            .iter()
            .flat_map(|f| f.selectors.iter().cloned())
            .collect();
        iterated.sort();
        iterated.dedup();
        for selector in &iterated {
            if let Ok(entries) = map.available_enum_entries(selector, &io) {
                if entries.len() >= 2 {
                    let _ = map.set_enum(selector, &entries[entries.len() - 2], &io);
                }
            } else if let Ok(state) = map.feature_state(selector, &io)
                && let Some(range) = state.range
            {
                let _ = map.set_integer(selector, range.min as i64, &io);
            }
        }
        map.invalidate_all();
        let before: Vec<(String, String)> = iterated
            .iter()
            .filter_map(|s| Some((s.clone(), map.get_text(s, &io).ok()?)))
            .collect();
        let saved = map.save_configuration(&io);
        map.invalidate_all();
        for (selector, value) in &before {
            selectors_checked += 1;
            match map.get_text(selector, &io) {
                Ok(after) if &after == value => {}
                other => failures.push(format!(
                    "{document}: {selector} was {value} before the save, {other:?} after"
                )),
            }
        }
        values += saved.values.len();
        omitted += saved.omitted.len();
        // What was saved parses back as it was written.
        let reparsed = viva_genapi::Configuration::parse(&saved.to_text());
        match reparsed {
            Ok(back) if back.values == saved.values => {}
            other => failures.push(format!(
                "{document}: the saved file does not read back: {:?}",
                other.map(|c| c.values.len())
            )),
        }
        println!(
            "ok    {document}: {} planned, {} values, {} left out",
            plan.features().len(),
            saved.values.len(),
            saved.omitted.len()
        );
    }
    println!(
        "=== save plans: {with_plan} documents, {planned} streamable features ({selected} under \
         selectors), {edges} dependencies ({cyclic} within cycles), {selectors_checked} selectors \
         put back; against memory {values} values saved, {omitted} left out; {} failures ===",
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The nodes of `model` that are indexed values (GenICam Standard v2.1.1
/// §2.8.5), or take their value through one by `<pValue>`.
fn read_through_indexed(model: &viva_genapi_xml::XmlModel) -> std::collections::HashSet<String> {
    use viva_genapi_xml::NodeDecl;
    let mut pvalue: HashMap<&str, &str> = HashMap::new();
    let mut found = std::collections::HashSet::new();
    for node in &model.nodes {
        match node {
            NodeDecl::Integer {
                name,
                indexed: Some(_),
                ..
            }
            | NodeDecl::Float {
                name,
                indexed: Some(_),
                ..
            } => {
                found.insert(name.clone());
            }
            NodeDecl::Integer {
                name,
                pvalue: Some(target),
                ..
            }
            | NodeDecl::Float {
                name,
                pvalue: Some(target),
                ..
            }
            | NodeDecl::Enum {
                name,
                pvalue: Some(target),
                ..
            }
            | NodeDecl::Boolean {
                name,
                pvalue: Some(target),
                ..
            } => {
                pvalue.insert(name, target);
            }
            _ => {}
        }
    }
    for start in pvalue.keys() {
        let mut current = *start;
        let mut seen = std::collections::HashSet::new();
        while let Some(next) = pvalue.get(current).filter(|next| seen.insert(**next)) {
            if found.contains(*next) {
                found.insert(start.to_string());
                break;
            }
            current = next;
        }
    }
    found
}

/// GA-05: every value a document declares `NoCache` — on its register, or on
/// a register it is read through — is read from the device every time, not
/// answered from a cache. Before 0.9 every one of them was cached until
/// something invalidated it, so a temperature or an auto-adjusted exposure
/// read twice came back the same.
#[test]
fn corpus_no_cache_values_are_read_from_the_device_every_time() {
    use viva_genapi::CachingMode;
    let dir = corpus_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("corpus not present at {}; skipping", dir.display());
        return;
    };
    let mut documents: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    documents.sort();

    let mut failures = Vec::new();
    let (mut checked, mut inherited, mut polled, mut indexed) = (0usize, 0usize, 0usize, 0usize);
    for path in &documents {
        let document = path
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned();
        let xml = String::from_utf8_lossy(&std::fs::read(path).expect("read")).into_owned();
        let model = viva_genapi_xml::parse(&xml).expect("parse");
        let through_indexed = read_through_indexed(&model);
        let nodemap = NodeMap::try_from_xml(model).expect("build");
        let io = RecordingIo::default();
        for name in nodemap.node_names() {
            polled += usize::from(nodemap.polling_time(name).is_some());
            if nodemap.caching_mode(name) != Some(CachingMode::NoCache)
                || nodemap.command_targets().contains(name)
                || nodemap.is_write_only(name)
            {
                continue;
            }
            // An indexed value is `NoCache` when any node it can select is;
            // the one its index selects now may be cached. Each of those
            // nodes is checked on its own.
            if through_indexed.contains(name) {
                indexed += 1;
                continue;
            }
            // A value on a chunk or event port, or one zeros do not decode,
            // is not a read this test can repeat.
            if read_any(&nodemap, name, &io).is_err() {
                continue;
            }
            let before = io.reads.borrow().len();
            if read_any(&nodemap, name, &io).is_err() {
                continue;
            }
            checked += 1;
            let declared = nodemap.node(name).and_then(|node| node.meta().cachable);
            inherited += usize::from(declared != Some(CachingMode::NoCache));
            if io.reads.borrow().len() == before {
                failures.push(format!("{document}: {name} answered from a cache"));
            }
        }
    }
    println!(
        "=== NoCache: {checked} values read twice, {inherited} of them NoCache through a \
         register they read; {indexed} read through an indexed value; {polled} nodes with a \
         polling time ==="
    );
    assert!(
        failures.is_empty(),
        "{} NoCache value(s) served from a cache:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
