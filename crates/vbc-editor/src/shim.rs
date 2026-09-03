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
//! four motions it is for.
//!
//! What the shim measures costs the cursor's own logical line and nothing else. It lays that one
//! line out, on the motion that asked for it, and keeps none of it, because a screenful of rows is
//! the renderer's and the text around the cursor is nobody's.
//!
//! The shim recognises and measures; it does not yet answer. Until it does, the action goes on to
//! modalkit exactly as it did before the seam existed, which is what makes this a seam that can be
//! shown to change nothing before it is asked to change something.

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

    /// Move to a column of the screen line the cursor is on, as `g0`, `gm` and `g$` do.
    LinePos(MovePosition),

    /// Move to the first word of the line drawn at a place in the viewport, as `H`, `M` and `L`
    /// do.
    ViewportPos(MovePosition),
}

impl ScreenMotion {
    /// # Returns
    ///
    /// The screen motion `move_type` names, and `None` where it names a motion counted in
    /// characters, which modalkit is already the authority on.
    #[must_use]
    pub fn of(move_type: &MoveType) -> Option<Self> {
        match move_type {
            MoveType::ScreenFirstWord(direction) => Some(Self::FirstWord(*direction)),
            MoveType::ScreenLine(direction) => Some(Self::Line(*direction)),
            MoveType::ScreenLinePos(position) => Some(Self::LinePos(*position)),
            MoveType::ViewportPos(position) => Some(Self::ViewportPos(*position)),
            _ => None,
        }
    }
}

/// One screen motion the shim took, and where the layout engine says the cursor stood when it
/// arrived.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Intercepted {
    /// The motion that was asked for.
    pub motion: ScreenMotion,

    /// How many times it was asked for.
    pub count: Count,

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

    /// Takes the screen motion `motion`, asked for `count` times with the cursor standing at `at`
    /// on a logical line whose text is `line`, and records where the layout engine draws that
    /// cursor. Only that one line is laid out, so what the measurement costs is the cursor's line
    /// rather than the text around it. The line is the text alone, without its line ending.
    pub fn intercept(
        &mut self,
        motion: ScreenMotion,
        count: Count,
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
/// The screen motion `action` asks for and the count it asks for it with, and `None` where the
/// action asks for something no layout has a say in.
#[must_use]
pub fn screen_motion(action: &EditorAction) -> Option<(ScreenMotion, Count)> {
    let EditorAction::Edit(_, EditTarget::Motion(move_type, count)) = action else {
        return None;
    };

    ScreenMotion::of(move_type).map(|motion| (motion, count.clone()))
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
    fn the_four_motions_counted_in_cells_are_recognised() {
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
                Some((motion, Count::Exact(2))),
                screen_motion(&edit(EditTarget::Motion(move_type, Count::Exact(2))))
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
            MoveType::LineColumnOffset,
        ] {
            assert_eq!(
                None,
                screen_motion(&edit(EditTarget::Motion(move_type, Count::Contextual)))
            );
        }
    }

    #[test]
    fn an_action_that_is_not_a_motion_is_left_to_modalkit() {
        assert_eq!(None, screen_motion(&edit(EditTarget::CurrentPosition)));
        assert_eq!(None, screen_motion(&edit(EditTarget::Selection)));
        assert_eq!(
            None,
            screen_motion(&EditorAction::Mark(Specifier::Contextual))
        );
    }

    #[test]
    fn the_cursor_is_measured_in_cells_rather_than_in_characters() {
        let mut shim = Shim::new(geometry());
        shim.intercept(
            ScreenMotion::Line(MoveDir1D::Next),
            Count::Contextual,
            LogicalPosition {
                line: 0,
                grapheme: 6,
            },
            WIDE,
        );

        assert_eq!(
            [Intercepted {
                motion: ScreenMotion::Line(MoveDir1D::Next),
                count: Count::Contextual,
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
            Count::Contextual,
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
