//! The table the editor's keys are bound by, held to the vim it was adopted from and to itself.
//!
//! The table exists because modalkit's own has a defect no caller can work around: `gU` and `gu`
//! are the first two keys of the longer normal-mode sequences `gUgU` and `gugu`, so a `g` typed
//! after `gU` walks further down that sequence instead of reaching the operator-pending table,
//! and `gUgj` ends as a cursor move with the operator dropped. The first thing checked here is
//! that it no longer does, against a real vim rather than against a rule someone wrote down.
//!
//! That check is worth what it would catch, so each case is required to be one whose answer a
//! table that dropped the operator could not produce: vim is required to answer the operator over
//! the display motion differently from the display motion alone, and differently from the
//! operator over the logical motion spelled the same way. An engine that answered `j` where `gj`
//! was typed, or that moved the cursor and changed no text, fails all three.
//!
//! The rest holds the table to itself, because a table is a thing that rots in ways a behavioural
//! test never sees. Every entry is typed and required to produce an action, and none of them is
//! allowed to be a no-op: an entry nothing can reach, or one bound to nothing, is exactly the
//! shape `gUgj` broke in. No entry is a prefix of another within the table it is looked up in,
//! which is the property that lets the machine fire a match the moment it completes rather than
//! waiting to see whether a longer one follows -- and waiting is what modalkit does and where its
//! defect lives. Every action every entry produces is one the engine drives rather than one it
//! reports as unsupported, and the entries whose motions the engine refuses are asserted as an
//! exact list rather than tolerated. And what the table produces is compared, action by action and
//! context by context, against modalkit's own table over the sequences modalkit is right about, so
//! that replacing the table changed the one thing it was replaced for and nothing else.
//!
//! Three of those checks are shown to bite rather than asserted to: a table with an entry that
//! produces nothing, a table with an entry buried under a longer one, and a table that answers a
//! key differently from modalkit are all built here and required to fail the checks that exist to
//! catch them.
//!
//! Rebinding is checked at both the things the table is configurable in: the prefix the display
//! motions and the case operators hang off, and any single binding. In each case the rebound keys
//! are required to do the work and the keys they replaced are required to have stopped doing it.
//!
//! The two reasons the table gives for the shape it has are measured here rather than taken on
//! trust. An operator typed twice is what runs it over whole lines, so the doubled sequences are
//! held to vim, `g~g~` among them, which modalkit's own table drops for the bare `~` the way it
//! drops `gUgj` for `j`. And the targets the table leaves unbound are required to be ones its text
//! still answers with nothing, and the word object to still name one range whichever way it is
//! asked for: each is bound to a key and typed, vim is required to answer it with something, and
//! the editor is required to answer it with nothing. Were modalkit to gain either, the table would
//! be the poorer for the omission, and these say so.

mod outcome;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use editor_types::context::EditContext;
use editor_types::prelude::{
    Count, EditTarget, MoveDir1D, MoveType, RangeType, Specifier, WordStyle,
};
use modalkit::actions::{Action, EditAction, EditorAction};
use modalkit::env::vim::keybindings::{default_vim_keys, VimMachine};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use modalkit::keybindings::BindingMachine;
use vbc_editor::engine::{typed, Engine};
use vbc_editor::keys::{Bindings, Edge, Emit, Keys, Step};
use vbc_editor::screen::Geometry;
use vbc_editor::shim::{classified, Classification};
use vbc_oracle::corpus::{Case as CorpusCase, Options as CaseOptions};
use vbc_oracle::vim::VimDriver;

use crate::outcome::Outcome;

/// One cross-check of an operator applied over a display motion: the operator, the motion, and the
/// logical motion spelled the same way, which the case is required to answer differently from.
struct Cased {
    operator: &'static str,
    motion: &'static str,
    logical: &'static str,
}

/// A line long enough to wrap into two rows of the window below, whose letters are of both cases
/// so that every case operator can be seen to have run, and whose later lines are short.
const PROSE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789\nsecond LINE here\nthird\n";

/// The cells the cases below are laid out in, narrow enough that the first line wraps.
const COLUMNS: u16 = 20;

/// The screen lines the cases below are laid out in.
const ROWS: u16 = 10;

/// The sequences that double an operator, which is what runs it over whole lines: an operator's own
/// keys typed again, and its last key alone.
const DOUBLED: [&str; 8] = ["dd", "yy", "gUU", "gUgU", "guu", "gugu", "g~~", "g~g~"];

/// The prose the unbound targets are named at, which holds two sentences to a paragraph, three
/// paragraphs, and a tag around the middle one.
const SENTENCES: &str = "Alpha one. Alpha two.\n\n<a>Beta one. Beta two.</a>\n\nGamma one.\n";

/// The keys that put the cursor inside the tag, the sentence and the paragraph in the middle of
/// [`SENTENCES`], so that a target naming any of them names something to travel over.
const PLACED: &str = "2jfB";

/// The operators whose keys begin with the character the display motions also begin with, which is
/// the pairing modalkit's own table cannot spell.
const CASED: [Cased; 6] = [
    Cased {
        operator: "gU",
        motion: "gj",
        logical: "j",
    },
    Cased {
        operator: "gu",
        motion: "gj",
        logical: "j",
    },
    Cased {
        operator: "g~",
        motion: "gj",
        logical: "j",
    },
    Cased {
        operator: "gU",
        motion: "g$",
        logical: "$",
    },
    Cased {
        operator: "gU",
        motion: "2gj",
        logical: "2j",
    },
    Cased {
        operator: "g~",
        motion: "g$",
        logical: "$",
    },
];

/// The sequences the table is compared against modalkit's own over, which are the ones modalkit
/// answers the way this editor wants them answered.
const SHARED: [&str; 98] = [
    "w", "W", "b", "B", "e", "E", "h", "l", "j", "k", "0", "^", "$", "_", "-", "+", "|", "5|", "G",
    "3G", "gg", "2gg", "%", "H", "M", "L", "gj", "gk", "g0", "g$", "g^", "g_", "gm", "gM", "3gM",
    "ge", "gE", "go", "fx", "Fx", "tx", "Tx", ";", ",", "x", "X", "D", "Y", "J", "gJ", "p", "P",
    "u", "~", "rz", "dd", "yy", "cc", ">>", "<<", "==", "dw", "d$", "d3w", "2d3w", "cw", "cW",
    "ciw", "ci(", "daw", "yw", ">j", "gUU", "guu", "g~~", "gUw", "guw", "g~w", "\"add", "\"Ayy",
    "3\"add", "s", "S", "C", "vjd", "vjy", "vjc", "Vjd", "VjD", "VjY", "vjJ", "vj>", "vjo", "v$hd",
    "vv", "i", "a", "o",
];

/// The change a repeat is reached behind, which is a change the table binds itself.
const CHANGED: &str = "x";

/// The keys that reach each mode the table binds in, from a machine in normal mode.
const REACHED: [(VimMode, &str); 4] = [
    (VimMode::Normal, ""),
    (VimMode::Visual, "v"),
    (VimMode::Insert, "i"),
    (VimMode::OperationPending, "d"),
];

/// The motions the table binds that the engine refuses because they land where display geometry
/// says and nothing measures them, named the way a refusal names them.
const REFUSED: [&str; 3] = ["gM", "gm", "|"];

#[test]
fn an_operator_over_a_display_motion_beginning_with_the_prefix_ends_where_vim_ends(
) -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &CASED {
        let keys = format!("{}{}", case.operator, case.motion);
        let expected = vim_outcome(&vim, &keys)?;

        assert_eq!(
            expected,
            outcome_of(Engine::laid_out_in(PROSE, window()), &keys),
            "`{keys}` left the engine somewhere other than where vim left it"
        );
        assert_ne!(
            PROSE, expected.text,
            "vim left `{keys}` with the text it started from, so a table that dropped the \
             operator would pass this case"
        );
    }

    Ok(())
}

#[test]
fn a_table_that_dropped_the_operator_or_the_display_motion_would_diverge_from_vim(
) -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &CASED {
        let keys = format!("{}{}", case.operator, case.motion);
        let expected = vim_outcome(&vim, &keys)?;

        assert_ne!(
            expected,
            vim_outcome(&vim, case.motion)?,
            "vim answers `{keys}` where it answers `{}`, so the case cannot tell an operator that \
             ran from one that was dropped",
            case.motion
        );
        assert_ne!(
            expected,
            vim_outcome(&vim, &format!("{}{}", case.operator, case.logical))?,
            "vim answers `{keys}` where it answers the same operator over `{}`, so the case \
             cannot tell a display motion from the logical motion spelled the same way",
            case.logical
        );
    }

    Ok(())
}

#[test]
fn every_binding_the_table_holds_produces_an_action() {
    assert_eq!(Vec::<String>::new(), silent(&Bindings::vim()));
}

#[test]
fn the_check_for_a_binding_that_produces_nothing_catches_one() {
    let mut bindings = Bindings::vim();
    bindings.bind(
        VimMode::Normal,
        "Q",
        Step::Run {
            changes: Vec::new(),
            emits: Vec::new(),
            mode: None,
        },
    );

    assert_eq!(vec!["Q".to_owned()], silent(&bindings));
}

#[test]
fn no_binding_the_table_holds_resolves_to_a_no_op() {
    let bindings = Bindings::vim();
    let mut silent = Vec::new();
    for keys in reachable(&bindings) {
        for (action, _context) in produced(&bindings, &keys) {
            if let Action::NoOp = action {
                silent.push(shown(&keys));
            }
        }
    }

    assert_eq!(Vec::<String>::new(), silent);
}

#[test]
fn no_binding_the_table_holds_is_buried_under_a_longer_one() {
    assert_eq!(Vec::<String>::new(), buried(&Bindings::vim()));
}

#[test]
fn the_check_for_a_buried_binding_catches_one() {
    let mut bindings = Bindings::vim();
    bindings.bind(
        VimMode::Normal,
        "dd",
        Step::Run {
            changes: Vec::new(),
            emits: vec![Emit::Always(Action::NoOp)],
            mode: None,
        },
    );

    assert_eq!(vec!["d < dd".to_owned()], buried(&bindings));
}

#[test]
fn every_action_the_table_produces_is_one_the_engine_drives() {
    let bindings = Bindings::vim();
    let mut refused = BTreeSet::new();
    let mut undriven = Vec::new();
    for keys in reachable(&bindings) {
        for (action, _context) in produced(&bindings, &keys) {
            let Action::Editor(editor) = action else {
                undriven.push(format!("{}: {action:?}", shown(&keys)));
                continue;
            };
            match classified(&editor) {
                Some((Classification::OutOfScope { keys: named }, _)) => {
                    refused.insert(named.to_owned());
                }
                Some((Classification::Unclassified, _)) => {
                    undriven.push(format!("{}: {editor:?}", shown(&keys)));
                }
                _ => {}
            }
        }
    }

    assert_eq!(
        Vec::<String>::new(),
        undriven,
        "the table produces actions the engine reports rather than runs"
    );
    assert_eq!(
        REFUSED
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>(),
        refused,
        "the motions the engine refuses are not the ones the table is named as binding"
    );
}

#[test]
fn the_table_produces_what_modalkit_produces_wherever_modalkit_is_right() {
    for keys in SHARED {
        assert_eq!(
            through_modalkit(keys),
            through(&Bindings::vim(), keys),
            "`{keys}` no longer produces what modalkit's own table produces for it"
        );
    }
}

#[test]
fn the_comparison_against_modalkit_reports_a_table_that_answers_a_key_differently() {
    let mut bindings = Bindings::vim();
    bindings.bind(
        VimMode::Normal,
        "x",
        Step::Run {
            changes: Vec::new(),
            emits: vec![Emit::Always(
                EditorAction::Edit(
                    Specifier::Exact(EditAction::Delete),
                    EditTarget::Range(RangeType::Line, true, Count::Contextual),
                )
                .into(),
            )],
            mode: None,
        },
    );

    assert_ne!(through_modalkit("x"), through(&bindings, "x"));
}

#[test]
fn the_prefix_the_display_motions_hang_off_can_be_rebound() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let expected = vim_outcome(&vim, "gUgj")?;
    let rebound = Bindings::prefixed('z');

    assert_eq!(
        expected,
        outcome_of(
            Engine::laid_out_in(PROSE, window()).bound_by(rebound.clone()),
            "zUzj"
        ),
        "`zUzj` against a table built around `z` does not do what `gUgj` does against vim"
    );
    assert_ne!(
        expected,
        outcome_of(
            Engine::laid_out_in(PROSE, window()).bound_by(rebound),
            "gUgj"
        ),
        "`gUgj` still runs against a table built around `z`"
    );

    Ok(())
}

#[test]
fn a_single_binding_can_be_rebound() {
    let mut bindings = Bindings::vim();
    let step = bindings
        .entries()
        .iter()
        .find(|binding| VimMode::Normal == binding.mode && [Edge::Key(key('x'))] == *binding.keys)
        .map(|binding| binding.step.clone())
        .expect("the table binds `x` in normal mode");
    bindings.bind(VimMode::Normal, "Q", step);
    bindings.unbind(VimMode::Normal, "x");

    let rebound = outcome_of(Engine::new(PROSE).bound_by(bindings.clone()), "Q");
    let dropped = outcome_of(Engine::new(PROSE).bound_by(bindings), "x");

    assert_eq!(
        outcome_of(Engine::new(PROSE), "x"),
        rebound,
        "`Q` does not do what `x` did once it is bound to what `x` was bound to"
    );
    assert_eq!(
        PROSE, dropped.text,
        "`x` still deletes a character once it is unbound"
    );
}

#[test]
fn an_operator_typed_twice_runs_over_whole_lines_where_vim_does() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let untouched = outcome_of(Engine::laid_out_in(PROSE, window()), "");

    for keys in DOUBLED {
        let expected = vim_outcome(&vim, keys)?;

        assert_eq!(
            expected,
            outcome_of(Engine::laid_out_in(PROSE, window()), keys),
            "`{keys}` left the engine somewhere other than where vim left it"
        );
        assert_ne!(
            untouched, expected,
            "vim left `{keys}` where it left an engine handed no keys, so the case cannot tell an \
             operator that ran over the line from one that was abandoned"
        );
    }

    Ok(())
}

#[test]
fn the_targets_the_table_leaves_unbound_are_ones_modalkit_answers_with_nothing(
) -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let standing = Outcome::from(vim.run(SENTENCES, PLACED)?);
    let unmoved = outcome_of(Engine::new(SENTENCES), PLACED);

    for (keys, target) in unbound() {
        let sequence = format!("{PLACED}d{keys}");
        let mut bindings = Bindings::vim();
        bindings.bind(VimMode::OperationPending, &keys, naming(target));

        assert_ne!(
            standing,
            Outcome::from(vim.run(SENTENCES, &sequence)?),
            "vim answers `d{keys}` with nothing, so the case cannot tell a target modalkit answers \
             from one it does not"
        );
        assert_eq!(
            unmoved,
            outcome_of(Engine::new(SENTENCES).bound_by(bindings), &sequence),
            "modalkit's text now answers `d{keys}`, which the table leaves unbound because it did \
             not"
        );
    }

    Ok(())
}

#[test]
fn the_word_object_names_one_range_whichever_way_modalkit_is_asked_for_it() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let sequence = format!("{PLACED}dQ");

    for style in [WordStyle::Little, WordStyle::Big] {
        let around = ranged(RangeType::Word(style.clone()), true);
        let inside = ranged(RangeType::Word(style), false);

        assert_eq!(
            outcome_of(Engine::new(SENTENCES).bound_by(around), &sequence),
            outcome_of(Engine::new(SENTENCES).bound_by(inside), &sequence),
            "modalkit's text now draws the distinction between `iw` and `aw` that the table names \
             one range apiece because it did not"
        );
    }
    assert_ne!(
        Outcome::from(vim.run(SENTENCES, &format!("{PLACED}daw"))?),
        Outcome::from(vim.run(SENTENCES, &format!("{PLACED}diw"))?),
        "vim answers `daw` where it answers `diw`, so the case cannot tell one range from two"
    );

    Ok(())
}

/// # Returns
///
/// The motions and the text objects the table leaves unbound, each spelled the way vim spells it
/// and paired with the target it would name were it bound.
fn unbound() -> Vec<(String, EditTarget)> {
    let mut unbound: Vec<(String, EditTarget)> = [
        (")", MoveType::SentenceBegin(MoveDir1D::Next)),
        ("(", MoveType::SentenceBegin(MoveDir1D::Previous)),
        ("}", MoveType::ParagraphBegin(MoveDir1D::Next)),
        ("{", MoveType::ParagraphBegin(MoveDir1D::Previous)),
    ]
    .into_iter()
    .map(|(keys, move_type)| {
        (
            keys.to_owned(),
            EditTarget::Motion(move_type, Count::Contextual),
        )
    })
    .collect();
    for (keys, range) in [
        ("s", RangeType::Sentence),
        ("p", RangeType::Paragraph),
        ("t", RangeType::XmlTag),
    ] {
        for (around, inclusive) in [("a", true), ("i", false)] {
            unbound.push((
                format!("{around}{keys}"),
                EditTarget::Range(range.clone(), inclusive, Count::Contextual),
            ));
        }
    }

    unbound
}

/// # Returns
///
/// A step running the operator the context holds over `target`.
fn naming(target: EditTarget) -> Step {
    Step::Run {
        changes: Vec::new(),
        emits: vec![Emit::Always(
            EditorAction::Edit(Specifier::Contextual, target).into(),
        )],
        mode: None,
    }
}

/// # Returns
///
/// The editor's own table with `Q` bound, in the operator-pending table, to the range `range`
/// names, taking the characters it stops on where `inclusive`.
fn ranged(range: RangeType, inclusive: bool) -> Bindings {
    let mut bindings = Bindings::vim();
    bindings.bind(
        VimMode::OperationPending,
        "Q",
        naming(EditTarget::Range(range, inclusive, Count::Contextual)),
    );

    bindings
}

/// # Returns
///
/// What vim was left holding after `keys` were typed at [`PROSE`] in the window the cases are laid
/// out in, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn vim_outcome(vim: &VimDriver, keys: &str) -> anyhow::Result<Outcome> {
    let state = vim.run_case(&CorpusCase {
        id: keys.to_owned(),
        description: keys.to_owned(),
        buffer: PROSE.to_owned(),
        keys: keys.to_owned(),
        viewport_width: COLUMNS,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: CaseOptions::default(),
    })?;

    Ok(state.into())
}

/// # Returns
///
/// What `engine` was left holding after `keys` were typed at it.
///
/// # Panics
///
/// Panics if the keys do not run.
fn outcome_of(mut engine: Engine, keys: &str) -> Outcome {
    engine
        .press_all(keys.chars().map(typed))
        .expect("the keys run against the engine");

    Outcome::of(&mut engine)
}

/// # Returns
///
/// The window the cases are laid out in, narrow enough that the first line of [`PROSE`] wraps.
///
/// # Panics
///
/// Panics if that window is zero columns wide or zero rows tall, which it is not.
fn window() -> Geometry {
    let columns = NonZeroUsize::new(usize::from(COLUMNS)).expect("the columns are not zero");
    let rows = NonZeroUsize::new(usize::from(ROWS)).expect("the rows are not zero");

    Geometry::new(columns, rows)
}

/// # Returns
///
/// The entries of `bindings` that produce nothing at all when the keys reaching them are typed.
fn silent(bindings: &Bindings) -> Vec<String> {
    reachable(bindings)
        .into_iter()
        .filter(|keys| produced(bindings, keys).is_empty())
        .map(|keys| shown(&keys))
        .collect()
}

/// # Returns
///
/// A sequence of keys reaching every entry `bindings` holds, each typed from normal mode. An
/// operator is reached twice, once with a motion after it and once typed again, because those are
/// the two things an operator answers, and a repeat is reached behind a change, because a repeat
/// with no change behind it is a key that answers by doing nothing at all.
///
/// # Panics
///
/// Panics if the table binds keys in a mode no sequence reaches.
fn reachable(bindings: &Bindings) -> Vec<Vec<TerminalKey>> {
    let mut reachable = Vec::new();
    for binding in bindings.entries() {
        let Some((_, prelude)) = REACHED.iter().find(|(mode, _)| *mode == binding.mode) else {
            panic!("`{:?}` is a mode no sequence reaches", binding.mode);
        };
        let mut keys: Vec<TerminalKey> = match &binding.operator {
            Some(operator) => operator.clone(),
            None => prelude.chars().map(key).collect(),
        };
        if let Step::Repeat = binding.step {
            keys.extend(CHANGED.chars().map(key));
        }
        keys.extend(filled(&binding.keys));
        if let Step::Operator { .. } = binding.step {
            let mut doubled = keys.clone();
            doubled.extend(filled(&binding.keys));
            reachable.push(doubled);
            keys.push(key('w'));
        }
        reachable.push(keys);
    }

    reachable
}

/// # Returns
///
/// The keys `edges` are typed by, with a key of any kind stood in for by one the table gives no
/// meaning of its own.
fn filled(edges: &[Edge]) -> Vec<TerminalKey> {
    edges
        .iter()
        .map(|edge| match edge {
            Edge::Key(bound) => *bound,
            Edge::Any => key('x'),
        })
        .collect()
}

/// # Returns
///
/// The entries of `bindings` a longer sequence of the same table buries, each named as the buried
/// sequence and the one burying it.
fn buried(bindings: &Bindings) -> Vec<String> {
    let mut buried = Vec::new();
    for binding in bindings.entries() {
        for longer in bindings.entries() {
            if longer.mode != binding.mode
                || longer.operator != binding.operator
                || longer.keys.len() <= binding.keys.len()
                || !longer.keys.starts_with(&binding.keys)
            {
                continue;
            }
            buried.push(format!(
                "{} < {}",
                spelled(&binding.keys),
                spelled(&longer.keys)
            ));
        }
    }

    buried
}

/// # Returns
///
/// What `bindings` produce for `keys`.
fn produced(bindings: &Bindings, keys: &[TerminalKey]) -> Vec<(Action, EditContext)> {
    let mut machine = Keys::new(bindings.clone());
    let mut produced = Vec::new();
    for key in keys {
        machine.input_key(*key);
        while let Some(pair) = machine.pop() {
            produced.push(pair);
        }
    }

    produced
}

/// # Returns
///
/// What modalkit's own table produces for `keys`, together with the mode it is left in, without
/// the no-ops it fills a step that produces nothing with.
fn through_modalkit(keys: &str) -> Vec<String> {
    let mut machine: VimMachine<TerminalKey> = default_vim_keys();
    let mut produced = Vec::new();
    for key in keys.chars().map(key) {
        machine.input_key(key);
        while let Some((action, context)) = machine.pop() {
            if let Action::NoOp = action {
                continue;
            }
            produced.push(format!("{action:?} @ {context:?}"));
        }
    }
    produced.push(format!("{:?}", machine.mode()));

    produced
}

/// # Returns
///
/// What `bindings` produce for `keys`, together with the mode they leave the machine in.
fn through(bindings: &Bindings, keys: &str) -> Vec<String> {
    let mut machine = Keys::new(bindings.clone());
    let mut produced = Vec::new();
    for key in keys.chars().map(key) {
        machine.input_key(key);
        while let Some((action, context)) = machine.pop() {
            produced.push(format!("{action:?} @ {context:?}"));
        }
    }
    produced.push(format!("{:?}", machine.mode()));

    produced
}

/// # Returns
///
/// The key a terminal reports when `character` is typed with no modifier held.
fn key(character: char) -> TerminalKey {
    TerminalKey::from(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
}

/// # Returns
///
/// `keys` spelled the way vim's manual spells them.
fn shown(keys: &[TerminalKey]) -> String {
    keys.iter().map(ToString::to_string).collect()
}

/// # Returns
///
/// `edges` spelled the way vim's manual spells them, with a key of any kind spelled `{any}`.
fn spelled(edges: &[Edge]) -> String {
    edges
        .iter()
        .map(|edge| match edge {
            Edge::Key(bound) => bound.to_string(),
            Edge::Any => "{any}".to_owned(),
        })
        .collect()
}
