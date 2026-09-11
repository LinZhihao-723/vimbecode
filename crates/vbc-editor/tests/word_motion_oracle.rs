//! Cross-checks the word motions against the byte vim itself left the cursor on.
//!
//! `w`, `e`, `b`, `W`, `E` and `B` are about character *classes* rather than about widths, so
//! nothing the layout engine was built for touches them: they are modalkit's, and on plain ASCII
//! letters they agree with vim by construction. Everywhere else they were unverified, which is
//! what this file is for. The corpus baseline records the byte vim left the cursor on for every
//! case of the word-motion grid, so replaying the same keys against the same buffer here says
//! whether the engine classes a character the way vim classes it.
//!
//! The grid crosses the six motions with the shapes a classifier has no reason to be right on:
//! CJK ideographs run together with Latin, CJK punctuation, a ZWJ family emoji, a run of unjoined
//! emoji, decomposed clusters carrying combining marks, `snake_case` and `kebab-case` identifiers,
//! precomposed Latin-1 letters, Latin letters past U+00FF, and runs of ASCII punctuation.
//!
//! The two classifiers are written down here because the divergences below are all one of the
//! three places they part. vim classes a character by `'iskeyword'` up to U+00FF and by
//! `utf_class()` above it, which gives ideographs, emoji and the punctuation of each script
//! classes of their own, and reads a combining mark as composing onto the character in front of
//! it. The engine reads `is_alphanumeric() || '_'` as one class and a fixed set of ASCII
//! punctuation -- `!`--`/`, `[`--`^`, `` ` ``, `{`--`~` -- as another, and everything else,
//! `:;<=>?@` and every non-alphanumeric character past ASCII included, as blank.
//!
//! Where the engine answers a case somewhere else than vim, the case is named below with the
//! reason, so that a case which starts or stops agreeing fails this file rather than quietly
//! leaving the sample. A pinned case is a difference recorded, not a difference adopted: neither
//! answer is written into the corpus, and the baseline goes on saying what vim says.
//!
//! The `W`, `E` and `B` cases all agree, on every shape: a WORD is a run of non-blank characters
//! and both classifiers read the same ASCII whitespace as blank, so nothing above decides them.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use vbc_editor::engine::{typed, Engine};
use vbc_editor::screen::Geometry;
use vbc_layout::line::Options;
use vbc_layout::width::{grapheme_indices, AmbiWidth, Metrics};
use vbc_oracle::baseline::{self, Baseline};
use vbc_oracle::corpus::{self, AmbiWidth as CaseAmbiWidth, Case, Corpus, Tag};
use vbc_oracle::state::EditorState;

/// The number of word-motion cases the engine answers where vim answers them, which is the sample
/// this cross-check is worth. A case added to the grid lands in the sample or in the list below,
/// and either way this number moves.
const MOTIONS_ANCHORED: usize = 41;

/// The word-motion cases the engine leaves the cursor on another byte than vim does, each with the
/// reason.
const MOTION_DIVERGENCES: [(&str, &str); 19] = [
    ("word-b-cjk-latin", IDEOGRAPHS_ARE_NOT_THEIR_OWN_CLASS),
    ("word-b-cjk-punctuation", CJK_PUNCTUATION_IS_BLANK),
    ("word-b-combining", A_COMBINING_MARK_IS_BLANK),
    ("word-b-emoji-run", AN_EMOJI_IS_BLANK),
    ("word-b-punctuation-run", ASCII_PUNCTUATION_IS_INCOMPLETE),
    ("word-b-snake-case", ASCII_PUNCTUATION_IS_INCOMPLETE),
    ("word-b-zwj-family", AN_EMOJI_IS_BLANK),
    ("word-e-cjk-latin", IDEOGRAPHS_ARE_NOT_THEIR_OWN_CLASS),
    ("word-e-cjk-punctuation", CJK_PUNCTUATION_IS_BLANK),
    ("word-e-combining", A_COMBINING_MARK_IS_BLANK),
    ("word-e-emoji-run", AN_EMOJI_IS_BLANK),
    ("word-e-snake-case", ASCII_PUNCTUATION_IS_INCOMPLETE),
    ("word-e-zwj-family", AN_EMOJI_IS_BLANK),
    ("word-w-cjk-latin", IDEOGRAPHS_ARE_NOT_THEIR_OWN_CLASS),
    ("word-w-cjk-punctuation", CJK_PUNCTUATION_IS_BLANK),
    ("word-w-combining", A_COMBINING_MARK_IS_BLANK),
    ("word-w-emoji-run", AN_EMOJI_IS_BLANK),
    ("word-w-snake-case", ASCII_PUNCTUATION_IS_INCOMPLETE),
    ("word-w-zwj-family", AN_EMOJI_IS_BLANK),
];

/// The reason the engine reads a run of ideographs and the Latin run against it as one word.
const IDEOGRAPHS_ARE_NOT_THEIR_OWN_CLASS: &str =
    "vim gives an ideograph a class of its own, so a boundary falls where a run of them meets \
     Latin; the engine reads every alphanumeric character as one class and the two runs as one \
     word";

/// The reason the engine steps over a CJK punctuation mark vim stops on.
const CJK_PUNCTUATION_IS_BLANK: &str =
    "vim gives CJK punctuation and the ideographs around it classes of their own; the engine \
     reads a punctuation mark past ASCII as blank and the scripts around it as one class";

/// The reason the engine ends a word at a combining mark vim reads as part of one.
const A_COMBINING_MARK_IS_BLANK: &str =
    "vim reads a combining mark as composing onto the character in front of it, so a decomposed \
     cluster is one word; the engine reads the mark as blank and ends the word at it";

/// The reason the engine steps over an emoji vim stops on.
const AN_EMOJI_IS_BLANK: &str =
    "vim gives an emoji a class of its own and stops on one, a joiner included; the engine reads \
     an emoji as blank and steps over the whole run";

/// The reason the engine steps over the ASCII punctuation vim stops on.
const ASCII_PUNCTUATION_IS_INCOMPLETE: &str =
    "vim's default 'iskeyword' leaves `:;<=>?@` out of a word and so stops on them; the engine's \
     punctuation class leaves them out as well but does not put them anywhere else, so they are \
     blank and it steps over them";

/// The word-motion cases that leave the cursor inside a grapheme cluster, with the reason. Landing
/// there is not a mistake by itself: vim does it too, and both are asserted to land on the same
/// byte by `word_motions_land_where_vim_landed`.
const CURSOR_INSIDE_A_CLUSTER: [(&str, &str); 1] = [(
    "word-big-e-zwj-family",
    "`E` ends on the last character of a blank-separated WORD, and the last character of a ZWJ \
     family is the last emoji of the cluster rather than the cluster",
)];

#[test]
fn word_motions_land_where_vim_landed() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut anchored = BTreeSet::new();
    let mut diverged = BTreeSet::new();
    for case in corpus.with_tag(Tag::WordMotion) {
        let state = state_of(&baseline, case);
        if cursor(case) == (state.cursor.line, state.cursor.column) {
            anchored.insert(case.id.as_str());
        } else {
            diverged.insert(case.id.as_str());
        }
    }

    assert_eq!(
        ids(&MOTION_DIVERGENCES),
        diverged,
        "the word-motion cases that disagree with vim are not the ones named as disagreeing"
    );
    assert_eq!(MOTIONS_ANCHORED, anchored.len());
}

#[test]
fn only_the_named_motions_land_inside_a_grapheme_cluster() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");

    let mut inside = BTreeSet::new();
    for case in corpus.with_tag(Tag::WordMotion) {
        let (line, column) = cursor(case);
        let text = lines_of(&case.buffer)
            .nth(usize::try_from(line).expect("a cursor line fits in a usize"))
            .unwrap_or_else(|| panic!("the cursor of `{}` rests on a line of its buffer", case.id));
        let offset = usize::try_from(column).expect("a cursor column fits in a usize");
        if text.len() != offset && !grapheme_indices(text).any(|(start, _)| start == offset) {
            inside.insert(case.id.as_str());
        }
    }

    assert_eq!(ids(&CURSOR_INSIDE_A_CLUSTER), inside);
}

/// # Returns
///
/// The identifiers `divergences` names.
fn ids<'divergences>(divergences: &[(&'divergences str, &str)]) -> BTreeSet<&'divergences str> {
    divergences.iter().map(|&(id, _)| id).collect()
}

/// # Returns
///
/// The state the baseline records vim ending `case` in.
fn state_of<'baseline>(baseline: &'baseline Baseline, case: &Case) -> &'baseline EditorState {
    baseline
        .cases
        .get(&case.id)
        .unwrap_or_else(|| panic!("the baseline holds the case `{}`", case.id))
}

/// # Returns
///
/// The logical lines of a buffer's text, with the newline closing the last line excluded.
fn lines_of(buffer: &str) -> impl Iterator<Item = &str> {
    buffer.strip_suffix('\n').unwrap_or(buffer).split('\n')
}

/// # Returns
///
/// The line and the byte an engine laid out as `case` declares leaves the cursor on once the
/// case's keys are typed at it, counted the way vim counts a cursor.
///
/// # Panics
///
/// Panics if the case's keys do not run.
fn cursor(case: &Case) -> (u64, u64) {
    let mut engine = Engine::laid_out_in(&case.buffer, geometry(case));
    engine
        .press_all(case.keys.chars().map(typed))
        .expect("the keys run");
    let at = engine.cursor();

    (at.line as u64, at.column as u64)
}

/// # Returns
///
/// The window `case` is laid out in.
fn geometry(case: &Case) -> Geometry {
    let columns = NonZeroUsize::new(usize::from(case.viewport_width))
        .expect("a viewport is not zero columns wide");
    let rows = NonZeroUsize::new(usize::from(case.viewport_height))
        .expect("a viewport is not zero rows tall");
    let ambiwidth = match case.options.ambiwidth {
        CaseAmbiWidth::Single => AmbiWidth::Single,
        CaseAmbiWidth::Double => AmbiWidth::Double,
    };
    let tab_stop =
        NonZeroUsize::new(usize::from(case.options.tabstop)).expect("a tab stop is not zero");
    let options = Options::new()
        .with_break_indent(case.options.breakindent)
        .with_show_break(case.options.showbreak.clone())
        .with_line_break(case.options.linebreak);

    Geometry::new(columns, rows)
        .with_metrics(Metrics::new(ambiwidth, tab_stop))
        .with_options(options)
}
