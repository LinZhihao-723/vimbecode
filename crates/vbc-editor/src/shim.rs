//! The seam a motion counted in cells is taken out of modalkit's hands at.
//!
//! modalkit's vim keybindings are the engine's, and its editing actions are the engine's, and that
//! is what `engine` exists to say. What it cannot be is the authority on how wide a grapheme is:
//! modalkit answers a screen motion by dividing a *character* column by the width of the window,
//! which is the same answer only for a text whose every character is one cell wide. On a line of
//! CJK it is wrong by a factor of two, and no amount of care above it recovers the row it skipped.
//!
//! The fix is not to patch that arithmetic but to reach the action before it does. Every key
//! modalkit turns into an action passes through here on its way to the text, a screen motion is
//! recognised by the shape of the action rather than by the key that produced it, and what a
//! motion is measured against is measured by the layout engine -- so that once the shim answers a
//! screen motion, modalkit's own width math is not code that is right or wrong but code that never
//! runs.
//!
//! A motion that asks nothing about cells is not the shim's business and is handed on untouched,
//! which is most of vim: the seam is worth having only if it is invisible to everything but the
//! motions it is for.
//!
//! What the shim measures costs the cursor's own logical line and nothing else. It lays that one
//! line out, on the motion that asked for it, and keeps none of it, because a screenful of rows is
//! the renderer's and the text around the cursor is nobody's.
//!
//! The shim recognises and measures; it does not yet answer. Until it does, the action goes on to
//! modalkit exactly as it did before the seam existed, which is what makes this a seam that can be
//! shown to change nothing before it is asked to change something.
//!
//! Which motions the seam is for is a decision rather than an observation, so it is written down
//! here as one. Every motion modalkit can hand it is classified: one whose answer is counted in
//! cells and which the shim measures is intercepted, one whose answer is counted in cells and
//! which nothing here measures is out of scope, and one whose answer is the same number counted
//! either way is modalkit's. The middle bucket is refused rather than run. `gm`, `gM` and `|` all
//! land where display geometry says and modalkit answers all three by counting characters, and on
//! a line of CJK vim was measured landing somewhere else for each of them; a wrong cursor that
//! reports itself is worth more than a wrong cursor that does not, so those three fail rather than
//! land.
//!
//! Two things the classification does not cover are named here so that they are not mistaken for
//! covered. A motion is classified by the place it names, which makes `j` and `k` characterwise:
//! the line they land on is a position in a text. The column they keep is not one -- vim carries a
//! screen column across a vertical motion where modalkit carries a character index -- and that is
//! a seam of its own, held to vim by a test that pins the divergence rather than left to be
//! rediscovered. And an intercepted motion is still answered by modalkit until the shim answers
//! it, so it is as wrong today as it was before the seam existed; what separates it from a refused
//! one is that it is measured and on its way to an answer.

use editor_types::prelude::{Count, EditTarget, MoveDir1D, MovePosition, MoveType};
use modalkit::actions::EditorAction;
use vbc_layout::line;
use vbc_layout::position::{DisplayPosition, LogicalPosition};

use crate::screen::{row_index, Geometry};

/// A motion whose answer is counted in the cells a terminal draws rather than in the characters a
/// text is written from, which is what makes it the layout engine's to answer and not modalkit's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenMotion {
    /// Move to the first word of the screen line a number of screen lines away, as `g^` does.
    FirstWord(MoveDir1D),

    /// Move a number of screen lines, as `gj` and `gk` do.
    Line(MoveDir1D),

    /// Move to a column of the screen line the cursor is on, as `g0` and `g$` do. `gm` is the
    /// same shape and is out of scope, so [`MovePosition::Middle`] does not arrive here.
    LinePos(MovePosition),

    /// Move to the first word of the line drawn at a place in the viewport, as `H`, `M` and `L`
    /// do.
    ViewportPos(MovePosition),
}

/// What the audit of vim's motions makes of one of them, which is what decides whether the layout
/// engine measures it, the engine refuses it, or modalkit answers it as it always has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Classification {
    /// The answer is counted in cells and the shim measures it, so the layout engine is the
    /// authority on where the motion lands.
    Intercepted(ScreenMotion),

    /// The answer is counted in cells and nothing here measures it, so the motion is refused
    /// rather than answered in characters.
    OutOfScope {
        /// The keys vim's manual names the motion by, which is what a refusal reports.
        keys: &'static str,
    },

    /// The place the motion names is a position in a text rather than a place on a screen, so its
    /// answer is the same however wide a grapheme is drawn and wherever a line breaks.
    Characterwise,

    /// A motion this audit does not name, which is what a [`MoveType`] added to a later release of
    /// the crate that declares it arrives as until someone classifies it.
    Unclassified,
}

/// One screen motion the shim took, and where the layout engine says the cursor stood when it
/// arrived.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Intercepted {
    /// The motion that was asked for.
    pub motion: ScreenMotion,

    /// The count the motion resolved to, in the terms that motion counts in: `gj` counts the
    /// screen lines to move down and resolves to one where no count was typed, `g$` counts the
    /// screen lines below the cursor's own and resolves to none, and `g0` takes no count at all.
    pub count: usize,

    /// Where the cursor was drawn, in the rows the cursor's own logical line lays out into: the
    /// row within that line, and the column in cells. This is the measurement modalkit divides a
    /// character column to guess at, and the one the shim will answer the motion from.
    pub from: DisplayPosition,
}

/// The seam itself: the layout the engine's screen motions are measured in, and the motions it has
/// taken so far.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shim {
    geometry: Geometry,
    intercepted: Vec<Intercepted>,
}

impl Shim {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created shim measuring the motions it takes in `geometry`, having taken none.
    #[must_use]
    pub fn new(geometry: Geometry) -> Self {
        Self {
            geometry,
            intercepted: Vec::new(),
        }
    }

    /// # Returns
    ///
    /// The layout a screen motion is measured in.
    #[must_use]
    pub fn geometry(&self) -> &Geometry {
        &self.geometry
    }

    /// # Returns
    ///
    /// Every screen motion the shim has taken, oldest first.
    #[must_use]
    pub fn intercepted(&self) -> &[Intercepted] {
        &self.intercepted
    }

    /// Takes the screen motion `motion`, resolved to the count `count`, with the cursor standing
    /// at `at` on a logical line whose text is `line`, and records where the layout engine draws
    /// that cursor. Only that one line is laid out, so what the measurement costs is the cursor's
    /// line rather than the text around it. The line is the text alone, without its line ending.
    pub fn intercept(
        &mut self,
        motion: ScreenMotion,
        count: usize,
        at: LogicalPosition,
        line: &str,
    ) {
        let rows = line::lay_out(
            at.line,
            line,
            self.geometry.columns(),
            self.geometry.metrics(),
            self.geometry.options(),
        );
        let index = row_index(&rows, at.grapheme);
        let columns = rows[index].columns();
        let offset = at
            .grapheme
            .saturating_sub(rows[index].start())
            .min(columns.len() - 1);

        self.intercepted.push(Intercepted {
            motion,
            count,
            from: DisplayPosition {
                row: index,
                column: columns[offset],
            },
        });
    }
}

/// # Returns
///
/// What the audit makes of `move_type`.
#[must_use]
pub fn classify(move_type: &MoveType) -> Classification {
    match move_type {
        MoveType::ScreenFirstWord(direction) => {
            Classification::Intercepted(ScreenMotion::FirstWord(*direction))
        }
        MoveType::ScreenLine(direction) => {
            Classification::Intercepted(ScreenMotion::Line(*direction))
        }
        MoveType::ScreenLinePos(MovePosition::Middle) => Classification::OutOfScope { keys: "gm" },
        MoveType::ScreenLinePos(position) => {
            Classification::Intercepted(ScreenMotion::LinePos(*position))
        }
        MoveType::ViewportPos(position) => {
            Classification::Intercepted(ScreenMotion::ViewportPos(*position))
        }
        MoveType::LineColumnOffset => Classification::OutOfScope { keys: "|" },
        MoveType::LinePercent | MoveType::LinePos(MovePosition::Middle) => {
            Classification::OutOfScope { keys: "gM" }
        }
        MoveType::BufferByteOffset
        | MoveType::BufferLineOffset
        | MoveType::BufferLinePercent
        | MoveType::BufferPos(_)
        | MoveType::Column(_, _)
        | MoveType::FinalNonBlank(_)
        | MoveType::FirstWord(_)
        | MoveType::ItemMatch
        | MoveType::Line(_)
        | MoveType::LinePos(_)
        | MoveType::ParagraphBegin(_)
        | MoveType::SectionBegin(_)
        | MoveType::SectionEnd(_)
        | MoveType::SentenceBegin(_)
        | MoveType::WordBegin(_, _)
        | MoveType::WordEnd(_, _) => Classification::Characterwise,
        _ => Classification::Unclassified,
    }
}

/// # Returns
///
/// What the audit makes of the motion `action` asks for, and the count it asks for it with, and
/// `None` where the action asks for something that is not a motion at all.
#[must_use]
pub fn classified(action: &EditorAction) -> Option<(Classification, Count)> {
    let EditorAction::Edit(_, EditTarget::Motion(move_type, count)) = action else {
        return None;
    };

    Some((classify(move_type), count.clone()))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use editor_types::prelude::{Specifier, WordStyle};

    use super::*;

    /// The columns the measurements below are made in.
    const COLUMNS: usize = 10;

    /// The rows the measurements below are made in.
    const ROWS: usize = 5;

    /// A line of characters two cells wide apiece, on which a character column and a display
    /// column are not the same number.
    const WIDE: &str = "你好世界一二三四五六";

    #[test]
    fn a_motion_counted_in_cells_is_the_layout_engines() {
        for (move_type, motion) in [
            (
                MoveType::ScreenFirstWord(MoveDir1D::Next),
                ScreenMotion::FirstWord(MoveDir1D::Next),
            ),
            (
                MoveType::ScreenLine(MoveDir1D::Previous),
                ScreenMotion::Line(MoveDir1D::Previous),
            ),
            (
                MoveType::ScreenLinePos(MovePosition::End),
                ScreenMotion::LinePos(MovePosition::End),
            ),
            (
                MoveType::ViewportPos(MovePosition::Middle),
                ScreenMotion::ViewportPos(MovePosition::Middle),
            ),
        ] {
            assert_eq!(
                Some((Classification::Intercepted(motion), Count::Exact(2))),
                classified(&edit(EditTarget::Motion(move_type, Count::Exact(2))))
            );
        }
    }

    #[test]
    fn a_motion_counted_in_cells_that_nothing_measures_is_out_of_scope() {
        for (move_type, keys) in [
            (MoveType::ScreenLinePos(MovePosition::Middle), "gm"),
            (MoveType::LinePos(MovePosition::Middle), "gM"),
            (MoveType::LinePercent, "gM"),
            (MoveType::LineColumnOffset, "|"),
        ] {
            assert_eq!(
                Some((Classification::OutOfScope { keys }, Count::Contextual)),
                classified(&edit(EditTarget::Motion(move_type, Count::Contextual)))
            );
        }
    }

    #[test]
    fn a_motion_counted_in_characters_is_left_to_modalkit() {
        for move_type in [
            MoveType::Line(MoveDir1D::Next),
            MoveType::Column(MoveDir1D::Next, false),
            MoveType::LinePos(MovePosition::End),
            MoveType::FirstWord(MoveDir1D::Next),
            MoveType::WordBegin(WordStyle::Little, MoveDir1D::Next),
        ] {
            assert_eq!(
                Some((Classification::Characterwise, Count::Contextual)),
                classified(&edit(EditTarget::Motion(move_type, Count::Contextual)))
            );
        }
    }

    #[test]
    fn an_action_that_is_not_a_motion_is_left_to_modalkit() {
        assert_eq!(None, classified(&edit(EditTarget::CurrentPosition)));
        assert_eq!(None, classified(&edit(EditTarget::Selection)));
        assert_eq!(None, classified(&EditorAction::Mark(Specifier::Contextual)));
    }

    #[test]
    fn the_cursor_is_measured_in_cells_rather_than_in_characters() {
        let mut shim = Shim::new(geometry());
        shim.intercept(
            ScreenMotion::Line(MoveDir1D::Next),
            1,
            LogicalPosition {
                line: 0,
                grapheme: 6,
            },
            WIDE,
        );

        assert_eq!(
            [Intercepted {
                motion: ScreenMotion::Line(MoveDir1D::Next),
                count: 1,
                from: DisplayPosition { row: 1, column: 2 },
            }],
            shim.intercepted()
        );
    }

    #[test]
    fn a_cursor_past_the_last_grapheme_of_a_line_is_measured_at_the_column_past_it() {
        let mut shim = Shim::new(geometry());
        shim.intercept(
            ScreenMotion::LinePos(MovePosition::End),
            1,
            LogicalPosition {
                line: 0,
                grapheme: 5,
            },
            "abc",
        );

        assert_eq!(
            DisplayPosition { row: 0, column: 3 },
            shim.intercepted()[0].from
        );
    }

    /// # Returns
    ///
    /// The layout the measurements above are made in.
    fn geometry() -> Geometry {
        Geometry::new(
            NonZeroUsize::new(COLUMNS).expect("the columns are not zero"),
            NonZeroUsize::new(ROWS).expect("the rows are not zero"),
        )
    }

    /// # Returns
    ///
    /// The action modalkit produces for a motion over `target` with no operator applied to it.
    fn edit(target: EditTarget) -> EditorAction {
        EditorAction::Edit(Specifier::Contextual, target)
    }
}
