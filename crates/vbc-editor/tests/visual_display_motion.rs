//! Cross-checks visual selections extended by a motion counted in screen lines against the vim
//! those motions were adopted from.
//!
//! A visual selection is not a thing the seam builds: modalkit's `v` and `V` set the shape a
//! motion is run under, and every motion that arrives while a shape is set extends the selection
//! instead of moving the cursor. What is checked here is that the motions counted in screen lines
//! extend it where vim extends it, and that the operator applied afterwards takes the selection
//! that leaves -- which is the whole of what a caller sees of a visual mode.
//!
//! The comparison is the one `vim_engine.rs` makes, in the record both share: the four dimensions
//! the engine is the authority on -- the text, the cursor, the mode and the registers, each
//! register with the type a put would reinsert it with -- taken from a real vim laid out in the
//! same viewport.
//!
//! Every case is written so that a display motion and the logical motion spelled the same way
//! answer differently, and that is asserted of vim rather than assumed: a case where `gj` and `j`
//! land in the same place would pass this file against an engine that has never heard of a screen
//! line. The cases that turn on the difference within one logical line are the ones a wrapped line
//! makes: on the only line of a buffer, `vgj` selects a row's worth of it and `vj` selects the one
//! character it started on.
//!
//! What is not covered, and is not implied anywhere here:
//!
//! * Blockwise visual mode. `C-v` is typed nowhere in this file, so nothing here says whether a
//!   blockwise selection extends over a display motion. It is not tested, and this file makes no
//!   claim about it.
//! * A line whose graphemes are wider than one cell. The shim measures a screen motion but does
//!   not yet answer one, so a motion over such a line is still modalkit's own guess at where a
//!   row ends. That divergence is asserted below rather than left to be discovered, and the
//!   assertion is what fails when the shim starts answering.

mod outcome;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use vbc_editor::engine::{typed, Engine};
use vbc_editor::screen::Geometry;
use vbc_oracle::corpus;
use vbc_oracle::state::{Mode, Register, RegisterType};
use vbc_oracle::vim::VimDriver;

use crate::outcome::Outcome;

/// One cross-check: the keys that extend a selection over a display motion, and the keys that
/// extend it over the logical motion spelled the same way, which is what says the case turns on
/// the difference between them.
struct Extension {
    id: &'static str,
    text: &'static str,
    keys: &'static str,
    logical: &'static str,
}

/// One logical line long enough to wrap into two rows of the viewport below, on which a display
/// motion moves within a line a logical motion cannot move within at all.
const WRAPPED: &str = "abcdefghijklmnopqrstuvwxyz0123456789\n";

/// Three logical lines, the first of which wraps into three rows, on which a display motion and a
/// logical motion both move and land in different places.
const PARAGRAPH: &str = "the quick brown fox jumps over the lazy dog\nsecond line here\nthird\n";

/// A line of characters drawn two cells wide apiece, which wraps where the layout engine says it
/// does and not where modalkit's own width math says it does.
const WIDE: &str = "你好世界一二三四五六七八九十\nsecond\n";

/// The cells the viewport these cases are laid out in is wide.
const COLUMNS: u16 = 20;

/// The screen lines the viewport these cases are laid out in is tall.
const ROWS: u16 = 10;

/// The selections a display motion extends, each paired with the logical motion it must not
/// behave as.
const EXTENSIONS: [Extension; 10] = [
    Extension {
        id: "charwise down a row of one wrapped line",
        text: WRAPPED,
        keys: "vgj",
        logical: "vj",
    },
    Extension {
        id: "charwise down a row, deleted",
        text: WRAPPED,
        keys: "vgjd",
        logical: "vjd",
    },
    Extension {
        id: "charwise down a row, yanked",
        text: WRAPPED,
        keys: "vgjy",
        logical: "vjy",
    },
    Extension {
        id: "charwise down two rows at once",
        text: WRAPPED,
        keys: "v2gjd",
        logical: "v2jd",
    },
    Extension {
        id: "charwise up a row of one wrapped line",
        text: WRAPPED,
        keys: "25lvgkd",
        logical: "25lvkd",
    },
    Extension {
        id: "linewise down a row of one wrapped line",
        text: PARAGRAPH,
        keys: "Vgjy",
        logical: "Vjy",
    },
    Extension {
        id: "linewise down two rows, deleted",
        text: PARAGRAPH,
        keys: "V2gjd",
        logical: "V2jd",
    },
    Extension {
        id: "charwise out of a wrapped line into the next",
        text: PARAGRAPH,
        keys: "v3gjy",
        logical: "v3jy",
    },
    Extension {
        id: "charwise up into a wrapped line",
        text: PARAGRAPH,
        keys: "jvgkd",
        logical: "jvkd",
    },
    Extension {
        id: "charwise up two rows into a wrapped line",
        text: PARAGRAPH,
        keys: "jjvgkgky",
        logical: "jjvkky",
    },
];

#[test]
fn every_visual_display_motion_ends_where_vim_ends_it() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for extension in &EXTENSIONS {
        assert_eq!(
            vim_outcome(&vim, extension.text, extension.keys)?,
            replayed(extension.text, extension.keys)?,
            "`{}` left the engine somewhere other than where vim left it",
            extension.id
        );
    }

    Ok(())
}

#[test]
fn every_case_answers_a_display_motion_differently_from_its_logical_twin() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for extension in &EXTENSIONS {
        assert_ne!(
            vim_outcome(&vim, extension.text, extension.logical)?,
            vim_outcome(&vim, extension.text, extension.keys)?,
            "`{}` is a case vim answers the same way whether it counts rows or lines",
            extension.id
        );
    }

    Ok(())
}

#[test]
fn an_engine_that_was_handed_no_keys_diverges_from_vim() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for extension in &EXTENSIONS {
        assert_ne!(
            vim_outcome(&vim, extension.text, extension.keys)?,
            replayed(extension.text, "")?,
            "`{}` agreed with vim without a single key being typed at the engine",
            extension.id
        );
    }

    Ok(())
}

#[test]
fn a_charwise_selection_crossing_a_wrap_boundary_deletes_what_vim_deletes() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let deleted = Register {
        text: "abcdefghijklmnopqrstu".to_owned(),
        register_type: RegisterType::Charwise,
    };
    let ended = Outcome {
        text: "vwxyz0123456789\n".to_owned(),
        line: 0,
        column: 0,
        mode: Mode::Normal,
        registers: [('"', deleted.clone()), ('-', deleted)]
            .into_iter()
            .collect(),
    };

    assert_eq!(ended, replayed(WRAPPED, "vgjd")?);
    assert_eq!(ended, vim_outcome(&vim, WRAPPED, "vgjd")?);

    Ok(())
}

#[test]
fn a_linewise_selection_crossing_a_wrap_boundary_yanks_what_vim_yanks() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let yanked = Register {
        text: "the quick brown fox jumps over the lazy dog\n".to_owned(),
        register_type: RegisterType::Linewise,
    };
    let ended = Outcome {
        text: PARAGRAPH.to_owned(),
        line: 0,
        column: 0,
        mode: Mode::Normal,
        registers: [('"', yanked.clone()), ('0', yanked)].into_iter().collect(),
    };

    assert_eq!(ended, replayed(PARAGRAPH, "Vgjy")?);
    assert_eq!(ended, vim_outcome(&vim, PARAGRAPH, "Vgjy")?);

    Ok(())
}

#[test]
fn a_backward_selection_takes_the_row_above_as_vim_does() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let deleted = Register {
        text: "fghijklmnopqrstuvwxyz".to_owned(),
        register_type: RegisterType::Charwise,
    };
    let ended = Outcome {
        text: "abcde0123456789\n".to_owned(),
        line: 0,
        column: 5,
        mode: Mode::Normal,
        registers: [('"', deleted.clone()), ('-', deleted)]
            .into_iter()
            .collect(),
    };

    assert_eq!(ended, replayed(WRAPPED, "25lvgkd")?);
    assert_eq!(ended, vim_outcome(&vim, WRAPPED, "25lvgkd")?);

    Ok(())
}

#[test]
fn a_selection_over_a_line_of_wide_characters_is_not_yet_measured_in_cells() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    assert_ne!(
        vim_outcome(&vim, WIDE, "vgjy")?,
        replayed(WIDE, "vgjy")?,
        "a display motion over a wide line agrees with vim, so the shim answers it now and this \
         file should hold it to vim rather than to the divergence"
    );

    Ok(())
}

/// # Returns
///
/// What the engine was left holding after `keys` were typed at `text`, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Engine::press_all`]'s return values on failure.
fn replayed(text: &str, keys: &str) -> anyhow::Result<Outcome> {
    let mut engine = Engine::laid_out_in(text, geometry());
    engine.press_all(keys.chars().map(typed))?;

    Ok(Outcome::of(&mut engine))
}

/// # Returns
///
/// What vim was left holding after the same keys were typed at the same text in the same viewport,
/// on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn vim_outcome(vim: &VimDriver, text: &str, keys: &str) -> anyhow::Result<Outcome> {
    let case = corpus::Case {
        id: "visual-display-motion".to_owned(),
        description: "A visual selection extended by a display motion.".to_owned(),
        buffer: text.to_owned(),
        keys: keys.to_owned(),
        viewport_width: COLUMNS,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: corpus::Options::default(),
    };

    Ok(Outcome::from(vim.run_case(&case)?))
}

/// # Returns
///
/// The viewport the cases here are laid out in, on both sides of the comparison.
///
/// # Panics
///
/// Panics if the viewport is zero columns wide or zero rows tall, which it is not.
fn geometry() -> Geometry {
    let columns =
        NonZeroUsize::new(usize::from(COLUMNS)).expect("the viewport is not zero columns wide");
    let rows = NonZeroUsize::new(usize::from(ROWS)).expect("the viewport is not zero rows tall");

    Geometry::new(columns, rows)
}
