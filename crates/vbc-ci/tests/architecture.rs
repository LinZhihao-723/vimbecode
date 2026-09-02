//! The rules the workspace's own source is held to, and the proof that holding it to them bites.
//!
//! Three of these rules pay for the layout the editor is built on. Nothing may keep a cache of
//! what it laid out, because a cache is what an anchored mapping exists to do without and what
//! every edit would then have to invalidate. Nothing outside the tests may lay a whole text out,
//! because that cost is the one the anchor was designed away from. And nothing that draws rows may
//! map them, because a renderer is handed the rows it draws and a renderer that asks where they
//! begin spends a frame's budget finding what it was already holding.
//!
//! Each rule is a property of the code rather than of a run, so each is read off the source. Two
//! things decide whether such a scan is worth anything: what it reads, and what it looks for. It
//! reads every crate of the workspace and every module of every crate, subdirectories included,
//! and it is held to that by the crates and the module declarations the tree itself holds, so a
//! crate or a module added later is covered without anyone remembering to add it here. And it
//! looks for the shape of the offence rather than for a name: a layout is caught by being called
//! once for every line of a text, whatever the function around it is called.
//!
//! What that is worth is not an argument either. The offences are written into a copy of this
//! workspace and the scans are required to find them, so a guard that has stopped covering the
//! code it names fails here rather than passing quietly.

mod guard;

use std::path::{Path, PathBuf};

use guard::fixture::Fixture;
use guard::{Error, Finding};

/// The words naming a type that would hold the text being edited.
const TEXT_WORDS: [&str; 2] = ["buffer", "document"];

/// The one such type the workspace declares, and the file that declares it.
const TEXT_TYPE: &str = "Buffer";
const TEXT_MODULE: &str = "crates/vbc-layout/src/buffer.rs";

/// The seams that must address the text through that one type, each paired with the declaration
/// that shows it does.
const SEAMS: [(&str, &str); 3] = [
    (
        "crates/vbc-layout/src/invariants.rs",
        "pub buffer: &'view Buffer",
    ),
    (
        "crates/vbc-layout/tests/fuzz/harness.rs",
        "pub buffer: Buffer",
    ),
    (
        "crates/vbc-layout/tests/fuzz/reference.rs",
        "view.buffer.lines()",
    ),
];

/// The reference layout the invariant search runs against, which is the one whole-text layout this
/// workspace holds and therefore the one thing the scan for such a layout is known to find.
const REFERENCE_LAYOUT: &str = "crates/vbc-layout/tests/fuzz/reference.rs";

/// A cache of what was laid out, as a module of any crate could hold it.
const CACHE: &str = "\
struct Frame {
    layout_cache: Vec<usize>,
}
";

/// A renderer that lays every line of a text out, which is the shape of the offence rather than
/// the name of it: it implements no layout trait and builds no screen, so nothing about it reads
/// as a whole-document layout until you look at what it does.
const WHOLE_TEXT_LAYOUT: &str = "\
impl Renderer {
    pub fn draw_document(&self, buffer: &mut Buffer, area: Rect, text: &Text) {
        for (index, line) in text.lines.iter().enumerate() {
            let rows = line::lay_out(line, text.width, text.metrics, &text.options);
            self.draw_line(buffer, area, index, &rows);
        }
    }
}
";

/// A gutter that asks the anchor mapping where the rows it draws begin.
const MAPPED_ROWS: &str = "\
impl Gutter {
    fn row_of(&self, text: &Text, at: LogicalPosition) -> usize {
        let offset = visual_offset_from_anchor(&text.lines, text.top, at, &text.wrapping, 1);
        offset.expect(\"a drawn row is mapped\").rows
    }
}
";

/// A crate added to the workspace after these guards were written.
const LATER_CRATE: &str = "\
[package]
name = \"vbc-later\"
version = \"0.0.0\"
edition = \"2021\"
";

#[test]
fn no_source_of_the_workspace_keeps_a_cache_of_what_it_laid_out() {
    let findings = guard::scan_for_caches(&guard::workspace()).expect("the workspace is scanned");

    assert_eq!(Vec::<Finding>::new(), findings);
}

#[test]
fn no_source_of_the_workspace_lays_a_whole_text_out_outside_the_tests() {
    let findings =
        guard::scan_for_whole_text_layouts(&guard::workspace()).expect("the workspace is scanned");

    assert_eq!(Vec::<Finding>::new(), findings);
}

#[test]
fn nothing_that_draws_names_the_mapping_that_would_place_its_rows() {
    let findings = guard::scan_for_mappings_where_rows_are_drawn(&guard::workspace())
        .expect("the workspace is scanned");

    assert_eq!(Vec::<Finding>::new(), findings);
}

#[test]
fn the_scan_for_a_whole_text_layout_finds_the_one_the_tests_hold() {
    let reference = guard::read(&guard::workspace().join(REFERENCE_LAYOUT))
        .expect("the reference layout is readable");

    assert!(
        !guard::whole_text_layouts(&reference).is_empty(),
        "{REFERENCE_LAYOUT} lays no whole text out, so the scan looks for a shape nothing takes"
    );
}

#[test]
fn every_crate_built_to_draw_holds_a_source_the_scan_reads_as_drawing() {
    let root = guard::workspace();
    let drawing = guard::drawing_sources(&root).expect("the workspace is scanned");
    let crates = guard::crates_that_draw(&root).expect("the workspace's manifests are read");

    assert!(
        !crates.is_empty(),
        "no crate of the workspace draws, so the scan for a source that draws looks for nothing"
    );
    for name in crates {
        let crate_root = format!("crates/{name}/");
        assert!(
            drawing.iter().any(|path| path.starts_with(&crate_root)),
            "`{name}` is built to draw and holds no source the scan reads as drawing, so the \
             scan passes over it"
        );
    }
}

#[test]
fn the_scans_read_every_crate_and_every_module_the_workspace_declares() {
    let root = guard::workspace();
    let scanned = guard::sources(&root).expect("the workspace is scanned");

    assert_eq!(Ok(()), guard::coverage(&root, &scanned));
}

#[test]
fn the_workspace_declares_one_type_for_the_text_being_edited() {
    let root = guard::workspace();
    let mut declared = Vec::new();
    for path in guard::sources(&root).expect("the workspace is scanned") {
        let source = guard::read(&path).expect("a source of the workspace is readable");
        for name in declared_types(&source) {
            let lowercase = name.to_lowercase();
            if TEXT_WORDS.iter().any(|word| lowercase.contains(word)) {
                declared.push((guard::relative(&root, &path), name));
            }
        }
    }

    assert_eq!(
        vec![(TEXT_MODULE.to_owned(), TEXT_TYPE.to_owned())],
        declared
    );
}

#[test]
fn the_layout_stack_the_invariants_and_the_fuzz_harness_share_that_type() {
    for (file, declaration) in SEAMS {
        let source =
            guard::read(&guard::workspace().join(file)).expect("a seam of the workspace is read");

        assert!(
            source.contains(declaration),
            "{file} does not hold `{declaration}`, so it addresses the text some other way"
        );
    }
}

#[test]
fn a_cache_added_to_the_editor_is_caught() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append("crates/vbc-editor/src/render.rs", CACHE);
    let findings = guard::scan_for_caches(fixture.root()).expect("the fixture is scanned");

    assert_eq!(
        vec!["crates/vbc-editor/src/render.rs"],
        paths(&findings),
        "a cache in a crate the first of these guards never read went uncaught"
    );
    assert_eq!(vec!["cache"], words(&findings));
    assert_eq!(
        vec![line_of(
            &fixture,
            "crates/vbc-editor/src/render.rs",
            "layout_cache"
        )],
        findings.iter().map(Finding::line).collect::<Vec<usize>>()
    );
}

#[test]
fn a_cache_added_to_a_module_in_a_subdirectory_is_caught() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append("crates/vbc-editor/src/event/reader.rs", CACHE);
    let findings = guard::scan_for_caches(fixture.root()).expect("the fixture is scanned");

    assert_eq!(
        vec!["crates/vbc-editor/src/event/reader.rs"],
        paths(&findings),
        "a cache below a crate's source directory went uncaught"
    );
}

#[test]
fn a_renderer_that_lays_every_line_of_a_text_out_is_caught() {
    // The offence implements no layout trait and names no screen, which is what a scan matching
    // those names let through.
    assert!(!WHOLE_TEXT_LAYOUT.contains("Layout for"));
    assert!(!WHOLE_TEXT_LAYOUT.contains("Screen {"));

    let fixture = Fixture::of(&guard::workspace());
    fixture.append("crates/vbc-editor/src/render.rs", WHOLE_TEXT_LAYOUT);
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(vec!["crates/vbc-editor/src/render.rs"], paths(&findings));
    assert_eq!(vec!["lay_out"], words(&findings));
}

#[test]
fn a_gutter_that_maps_the_rows_it_draws_is_caught() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append("crates/vbc-editor/src/gutter.rs", MAPPED_ROWS);
    let findings = guard::scan_for_mappings_where_rows_are_drawn(fixture.root())
        .expect("the fixture is scanned");

    assert_eq!(
        vec!["crates/vbc-editor/src/gutter.rs"],
        paths(&findings),
        "a mapping in a module the first of these guards never read went uncaught"
    );
    assert_eq!(vec!["anchor"], words(&findings));
}

#[test]
fn a_layout_the_tests_reach_for_is_not_an_offence() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append(
        "crates/vbc-editor/src/render.rs",
        &format!("#[cfg(test)]\nmod tests {{\n{WHOLE_TEXT_LAYOUT}}}\n"),
    );
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(Vec::<Finding>::new(), findings);
}

#[test]
fn a_scan_that_would_read_nothing_fails() {
    let nowhere = Fixture::empty();
    let bare = Fixture::empty();
    bare.write("crates/vbc-bare/src/README.md", "no source at all\n");

    for scan in [
        guard::scan_for_caches as fn(&Path) -> Result<Vec<Finding>, Error>,
        guard::scan_for_whole_text_layouts,
        guard::scan_for_mappings_where_rows_are_drawn,
    ] {
        assert_eq!(
            Err(Error::NoCrates {
                root: nowhere.root().display().to_string()
            }),
            scan(nowhere.root())
        );
        assert_eq!(Err(Error::ReadNothing), scan(bare.root()));
    }
}

#[test]
fn a_scan_of_a_tree_it_cannot_read_the_whole_of_fails() {
    let short = Fixture::of(&guard::workspace());
    short.remove("crates/vbc-editor/src/event/reader.rs");
    for scan in [
        guard::scan_for_caches as fn(&Path) -> Result<Vec<Finding>, Error>,
        guard::scan_for_whole_text_layouts,
        guard::scan_for_mappings_where_rows_are_drawn,
    ] {
        assert_eq!(
            Err(Error::UnreadModule {
                path: "crates/vbc-editor/src/event.rs".to_owned(),
                module: "reader".to_owned()
            }),
            scan(short.root())
        );
    }

    let without_reference = Fixture::of(&guard::workspace());
    without_reference.remove(REFERENCE_LAYOUT);
    assert_eq!(
        Err(Error::MissingSource {
            path: REFERENCE_LAYOUT.to_owned()
        }),
        guard::scan_for_caches(without_reference.root())
    );
}

#[test]
fn a_crate_added_to_the_workspace_is_scanned() {
    let root = guard::workspace();
    let fixture = Fixture::of(&root);
    fixture.write("crates/vbc-later/Cargo.toml", LATER_CRATE);
    fixture.write("crates/vbc-later/src/lib.rs", CACHE);

    let workspace_sources = guard::sources(&root).expect("the workspace is scanned");
    let fixture_sources = guard::sources(fixture.root()).expect("the fixture is scanned");
    assert_eq!(workspace_sources.len() + 1, fixture_sources.len());

    let findings = guard::scan_for_caches(fixture.root()).expect("the fixture is scanned");
    assert_eq!(vec!["crates/vbc-later/src/lib.rs"], paths(&findings));
}

#[test]
fn a_scan_that_missed_a_crate_or_a_module_fails() {
    let root = guard::workspace();
    let scanned = guard::sources(&root).expect("the workspace is scanned");

    let without_module: Vec<PathBuf> = scanned
        .iter()
        .filter(|path| !path.ends_with("event/reader.rs"))
        .cloned()
        .collect();
    assert_eq!(
        Err(Error::UnreadModule {
            path: "crates/vbc-editor/src/event.rs".to_owned(),
            module: "reader".to_owned()
        }),
        guard::coverage(&root, &without_module)
    );

    let without_crate: Vec<PathBuf> = scanned
        .iter()
        .filter(|path| !path.starts_with(root.join("crates").join("vbc-oracle")))
        .cloned()
        .collect();
    assert_eq!(
        Err(Error::UnreadCrate {
            name: "vbc-oracle".to_owned()
        }),
        guard::coverage(&root, &without_crate)
    );
}

#[test]
fn a_copy_of_the_workspace_breaks_none_of_these_rules() {
    let fixture = Fixture::of(&guard::workspace());

    assert_eq!(
        Vec::<Finding>::new(),
        guard::scan_for_caches(fixture.root()).expect("the fixture is scanned")
    );
    assert_eq!(
        Vec::<Finding>::new(),
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned")
    );
    assert_eq!(
        Vec::<Finding>::new(),
        guard::scan_for_mappings_where_rows_are_drawn(fixture.root())
            .expect("the fixture is scanned")
    );
}

/// # Returns
///
/// The name of every type a source declares.
fn declared_types(source: &str) -> Vec<String> {
    guard::shape::words(source)
        .windows(2)
        .filter(|pair| ["enum", "struct", "type"].contains(&pair[0].text()))
        .map(|pair| pair[1].text().to_owned())
        .collect()
}

/// # Returns
///
/// The line of one of a fixture's sources that `word` was written on.
///
/// # Panics
///
/// Panics if the source is unreadable, or if it does not hold the word.
fn line_of(fixture: &Fixture, path: &str, word: &str) -> usize {
    let source = guard::read(&fixture.root().join(path)).expect("a source of the fixture is read");

    source
        .lines()
        .position(|line| line.contains(word))
        .expect("the fixture holds the word")
        + 1
}

/// # Returns
///
/// The file every finding was made in.
fn paths(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(Finding::path).collect()
}

/// # Returns
///
/// The word every finding was made on.
fn words(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(Finding::word).collect()
}
