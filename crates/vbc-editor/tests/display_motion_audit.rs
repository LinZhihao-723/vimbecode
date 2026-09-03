//! The audit of which of vim's motions display geometry decides, and what this editor does about
//! each of them.
//!
//! Every motion modalkit can hand the engine falls into one of three buckets, and the whole of the
//! classification is the table `audited` holds:
//!
//! * **Intercepted.** `g^`, `gj`, `gk`, `g0`, `g$`, `H`, `M` and `L` land at a place counted in the
//!   cells a terminal draws, and the shim measures them so that the layout engine can answer them.
//!   Until it does they are still answered by modalkit, so they are as wrong today as they were
//!   before the seam existed; what puts them here rather than in the next bucket is that they are
//!   measured and on their way to an answer.
//! * **Out of scope.** `gm`, `gM` and `|` land at a place counted in cells too, and nothing here
//!   measures them. modalkit answers all three by counting characters, which is a different place
//!   on any line that is not drawn one cell to the character, so the engine refuses them rather
//!   than moving the cursor somewhere vim would not. That refusal is the decision this file exists
//!   to record: `gm` and `|` are out of scope for v1, and bringing either of them in means giving
//!   the shim a way to answer a motion rather than only to measure one.
//! * **Characterwise.** Everything else names a position in a text rather than a place on a
//!   screen, so its answer is the same however wide a grapheme is drawn and wherever a line
//!   breaks. Nothing here claims modalkit's answer for such a motion matches vim's in every other
//!   respect; the claim is only that no layout has a say in it.
//!
//! Three things hold that classification up. The refusals are checked at the keys they are typed
//! by, with and without a shim installed and under an operator, and are required to leave the
//! cursor where it stood rather than to move it somewhere plausible. The reason for the refusals
//! is measured rather than asserted from a manual: each refused motion is typed at a real vim over
//! three lines of the same number of characters drawn in different numbers of cells, and vim is
//! required to land on a different character in them, while a control group of characterwise
//! motions over the same three lines is required to land on the same one. And the table itself is
//! held to the enum it classifies, which is read out of the source of the crate that declares it,
//! so a motion added to a later release of that crate fails this file rather than arriving
//! silently as a motion nobody looked at.
//!
//! One divergence the audit found is not a motion at all and is pinned here so that it is not
//! rediscovered. A vertical motion carries a column from the line it leaves to the line it lands
//! on: vim carries the screen column and modalkit carries the character index, so `j` off a line
//! of CJK lands in a different place in the two engines. That is a seam of its own rather than a
//! motion to classify -- `j` names a line, and a line is a position in a text -- and the test that
//! pins it is expected to go red on the day someone builds that seam.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use editor_types::prelude::{MoveDir1D, MovePosition, MoveType, WordStyle};
use vbc_editor::engine::{typed, Engine, Error};
use vbc_editor::shim::{classify, Classification, ScreenMotion};
use vbc_oracle::corpus::{Case, Options};
use vbc_oracle::state::EditorState;
use vbc_oracle::vim::VimDriver;

/// The package that declares the motions the audit classifies.
const UPSTREAM: &str = "editor-types";

/// The declaration in that package that names every motion it can hand the engine.
const DECLARATION: &str = "pub enum MoveType {";

/// The motions the audit refuses, named by the keys they are typed by.
const REFUSED: [&str; 4] = ["gm", "gM", "3gM", "12|"];

/// The same motions typed with counts small enough to land inside the lines they are measured
/// over, which is what makes the places vim lands them comparable.
const REFUSED_AT_A_MEASURABLE_COUNT: [&str; 3] = ["gm", "gM", "5|"];

/// Motions whose answer is the same character however wide the characters before it are drawn,
/// which is what says the lines below tell a character count from a cell count.
const CHARACTERWISE: [&str; 3] = ["5l", "$", "0"];

/// Three lines of the same number of characters, drawn in three different numbers of cells: one
/// cell to the character, two cells to the character, and a tab stop to every other character.
const SAME_CHARACTER_COUNT: [(&str, &str); 3] = [
    ("ascii", "abcdefghijklmn\n"),
    ("cjk", "你好世界一二三四五六七八九十\n"),
    ("tabbed", "\ta\tb\tc\td\te\tf\tg\n"),
];

/// The characters each of those lines holds, which is what makes them comparable.
const CHARACTERS_PER_LINE: usize = 14;

/// The window the motions above are typed in, narrow enough that half of it is a place inside
/// every one of those lines.
const COLUMNS: u16 = 12;

/// The screen lines that window is tall.
const ROWS: u16 = 5;

/// A buffer whose first line is drawn two cells to the character and whose second is drawn one,
/// which is what separates the column a vertical motion carries from the character it carries.
const DESCENDING: &str = "你好世界一二\nabcdefghijkl\n";

/// The keys that walk two characters along the first of those lines and then step down.
const WALKED_THEN_DOWN: &str = "llj";

/// The character `j` lands on in [`DESCENDING`] in vim, which carries the screen column the cursor
/// was drawn in.
const VIM_CARRIES_THE_SCREEN_COLUMN: usize = 4;

/// The character `j` lands on in [`DESCENDING`] in this engine, which carries modalkit's character
/// index.
const THE_ENGINE_CARRIES_THE_CHARACTER: usize = 2;

/// The text the refusals are typed at, whose second line is there so that a motion that ran would
/// have somewhere to go.
const PROSE: &str = "the quick brown fox\njumps over it\n";

#[test]
fn every_motion_the_audit_names_is_classified_the_way_it_is_recorded() {
    for (move_type, classification) in audited() {
        assert_eq!(
            classification,
            classify(&move_type),
            "`{move_type:?}` is classified as something other than the audit records"
        );
    }
}

#[test]
fn the_intercepted_set_is_the_screen_motions_the_shim_measures_and_nothing_else() {
    assert_eq!(
        BTreeSet::from([
            "ScreenFirstWord(Next)".to_owned(),
            "ScreenLine(Next)".to_owned(),
            "ScreenLinePos(Beginning)".to_owned(),
            "ScreenLinePos(End)".to_owned(),
            "ViewportPos(Middle)".to_owned(),
        ]),
        named(|classification| matches!(classification, Classification::Intercepted(_)))
    );
    assert_eq!(
        BTreeSet::from([
            "LineColumnOffset".to_owned(),
            "LinePercent".to_owned(),
            "LinePos(Middle)".to_owned(),
            "ScreenLinePos(Middle)".to_owned(),
        ]),
        named(|classification| matches!(classification, Classification::OutOfScope { .. }))
    );
}

#[test]
fn every_motion_the_crate_that_declares_them_holds_is_classified() -> anyhow::Result<()> {
    let unclassified = unclassified(&declared(&upstream_source()?));

    assert_eq!(
        Vec::<String>::new(),
        unclassified,
        "`{UPSTREAM}` declares motions this audit does not classify; classify each of them in \
         `vbc_editor::shim::classify` and record it in the table this file holds"
    );

    Ok(())
}

#[test]
fn the_reading_of_that_declaration_catches_a_motion_added_to_it() {
    let source = format!(
        "{DECLARATION}\n    /// Move to a screen column.\n    ScreenColumn(MoveDir1D),\n\n    \
         /// Move to a line.\n    Line(MoveDir1D),\n}}\n"
    );
    let declared = declared(&source);

    assert_eq!(
        BTreeSet::from(["Line".to_owned(), "ScreenColumn".to_owned()]),
        declared
    );
    assert_eq!(vec!["ScreenColumn".to_owned()], unclassified(&declared));
}

#[test]
fn a_motion_the_audit_puts_out_of_scope_is_refused_at_the_keys_it_is_typed_by() {
    for keys in REFUSED {
        let mut engine = Engine::new(PROSE);
        let refused = engine
            .press_all(keys.chars().map(typed))
            .expect_err("a motion the audit puts out of scope is refused");
        let Error::OutOfScope { keys: named } = &refused else {
            panic!("`{keys}` was refused as `{refused:?}` rather than as an out-of-scope motion");
        };

        assert!(
            keys.ends_with(named.as_str()),
            "`{keys}` was refused under the name `{named}`"
        );
        assert!(
            refused.to_string().contains("refused"),
            "`{keys}` was refused without saying so: {refused}"
        );
    }
}

#[test]
fn a_refused_motion_leaves_the_cursor_where_it_stood_rather_than_somewhere_plausible() {
    for keys in REFUSED {
        let mut engine = Engine::new(PROSE);
        engine
            .press_all("ll".chars().map(typed))
            .expect("`ll` runs");
        let before = engine.cursor();
        let _refused = engine.press_all(keys.chars().map(typed));

        assert_eq!(
            before,
            engine.cursor(),
            "`{keys}` was refused and moved the cursor anyway"
        );
        assert_eq!(
            Vec::<ScreenMotion>::new(),
            engine
                .shim()
                .expect("an engine built by `new` holds a shim")
                .intercepted()
                .iter()
                .map(|taken| taken.motion)
                .collect::<Vec<ScreenMotion>>(),
            "`{keys}` was refused and measured anyway"
        );
    }
}

#[test]
fn a_refused_motion_is_refused_under_an_operator_and_without_a_shim() {
    for keys in REFUSED {
        let operated: String = format!("d{keys}");
        for (built, mut engine) in [
            ("with a shim", Engine::new(PROSE)),
            ("without a shim", Engine::bypassing_the_shim(PROSE)),
        ] {
            let refused = engine
                .press_all(operated.chars().map(typed))
                .expect_err("an operator over a refused motion is refused");

            assert!(
                matches!(refused, Error::OutOfScope { .. }),
                "`{operated}` was refused as `{refused:?}` by an engine built {built}"
            );
            assert_eq!(PROSE, engine.text(), "`{operated}` edited the text anyway");
        }
    }
}

#[test]
fn vim_answers_a_refused_motion_from_the_cells_a_line_is_drawn_in() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in REFUSED_AT_A_MEASURABLE_COUNT {
        assert!(
            landed(&vim, keys)?.len() > 1,
            "vim answered `{keys}` at the same character in every line, so nothing about it is \
             counted in cells and it does not belong out of scope"
        );
    }

    Ok(())
}

#[test]
fn vim_answers_a_characterwise_motion_at_the_same_character_in_all_three_lines(
) -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in CHARACTERWISE {
        assert_eq!(
            1,
            landed(&vim, keys)?.len(),
            "vim answered `{keys}` at a different character in lines of the same length, so the \
             lines above do not tell a character count from a cell count"
        );
    }

    Ok(())
}

#[test]
fn the_column_a_vertical_motion_carries_is_a_known_divergence() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let mut engine = Engine::new(DESCENDING);
    engine
        .press_all(WALKED_THEN_DOWN.chars().map(typed))
        .expect("the keys run");

    let landed = engine.cursor();
    let line = DESCENDING.lines().nth(landed.line).unwrap_or_default();

    assert_eq!(
        VIM_CARRIES_THE_SCREEN_COLUMN,
        character_of(&vim.run(DESCENDING, WALKED_THEN_DOWN)?, DESCENDING),
        "vim no longer carries the screen column across a vertical motion; the audit says it does"
    );
    assert_eq!(
        THE_ENGINE_CARRIES_THE_CHARACTER,
        line[..landed.column].chars().count(),
        "the engine no longer carries modalkit's character index across a vertical motion, so the \
         divergence the audit pins has been closed and the audit should say so"
    );

    Ok(())
}

/// # Returns
///
/// Every motion the audit classifies, paired with what it classifies it as. A motion whose
/// classification turns on the position it names is listed at each of those positions.
fn audited() -> Vec<(MoveType, Classification)> {
    vec![
        (MoveType::BufferByteOffset, Classification::Characterwise),
        (MoveType::BufferLineOffset, Classification::Characterwise),
        (MoveType::BufferLinePercent, Classification::Characterwise),
        (
            MoveType::BufferPos(MovePosition::End),
            Classification::Characterwise,
        ),
        (
            MoveType::Column(MoveDir1D::Next, false),
            Classification::Characterwise,
        ),
        (
            MoveType::FinalNonBlank(MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (
            MoveType::FirstWord(MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (MoveType::ItemMatch, Classification::Characterwise),
        (
            MoveType::Line(MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (
            MoveType::LineColumnOffset,
            Classification::OutOfScope { keys: "|" },
        ),
        (
            MoveType::LinePercent,
            Classification::OutOfScope { keys: "gM" },
        ),
        (
            MoveType::LinePos(MovePosition::Beginning),
            Classification::Characterwise,
        ),
        (
            MoveType::LinePos(MovePosition::Middle),
            Classification::OutOfScope { keys: "gM" },
        ),
        (
            MoveType::LinePos(MovePosition::End),
            Classification::Characterwise,
        ),
        (
            MoveType::ParagraphBegin(MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (
            MoveType::ScreenFirstWord(MoveDir1D::Next),
            Classification::Intercepted(ScreenMotion::FirstWord(MoveDir1D::Next)),
        ),
        (
            MoveType::ScreenLine(MoveDir1D::Next),
            Classification::Intercepted(ScreenMotion::Line(MoveDir1D::Next)),
        ),
        (
            MoveType::ScreenLinePos(MovePosition::Beginning),
            Classification::Intercepted(ScreenMotion::LinePos(MovePosition::Beginning)),
        ),
        (
            MoveType::ScreenLinePos(MovePosition::Middle),
            Classification::OutOfScope { keys: "gm" },
        ),
        (
            MoveType::ScreenLinePos(MovePosition::End),
            Classification::Intercepted(ScreenMotion::LinePos(MovePosition::End)),
        ),
        (
            MoveType::SectionBegin(MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (
            MoveType::SectionEnd(MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (
            MoveType::SentenceBegin(MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (
            MoveType::ViewportPos(MovePosition::Middle),
            Classification::Intercepted(ScreenMotion::ViewportPos(MovePosition::Middle)),
        ),
        (
            MoveType::WordBegin(WordStyle::Little, MoveDir1D::Next),
            Classification::Characterwise,
        ),
        (
            MoveType::WordEnd(WordStyle::Little, MoveDir1D::Next),
            Classification::Characterwise,
        ),
    ]
}

/// # Returns
///
/// The motions the audit classifies as `wanted` says, named the way the crate that declares them
/// spells them.
fn named<PredicateType: Fn(&Classification) -> bool>(wanted: PredicateType) -> BTreeSet<String> {
    audited()
        .into_iter()
        .filter(|(_move_type, classification)| wanted(classification))
        .map(|(move_type, _classification)| format!("{move_type:?}"))
        .collect()
}

/// # Returns
///
/// The names among `declared` that the audit classifies none of.
fn unclassified(declared: &BTreeSet<String>) -> Vec<String> {
    let audited: BTreeSet<String> = audited()
        .into_iter()
        .map(|(move_type, _classification)| variant(&move_type))
        .collect();

    declared.difference(&audited).cloned().collect()
}

/// # Returns
///
/// The name of the variant `move_type` is, without the positions or directions it carries.
fn variant(move_type: &MoveType) -> String {
    let named = format!("{move_type:?}");

    named
        .split_once('(')
        .map_or(named.as_str(), |(name, _rest)| name)
        .to_owned()
}

/// # Returns
///
/// Every variant [`DECLARATION`] declares in `source`.
fn declared(source: &str) -> BTreeSet<String> {
    let Some((_before, body)) = source.split_once(DECLARATION) else {
        return BTreeSet::new();
    };

    let mut declared = BTreeSet::new();
    for line in body.lines() {
        if line == "}" {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('#') {
            continue;
        }
        let name: String = trimmed
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if !name.is_empty() {
            declared.insert(name);
        }
    }

    declared
}

/// # Returns
///
/// The source of the file that holds [`DECLARATION`] in the copy of [`UPSTREAM`] the workspace
/// links, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if `cargo metadata` could not be run, if it reported a failure, if it does
///   not report [`UPSTREAM`], or if no file of that package holds [`DECLARATION`].
/// * Forwards [`serde_json::from_slice`]'s return values on failure.
fn upstream_source() -> anyhow::Result<String> {
    let reported = Command::new(env!("CARGO"))
        .current_dir(workspace())
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()?;
    anyhow::ensure!(
        reported.status.success(),
        "`cargo metadata` failed: {}",
        String::from_utf8_lossy(&reported.stderr)
    );

    let metadata: serde_json::Value = serde_json::from_slice(&reported.stdout)?;
    let manifest = metadata["packages"]
        .as_array()
        .context("`cargo metadata` reports the packages the workspace links")?
        .iter()
        .find(|package| package["name"] == UPSTREAM)
        .and_then(|package| package["manifest_path"].as_str())
        .with_context(|| format!("the workspace links `{UPSTREAM}`"))?;
    let root = Path::new(manifest)
        .parent()
        .context("a manifest sits in the package it describes")?
        .to_owned();

    holding(&root.join("src"))?
        .with_context(|| format!("`{UPSTREAM}` declares `{DECLARATION}` in one of its files"))
}

/// # Returns
///
/// The source of the first file under `directory` that holds [`DECLARATION`], and `None` where no
/// file under it does, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`fs::read_dir`]'s return values on failure.
/// * Forwards [`fs::read_to_string`]'s return values on failure.
fn holding(directory: &Path) -> anyhow::Result<Option<String>> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let found = if path.is_dir() {
            holding(&path)?
        } else {
            fs::read_to_string(&path)
                .ok()
                .filter(|source| source.contains(DECLARATION))
        };
        if found.is_some() {
            return Ok(found);
        }
    }

    Ok(None)
}

/// # Returns
///
/// The characters `keys` lands vim on in each of [`SAME_CHARACTER_COUNT`], without repetition, on
/// success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn landed(vim: &VimDriver, keys: &str) -> anyhow::Result<BTreeSet<usize>> {
    let mut landed = BTreeSet::new();
    for (name, buffer) in SAME_CHARACTER_COUNT {
        assert_eq!(
            CHARACTERS_PER_LINE,
            buffer.trim_end().chars().count(),
            "the `{name}` line is not the length the lines beside it are"
        );
        landed.insert(character_of(&vim.run_case(&case(buffer, keys))?, buffer));
    }

    Ok(landed)
}

/// # Returns
///
/// The character of `buffer` the cursor of `state` rests on, counted from the start of its line.
///
/// # Panics
///
/// Panics if the state's cursor is not on a line `buffer` holds.
fn character_of(state: &EditorState, buffer: &str) -> usize {
    let line = buffer
        .lines()
        .nth(usize::try_from(state.cursor.line).expect("a line index fits a machine word"))
        .expect("the cursor rests on a line the buffer holds");
    let column = usize::try_from(state.cursor.column).expect("a byte offset fits a machine word");

    line[..column].chars().count()
}

/// # Returns
///
/// A case that types `keys` at `buffer` in the window the audit's measurements are made in.
fn case(buffer: &str, keys: &str) -> Case {
    Case {
        id: format!("audit-{keys}"),
        description: "A motion the display-motion audit measures.".to_owned(),
        buffer: buffer.to_owned(),
        keys: keys.to_owned(),
        viewport_width: COLUMNS,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: Options::default(),
    }
}

/// # Returns
///
/// The root of the workspace this crate is a member of.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below its workspace root")
        .to_owned()
}
