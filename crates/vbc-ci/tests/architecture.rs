//! The rules the workspace's own source is held to, and the proof that holding it to them bites.
//!
//! Four of these rules pay for the layout the editor is built on. Nothing may keep a cache of
//! what it laid out, because a cache is what an anchored mapping exists to do without and what
//! every edit would then have to invalidate. Nothing outside the tests may lay a whole text out,
//! because that cost is the one the anchor was designed away from. Nothing that draws rows may map
//! them, because a renderer is handed the rows it draws and a renderer that asks where they begin
//! spends a frame's budget finding what it was already holding. And nothing that ships may reach
//! for the vocabulary a layout is checked in, because that vocabulary is the language the fuzz
//! search reads a layout in rather than a language the editor is written in.
//!
//! A fifth rule is about what the layout is for rather than what it costs. A module that only its
//! own tests import is a module the application has drifted away from, and a workspace can
//! accumulate a great deal of such work without a single test going red, so every module of every
//! crate is required here to be one a run of a binary can arrive at.
//!
//! Each rule is a property of the code rather than of a run, so each is read off the source. Two
//! things decide whether such a scan is worth anything: what it reads, and what it looks for. It
//! reads every crate of the workspace and every module of every crate, subdirectories included,
//! and it is held to that by the crates and the module declarations the tree itself holds, so a
//! crate or a module added later is covered without anyone remembering to add it here. And it
//! looks for the shape of the offence rather than for a name: a layout is caught by being called
//! once for every line of a text, whatever the function around it is called, and by being called
//! once for every line of one whether the loop's bound is measured in its header or hoisted into a
//! local above it. Reachability is read the same way, off the import graph the binaries are the
//! roots of rather than off a list of the modules somebody remembered to name.
//!
//! What that is worth is not an argument either. The offences are written into a copy of this
//! workspace and the scans are required to find them, so a guard that has stopped covering the
//! code it names fails here rather than passing quietly.
//!
//! Two of the offences are the workspace's own, and both are written down rather than passed over.
//! A rule that a tree already breaks cannot be enforced from the day it is written, but it can be
//! held to exactly what the tree breaks it by, so that the next offence fails and so that fixing
//! one fails until it is struck off the list below.

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

/// The fuzz harness the invariant vocabulary exists for, which is where it is reached for now that
/// nothing that ships reaches for it.
const FUZZ_HARNESS: &str = "crates/vbc-layout/tests/fuzz/harness.rs";

/// The source of the editor an offence is written into, which is the renderer the binary draws
/// with.
const RENDERER: &str = "crates/vbc-editor/src/render.rs";

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

/// A renderer that lays every line of a text out with the bound of the loop hoisted above it,
/// which is the shape the workspace's own block renderer takes. Nothing in the loop's header names
/// the text: the walk runs to `end`, and `end` was measured one line up. A scan that reads only
/// the header sees a walk over two counters, and the call inside it is still made once for every
/// line of a document.
const HOISTED_WHOLE_TEXT_LAYOUT: &str = "\
impl Block {
    pub fn render(&self, window: RowWindow, wrapping: &Wrapping) -> Rendered {
        let source = self.body.source();
        let end = source.len();
        let wanted = window.rows;
        let mut offset = 0;
        let mut drawn = 0;

        while offset <= end && drawn < wanted {
            let text = line_at(source, offset);
            let laid_out = line::lay_out(text, wrapping.width(), wrapping.metrics());
            drawn += laid_out.len();
            offset += text.len();
        }

        Rendered { rows: drawn }
    }
}
";

/// The same offence with the bound passed along one local further: the extent is measured into
/// `end`, and the loop walks to a name `end` was handed to. Nothing the loop reads was measured
/// from the text, and a scan that follows the extent only as far as the name it was first bound to
/// is defeated by writing one more line.
const ALIASED_WHOLE_TEXT_LAYOUT: &str = "\
impl Block {
    pub fn render(&self, window: RowWindow, wrapping: &Wrapping) -> Rendered {
        let source = self.body.source();
        let end = source.len();
        let limit = end;
        let wanted = window.rows;
        let mut offset = 0;
        let mut drawn = 0;

        while offset <= limit && drawn < wanted {
            let text = line_at(source, offset);
            let laid_out = line::lay_out(text, wrapping.width(), wrapping.metrics());
            drawn += laid_out.len();
            offset += text.len();
        }

        Rendered { rows: drawn }
    }
}
";

/// A renderer that walks the rows a window asked for rather than the lines of a text. Its bounds
/// are hoisted into locals exactly as the offence above hoists its own, and the difference is
/// where they were measured: a window, which is bounded, rather than a text, which is not. This is
/// the walk the anchor exists to make possible, so a scan that fires on it has stopped telling the
/// two apart.
const BOUNDED_ROW_WALK: &str = "\
impl Block {
    pub fn render(&self, window: RowWindow, wrapping: &Wrapping) -> Rendered {
        let wanted = window.rows;
        let mut rows = Vec::new();
        let mut at = self.anchored(window.start);
        let mut drawn = 0;

        while drawn < wanted {
            let text = self.row_at(at);
            rows.push(line::lay_out(text, wrapping.width(), wrapping.metrics()));
            drawn += 1;
            at += 1;
        }

        Rendered { rows }
    }
}
";

/// A module nothing at all imports.
const ORPHAN: &str = "\
//! A module the workspace holds and nothing reaches.

pub const NAME: &str = \"orphan\";
";

/// A source reaching for that module, which is what makes it one a run can arrive at.
const REACHES_THE_ORPHAN: &str = "\
use crate::orphan;

pub fn named() -> &'static str {
    orphan::NAME
}
";

/// A source reaching for that module only where its own tests are compiled, which is the shape of
/// every orphan this workspace has accumulated: the module is imported, and by nothing a run
/// arrives at.
const TESTS_REACH_THE_ORPHAN: &str = "\
#[cfg(test)]
mod orphan_tests {
    use crate::orphan;

    #[test]
    fn names_the_orphan() {
        assert_eq!(\"orphan\", orphan::NAME);
    }
}
";

/// A source reaching for that module under an attribute that compiles the import for the tests
/// alone, which is the same offence as an import inside a `#[cfg(test)]` module with no module
/// around it to say so.
const A_TEST_IMPORT_REACHES_THE_ORPHAN: &str = "#[cfg(test)]\nuse crate::orphan;\n";

/// A crate root handing a module of its own out under another name, which is a path a run reaches
/// the module through rather than a declaration of it.
const REEXPORTS_THE_ORPHAN: &str = "pub use crate::orphan::NAME;\n";

/// A gutter that asks the anchor mapping where the rows it draws begin.
const MAPPED_ROWS: &str = "\
impl Gutter {
    fn row_of(&self, text: &Text, at: LogicalPosition) -> usize {
        let offset = visual_offset_from_anchor(&text.lines, text.top, at, &text.wrapping, 1);
        offset.expect(\"a drawn row is mapped\").rows
    }
}
";

/// A gutter reaching for the vocabulary a layout is checked in, which is what ties the type that
/// ships to the one the fuzz search happens to want.
const INVARIANT_VOCABULARY: &str = "\
use vbc_layout::invariants::Row;

impl Gutter {
    fn labelled(&self, rows: &[Row]) -> usize {
        rows.len()
    }
}
";

/// The modules of the layout engine the editor is composed from. A module nothing but its own
/// tests import is a module the application has drifted away from, however much the module itself
/// is worth on its own.
const COMPOSED: [&str; 2] = ["vbc_layout::buffer", "vbc_layout::viewport"];

/// The whole-text layouts the workspace still holds, and the sources holding them. The block
/// renderer held the one this list was written for: it laid every logical line above a window out
/// on the way down to it. It no longer does -- the lines above a window are counted, and a line
/// whose rows are its length is not even counted a row at a time -- so the entry is struck out and
/// the next such layout fails this rule again.
///
/// What the scan reads is a layout call written inside a repetition, so a call one function deep is
/// a call it does not see: `Block::counted_line` lays a line out where its rows cannot be read off
/// its length, and the walks that call it are repetitions over a whole text. `chat_cost.rs` is what
/// holds those walks to what they cost, by measuring them; following a call would be this scan's
/// own next step.
const STANDING_LAYOUTS: [(&str, &str); 0] = [];

/// The module the workspace keeps apart from its binaries on purpose. The vocabulary a layout is
/// checked in is the language the fuzz search reads a layout in, and the fourth rule here forbids
/// anything that ships from naming it, so no run may arrive at it and this rule expects none to.
const HELD_APART: [&str; 1] = ["vbc_layout::invariants"];

/// The modules no run of a binary arrives at yet. Each is work the application has drifted away
/// from rather than work that is wrong, and each is written down so that a module joining them
/// fails this rule and a module wired up to a keystroke fails it until its line is struck out.
const ORPHANED: [&str; 14] = [
    "vbc_editor::chat",
    "vbc_editor::chat::ansi",
    "vbc_editor::chat::block",
    "vbc_editor::chat::diff",
    "vbc_editor::chat::fold",
    "vbc_editor::chat::object",
    "vbc_editor::chat::policy",
    "vbc_editor::chat::selection",
    "vbc_editor::chat::transcript",
    "vbc_editor::chat::yank",
    "vbc_editor::clipboard",
    "vbc_editor::clipboard::helper",
    "vbc_editor::clipboard::protocol",
    "vbc_editor::clipboard::reader",
];

/// A scan of a tree, which reads every crate and every module of it and says what it found.
type Scan = fn(&Path) -> Result<Vec<Finding>, Error>;

/// Every scan the workspace is held to, each of which reads the tree it is given and is held to
/// having read it.
const SCANS: [Scan; 5] = [
    guard::scan_for_caches,
    guard::scan_for_whole_text_layouts,
    guard::scan_for_mappings_where_rows_are_drawn,
    guard::scan_for_invariant_vocabulary,
    guard::reach::unreachable,
];

/// A crate holding a library and no binary at all, which is a tree the reachability scan has
/// nothing to start a run from.
const LIBRARY_CRATE: &str = "\
[package]
name = \"vbc-alone\"
version = \"0.0.0\"
edition = \"2021\"
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
fn the_only_whole_text_layouts_the_workspace_holds_are_the_ones_recorded_against_it() {
    let findings =
        guard::scan_for_whole_text_layouts(&guard::workspace()).expect("the workspace is scanned");

    assert_eq!(
        STANDING_LAYOUTS
            .iter()
            .map(|(path, word)| ((*path).to_owned(), (*word).to_owned()))
            .collect::<Vec<(String, String)>>(),
        findings
            .iter()
            .map(|finding| (finding.path().to_owned(), finding.word().to_owned()))
            .collect::<Vec<(String, String)>>(),
        "a whole-text layout was added, or one was fixed without being struck off STANDING_LAYOUTS"
    );
}

#[test]
fn nothing_that_draws_names_the_mapping_that_would_place_its_rows() {
    let findings = guard::scan_for_mappings_where_rows_are_drawn(&guard::workspace())
        .expect("the workspace is scanned");

    assert_eq!(Vec::<Finding>::new(), findings);
}

#[test]
fn nothing_that_ships_reaches_for_the_vocabulary_a_layout_is_checked_in() {
    let findings = guard::scan_for_invariant_vocabulary(&guard::workspace())
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
fn the_scan_for_the_invariant_vocabulary_finds_the_one_the_fuzz_harness_reaches_for() {
    let harness =
        guard::read(&guard::workspace().join(FUZZ_HARNESS)).expect("the fuzz harness is readable");

    assert!(
        !guard::invariant_vocabulary(&harness).is_empty(),
        "{FUZZ_HARNESS} names no invariant vocabulary, so the scan looks for a shape nothing takes"
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
fn every_module_of_the_workspace_is_reachable_from_a_binary_or_recorded_as_not() {
    let findings =
        guard::reach::unreachable(&guard::workspace()).expect("the workspace is scanned");
    let mut recorded: Vec<&str> = HELD_APART.iter().chain(ORPHANED.iter()).copied().collect();
    recorded.sort_unstable();

    assert_eq!(
        recorded,
        words(&findings),
        "a module drifted out of the binary's reach, or one was wired up without being struck off \
         ORPHANED"
    );
}

#[test]
fn the_reachability_scan_starts_at_every_binary_the_workspace_holds() {
    let modules = guard::reach::modules(&guard::workspace()).expect("the workspace is scanned");

    assert_eq!(
        vec![
            "crates/vbc-ci/src/bin/pr-title-lint.rs",
            "crates/vbc-editor/src/main.rs",
            "crates/vbc-oracle/src/bin/differential-run.rs",
        ],
        modules
            .iter()
            .filter(|module| module.binary())
            .map(guard::reach::Module::path)
            .collect::<Vec<&str>>()
    );
}

#[test]
fn the_module_the_workspace_keeps_apart_from_its_binaries_is_still_apart_from_them() {
    let findings =
        guard::reach::unreachable(&guard::workspace()).expect("the workspace is scanned");

    for module in HELD_APART {
        assert!(
            words(&findings).contains(&module),
            "`{module}` is reachable from a binary, so the vocabulary a layout is checked in has \
             been wired into something that ships"
        );
    }
}

#[test]
fn every_layout_module_the_editor_is_built_on_is_reachable_from_a_binary() {
    let findings =
        guard::reach::unreachable(&guard::workspace()).expect("the workspace is scanned");

    for module in COMPOSED {
        assert!(
            !words(&findings).contains(&module),
            "no run arrives at `{module}`, so nothing the editor ships composes it"
        );
    }
}

#[test]
fn a_layout_module_the_editor_stopped_composing_is_caught() {
    let fixture = Fixture::of(&guard::workspace());
    for module in COMPOSED {
        for path in guard::importers(fixture.root(), module).expect("the fixture is scanned") {
            let source =
                guard::read(&fixture.root().join(&path)).expect("a source of the fixture is read");
            fixture.write(&path, &without(&source, module));
        }
    }
    let findings = guard::reach::unreachable(fixture.root()).expect("the fixture is scanned");

    for module in COMPOSED {
        assert!(
            words(&findings).contains(&module),
            "`{module}` was reported as reachable by a tree that no longer names it"
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
    fixture.append(RENDERER, WHOLE_TEXT_LAYOUT);
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(vec!["lay_out"], added(&findings, RENDERER));
}

#[test]
fn a_renderer_whose_loop_bound_was_hoisted_above_it_is_caught() {
    // The loop's own header names nothing of the text it walks, which is what a scan reading only
    // the header let through.
    let header = HOISTED_WHOLE_TEXT_LAYOUT
        .lines()
        .find(|line| line.trim_start().starts_with("while "))
        .expect("the offence holds a loop");
    for word in guard::shape::WHOLE_TEXT {
        assert!(
            !header.contains(word),
            "the loop's header names `{word}`, so it is not the dodge this is about"
        );
    }

    let fixture = Fixture::of(&guard::workspace());
    fixture.append(RENDERER, HOISTED_WHOLE_TEXT_LAYOUT);
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(vec!["lay_out"], added(&findings, RENDERER));
    assert_eq!(
        vec![line_of(&fixture, RENDERER, "line::lay_out")],
        findings
            .iter()
            .filter(|finding| RENDERER == finding.path())
            .map(Finding::line)
            .collect::<Vec<usize>>()
    );
}

#[test]
fn a_renderer_whose_loop_bound_was_passed_along_a_second_local_is_caught() {
    // Neither the loop's header nor the statement binding what it walks to names anything of the
    // text, which is what a scan following the extent one hop let through.
    for line in ALIASED_WHOLE_TEXT_LAYOUT
        .lines()
        .filter(|line| line.trim_start().starts_with("while ") || line.contains("let limit"))
    {
        for word in guard::shape::WHOLE_TEXT {
            assert!(
                !line.contains(word),
                "`{}` names `{word}`, so it is not the dodge this is about",
                line.trim()
            );
        }
    }

    let fixture = Fixture::of(&guard::workspace());
    fixture.append(RENDERER, ALIASED_WHOLE_TEXT_LAYOUT);
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(vec!["lay_out"], added(&findings, RENDERER));
}

#[test]
fn a_hoisted_bound_the_tests_walk_is_not_an_offence() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append(
        RENDERER,
        &format!("#[cfg(test)]\nmod tests {{\n{HOISTED_WHOLE_TEXT_LAYOUT}}}\n"),
    );
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(Vec::<&str>::new(), added(&findings, RENDERER));
}

#[test]
fn a_bounded_row_walk_is_not_an_offence() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append(RENDERER, BOUNDED_ROW_WALK);
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(
        Vec::<&str>::new(),
        added(&findings, RENDERER),
        "a walk bounded by the window it was asked for was read as a walk over a whole text, so \
         this rule no longer tells the layout the anchor exists for from the one it exists to \
         avoid"
    );
}

#[test]
fn a_module_nothing_imports_is_caught() {
    let fixture = orphaned(None);
    let findings = guard::reach::unreachable(fixture.root()).expect("the fixture is scanned");

    assert!(
        words(&findings).contains(&"vbc_editor::orphan"),
        "a module of a shipping crate that nothing at all names went uncaught"
    );
    assert_eq!(
        vec!["crates/vbc-editor/src/orphan.rs"],
        findings
            .iter()
            .filter(|finding| "vbc_editor::orphan" == finding.word())
            .map(Finding::path)
            .collect::<Vec<&str>>()
    );
}

#[test]
fn a_module_only_its_own_tests_import_is_caught() {
    let fixture = orphaned(Some(TESTS_REACH_THE_ORPHAN));
    let findings = guard::reach::unreachable(fixture.root()).expect("the fixture is scanned");

    assert!(
        words(&findings).contains(&"vbc_editor::orphan"),
        "a module imported only where a crate's tests are compiled was read as one a run reaches"
    );
}

#[test]
fn a_module_only_an_import_written_for_the_tests_reaches_is_caught() {
    let fixture = orphaned(Some(A_TEST_IMPORT_REACHES_THE_ORPHAN));
    let findings = guard::reach::unreachable(fixture.root()).expect("the fixture is scanned");

    assert!(
        words(&findings).contains(&"vbc_editor::orphan"),
        "a module imported under an attribute that compiles the import for the tests alone was \
         read as one a run reaches"
    );
}

#[test]
fn an_import_written_below_one_the_tests_alone_are_given_still_reaches() {
    let fixture = orphaned(Some(A_TEST_IMPORT_REACHES_THE_ORPHAN));
    fixture.append(RENDERER, REACHES_THE_ORPHAN);
    let findings = guard::reach::unreachable(fixture.root()).expect("the fixture is scanned");

    assert!(
        !words(&findings).contains(&"vbc_editor::orphan"),
        "an attribute standing above one import was read as standing above the ones below it too"
    );
}

#[test]
fn a_module_a_binary_reaches_through_another_is_not_caught() {
    let fixture = orphaned(Some(REACHES_THE_ORPHAN));
    let findings = guard::reach::unreachable(fixture.root()).expect("the fixture is scanned");

    assert!(
        !words(&findings).contains(&"vbc_editor::orphan"),
        "a module the binary reaches through the renderer it draws with was reported unreachable"
    );
}

#[test]
fn a_module_a_crate_root_hands_out_under_another_name_is_not_caught() {
    let fixture = orphaned(None);
    fixture.append("crates/vbc-editor/src/lib.rs", REEXPORTS_THE_ORPHAN);
    let findings = guard::reach::unreachable(fixture.root()).expect("the fixture is scanned");

    assert!(
        !words(&findings).contains(&"vbc_editor::orphan"),
        "a module its crate root hands out, in a crate the binary names, was reported unreachable"
    );
}

#[test]
fn a_reachability_scan_with_no_binary_to_start_from_fails() {
    let library = Fixture::empty();
    library.write("crates/vbc-alone/Cargo.toml", LIBRARY_CRATE);
    library.write("crates/vbc-alone/src/lib.rs", "pub mod named;\n");
    library.write("crates/vbc-alone/src/named.rs", ORPHAN);

    assert_eq!(
        Err(Error::NoBinaries {
            root: library.root().display().to_string()
        }),
        guard::reach::unreachable(library.root())
    );
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
fn a_gutter_that_reaches_for_the_invariant_vocabulary_is_caught() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append("crates/vbc-editor/src/gutter.rs", INVARIANT_VOCABULARY);
    let findings =
        guard::scan_for_invariant_vocabulary(fixture.root()).expect("the fixture is scanned");

    assert_eq!(
        vec!["crates/vbc-editor/src/gutter.rs"],
        paths(&findings),
        "a source that ships reaching for the invariant vocabulary went uncaught"
    );
    assert_eq!(vec!["invariant"], words(&findings));
    assert_eq!(
        vec![line_of(
            &fixture,
            "crates/vbc-editor/src/gutter.rs",
            "vbc_layout::invariants"
        )],
        findings.iter().map(Finding::line).collect::<Vec<usize>>()
    );
}

#[test]
fn a_layout_the_tests_reach_for_is_not_an_offence() {
    let fixture = Fixture::of(&guard::workspace());
    fixture.append(
        RENDERER,
        &format!("#[cfg(test)]\nmod tests {{\n{WHOLE_TEXT_LAYOUT}}}\n"),
    );
    let findings =
        guard::scan_for_whole_text_layouts(fixture.root()).expect("the fixture is scanned");

    assert_eq!(Vec::<&str>::new(), added(&findings, RENDERER));
}

#[test]
fn a_scan_that_would_read_nothing_fails() {
    let nowhere = Fixture::empty();
    let bare = Fixture::empty();
    bare.write("crates/vbc-bare/src/README.md", "no source at all\n");

    for scan in SCANS {
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
    for scan in SCANS {
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
fn a_copy_of_the_workspace_reads_the_same_as_the_workspace() {
    let root = guard::workspace();
    let fixture = Fixture::of(&root);

    for scan in SCANS {
        assert_eq!(
            scan(&root).expect("the workspace is scanned"),
            scan(fixture.root()).expect("the fixture is scanned"),
            "a copy of the workspace reads differently from the workspace, so every offence \
             written into one proves nothing about the other"
        );
    }
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
/// A copy of the workspace holding a module of the editor that nothing declares but its crate's
/// root, reached for by `import` written into the renderer the binary draws with, or by nothing at
/// all where there is none.
fn orphaned(import: Option<&str>) -> Fixture {
    let fixture = Fixture::of(&guard::workspace());
    fixture.write("crates/vbc-editor/src/orphan.rs", ORPHAN);
    fixture.append("crates/vbc-editor/src/lib.rs", "pub mod orphan;\n");
    if let Some(import) = import {
        fixture.append("crates/vbc-editor/src/render.rs", import);
    }

    fixture
}

/// # Returns
///
/// `source` with every line naming `module` left out.
fn without(source: &str, module: &str) -> String {
    source
        .lines()
        .filter(|line| !line.contains(module))
        .map(|line| format!("{line}\n"))
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
/// The word of every finding made in one of a tree's sources, which is what an offence written
/// into that source added to whatever the tree already held.
fn added<'findings>(findings: &'findings [Finding], path: &str) -> Vec<&'findings str> {
    findings
        .iter()
        .filter(|finding| path == finding.path())
        .map(Finding::word)
        .collect()
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
