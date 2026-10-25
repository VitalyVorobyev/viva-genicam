# Licensing and provenance review

Reviewed 2026-10-25. This is an engineering inventory and risk record, not a
legal clearance opinion. The maintainer reports no known A3 license or accepted
specification-download commitments. Absence of a known agreement is not proof
that no relevant rights or obligations exist.

## Rights and unresolved questions

The project's independently authored code remains under MIT. MIT permits
commercial use of that code; it does not grant third-party trademark rights,
relicense dependencies, certify a product, or resolve standards-related patent
or contractual claims.

**A3:** the published [GigE Vision licensing policy](https://www.automate.org/vision/vision-standards/gige-vision-license-product-registration)
and [USB3 Vision licensing policy](https://www.automate.org/vision/vision-standards/vision-standards-usb3-vision-license-product-registration)
expressly address software developed for distribution, alongside commercial
products and name/logo use. Their limited internal-integration exceptions do not
describe a public library release. Whether, and on which legal basis, those
policies bind this independent implementation remains unresolved. Neither free
distribution nor an affiliation disclaimer settles that question. The project
does not claim A3 registration, certification, or endorsement. Discuss the
[unsent clarification brief](a3-clarification-draft.md) with qualified counsel or
obtain written clarification before treating downstream distribution as cleared.

**EMVA:** GenICam is administered by EMVA, separately from A3's transport
standards. The [GenICam License v1.6](https://www.emva.org/wp-content/uploads/GenICam_License_20180629.pdf)
distinguishes standard documents, conformity claims, and the reference
implementation. Its reference runtime uses a modified BSD license with a
no-modification restriction; it is not ordinary BSD/MIT. Chapter 6 addresses
redistribution of SFNC/GenTL SFNC material. Availability of documents for free is
not a blanket permission to copy their text or relicense their implementation.
The independent Rust implementation and use of EMVA software for testing must
remain distinguishable. Do not call hardware smoke tests formal certification.

Copyright protects expression; interoperability facts and implementation code
are different review subjects. The [EU Software Directive](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32009L0024)
distinguishes underlying ideas/interfaces from program expression and places
conditions on interoperability exceptions. This does not establish that every
table, manual, implementation technique, or download agreement is unrestricted,
nor answer patent or non-EU questions.

## Repository audit and limits

Inspection covered tracked filenames, relevant source attributions, dependency
manifests, fetch scripts, release packaging, and introduction/history of the
GenTL ABI, PFNC tables, and artwork. It did not perform exhaustive textual
similarity analysis of every historical revision, a patent search, or ownership
verification of every contribution. **No retrospective clean-room claim is made.**

| Material | Observed provenance and handling | Remaining review |
|---|---|---|
| Transport/GenApi implementation | ADR-0011 prohibits copying/porting aravis. ADRs cite aravis and Wireshark as comparative evidence; these citations are not proof of copying or proof of independence. | Review suspicious similarities individually; record source, exact revision, facts used, and independently written behavior. Never erase provenance comments to imply independence. |
| GenTL ABI constants/types | `crates/viva-gentl/src/abi.rs` explicitly says it transcribes GenTL 1.6 and `GenTL.h`; introduced in #204. The official header is fetched, not tracked. | Distinguish necessary names/numbers/layouts from expressive comments or code. Current attribution is evidence, not an automatic copyright exception. |
| SFNC/PFNC | `viva-sfnc` contains names and project-specific helper ordering; `viva-pfnc` has standardized names/numbers plus independent conversion code. PFNC tests explicitly cite copying names/codes from the values document. | Keep exact document/version attribution; review copied prose/table arrangement under applicable EMVA terms. Do not describe the standards' text as MIT. |
| Vendor XML and conformance material | `scripts/fetch-xml-corpus.sh` records sources; corpus excluded from Git. `scripts/fetch-gentl-header.sh` pins the EMVA package and extracts header/schema into build output. | Downloading instead of committing does not itself establish permission. Preserve upstream terms locally; do not republish documents or reports without permission. |
| Tracked XML | The tracked minimal Studio fixture is synthetic project test data; embedded fake-camera XML requires the same provenance discipline as Rust source. | Do not place vendor dumps into synthetic fixtures. |
| Artwork | Viva SVG/PNG lockups entered in the initial release work; Studio icons were imported with Studio in #43. SVG metadata identifies editing tools, not rights ownership. The editable lockup imports Nunito and Fira Code through Google Fonts; the optimized lockup uses paths. No standards-logo license is established by these files. | Maintainer should retain creation/source records and verify font/source-art rights. Do not bundle font binaries without their notices; remote font references are separate from code dependency notices. Visual resemblance alone is not a trademark clearance search. |
| Dependencies | Rust lockfiles, Python metadata and Bun lockfile are separate inventories. `option-ext` has an explicit MPL-2.0 exception in the root policy; Studio also reaches MPL-2.0 `selectors`. Zenoh packages offer Apache-2.0 as an alternative. | Include license/NOTICE texts per artifact; dependency allowlisting does not satisfy distribution obligations on its own. |
| Historical USB wheels | Pre-0.9 wheels linked libusb; changing current dependencies does not cure historical obligations. | Retain the historical LGPL notices and source/relinking compliance record for those releases. |

No tracked third-party header or PDF was found in the current tree. The tracked
art inventory contains project lockups and app icons. This finding is scoped to
the audited tree, not every release archive or every Git object.

## Distribution controls

`scripts/dependency-notices.py --artifact {cli,gentl,python,studio} --out <file>`
collects full packaged license/notice texts for the selected roots and target.
Pass `--target <triple>` for cross-compilation. It follows locked Cargo normal
and build edges, excludes dev-only edges, and includes Studio's installed
production npm dependency graph. Cargo feature unification and build tools can
make this a conservative superset of linked code. It is not a binary scanner.

Some crates omit workspace license files from their published tarball. For those,
the collector retrieves root license/notice/author files at the crate's exact
`.cargo_vcs_info.json` revision, caches them under ignored `target/legal`, and
records immutable source URLs. Four reviewed versions (`keyed-set` 1.1.0,
`token-cell` 2.1.1, `validated_struct` and `validated_struct_macros` 2.2.0)
declare the Apache-2.0 alternative but omit full terms even at the upstream root;
the collector supplies the unmodified Apache text with manifest authors and exact
source revision. Studio's `selectors` 0.38.0 similarly supplies the standard
MPL-2.0 terms, preserving its source copyright notices. `libappindicator-sys`
0.9.0 omits repository metadata; its reviewed override points to the wrapper's
upstream workspace at the sys crate's recorded revision. Other missing or unsupported provenance fails generation.
Review any new failure; do not substitute a guessed license text. Upstream SPDX
alternatives are recorded as declared; our intended Apache-2.0 choice for Zenoh
does not require accepting EPL-2.0 for unrelated dependencies.

Release archives carry their Rust notices; CTI distributions carry a separate
notice sidecar; Python wheels carry notices inside `viva_genicam`; Studio bundles
carry Rust and frontend notices as resources. A small tracked development
placeholder lets clean-checkout debug builds work; release-mode Tauri builds
reject it. Before a local Studio release build, run the collector with `--artifact
studio --out studio/apps/viva-studio-tauri/src-tauri/resources/THIRD_PARTY_NOTICES.txt`.
Release CI performs that step automatically. NumPy is installed separately, not
vendored by the wheel, and supplies its own installed-distribution notices.
Platform system libraries/frameworks also retain their own terms.

Run `scripts/check-distribution.py <archive>...` on prepared archives to verify
notice inclusion and reject private captures, vendor corpus, fetched official
headers/schemas, and local evidence. Keep camera reports/captures outside tracked
sources. Source-crate license metadata is not an A3 permission grant.

## Contribution and release discipline

Keep source citations and copyright notices. Do not import reference code or
vendor text under the project's MIT label. Record provenance and licensing for
new fixtures, copied snippets, generated tables, images, and fonts. Compare wire
behavior through independent tests; a compatible result does not prove how code
was authored. Do not accept gated specification terms for the maintainer or
represent a test result as registration. Revisit this record when written A3
clarification, actual download terms, or source ownership evidence becomes
available.
