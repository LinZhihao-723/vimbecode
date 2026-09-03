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
//! What the shim answers is the motions counted against a line's own rows: `gj` and `gk` a number
//! of screen lines away, `g0`, `gm` and `g$` along the screen line the cursor is on, and `g^` at
//! its first word. `H`, `M` and `L` are counted against the window a text is scrolled inside
//! rather than against the text, so they are recognised and measured here and left to modalkit
//! until the seam is handed that window. A window with `'wrap'` unset draws one row of a line and
//! scrolls it sideways, so the row a motion is counted against there is the viewport's to say as
//! well; a line is laid out here as a wrapped line whatever the window does with it, and the
//! answer is right only for a window that wraps.
//!
//! What the shim measures costs the logical lines a motion steps through and nothing else. It lays
//! each of them out as the walk reaches it, on the motion that asked for it, and keeps none of
//! them, because a screenful of rows is the renderer's and the text around the cursor is nobody's.
//! A `gj` therefore costs one line and a `3gj` at most four, whether the text is a hundred lines
//! or fifty thousand.
//!
//! The one thing the shim does keep is the column a chain of screen motions is walking down, which
//! is vim's `curswant` measured in cells: a `gj` after a `gj` returns to the column the first one
//! left from rather than to the column the row it landed on cut it back to, and a `g$` in front of
//! either sticks to the end of every row they pass. Anything else the engine runs forgets that
//! column, which is what keeps it the memory of a chain rather than a memory that outlives one.

use std::borrow::Cow;

use editor_types::prelude::{Count, EditTarget, MoveDir1D, MovePosition, MoveType};
use modalkit::actions::EditorAction;
use vbc_layout::line::{self, DisplayRow};
use vbc_layout::position::{DisplayPosition, LogicalPosition};
use vbc_layout::width::graphemes;

use crate::screen::{row_index, Geometry};

/// The one grapheme a screen draws as the blanks it advances by rather than as itself.
const TAB: &str = "\t";

/// The text a screen motion is walked over, read one logical line at a time so that a step costs
/// the lines it passes through rather than the text they sit in.
pub trait Text {
    /// # Returns
    ///
    /// The number of logical lines the text holds, which is at least one.
    fn line_count(&self) -> usize;

    /// # Returns
    ///
    /// The logical line at `index` without its line ending, and [`None`] where the text holds no
    /// such line.
    fn line(&self, index: usize) -> Option<Cow<'_, str>>;
}

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

    /// The count the motion resolved to, in the terms that motion counts in: `gj` counts the
    /// screen lines to move down and resolves to one where no count was typed, `g$` counts the
    /// screen lines below the cursor's own and resolves to none, and `g0` takes no count at all.
    pub count: usize,

    /// Where the cursor was drawn, in the rows the cursor's own logical line lays out into: the
    /// row within that line, and the column in cells. This is the measurement modalkit divides a
    /// character column to guess at, and the one the shim answers the motion from.
    pub from: DisplayPosition,
}

/// The place along a screen line a motion is walking towards, which is vim's `curswant` measured
/// in cells rather than in characters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wanted {
    /// A display column, which a row too short to reach it cuts back to its last grapheme.
    Column(usize),

    /// The last grapheme of whatever row the motion lands on, however long that row is.
    End,

    /// The first grapheme of the landing row that is not a blank.
    FirstWord,
}

/// The seam itself: the layout the engine's screen motions are measured in, the motions it has
/// taken so far, and the column the chain of them is walking down.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shim {
    geometry: Geometry,
    intercepted: Vec<Intercepted>,
    wanted: Option<Wanted>,
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
            wanted: None,
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

    /// Reads what an action this seam does not answer leaves a chain of screen motions wanting.
    ///
    /// vim's own `$` wants the end of every row a screen motion after it lands on, which is the
    /// one column a chain can be walking down without a screen motion having started it. Anything
    /// else that moves the cursor wants nothing at all, and the next screen motion is measured
    /// from wherever the cursor was left.
    ///
    /// An action that only records where the text has been moves no cursor and so ends no chain.
    /// modalkit files a history checkpoint after every key, and a chain such a checkpoint broke
    /// would be a chain no second motion could ever join.
    pub fn note(&mut self, action: &EditorAction) {
        if matches!(action, EditorAction::History(_)) {
            return;
        }

        self.wanted = ends_a_line(action).then_some(Wanted::End);
    }

    /// Takes the screen motion `motion`, resolved to the count `count`, with the cursor standing
    /// at `at` in `text`, and answers it from the layout engine.
    ///
    /// # Type Parameters
    ///
    /// * `TextType` - The text the motion is walked over.
    ///
    /// # Returns
    ///
    /// Where the motion leaves the cursor, and [`None`] for a motion this seam does not answer,
    /// which the caller is then to leave to modalkit.
    pub fn intercept<TextType: Text>(
        &mut self,
        motion: ScreenMotion,
        count: usize,
        at: LogicalPosition,
        text: &TextType,
    ) -> Option<LogicalPosition> {
        let rows = self.lay_out(at.line, text);
        let row = row_index(&rows, at.grapheme);
        let from = DisplayPosition {
            row,
            column: column_of(&rows[row], at.grapheme),
        };
        self.intercepted.push(Intercepted {
            motion,
            count,
            from,
        });

        let (steps, direction, wanted) = match motion {
            ScreenMotion::ViewportPos(_) => return None,
            ScreenMotion::FirstWord(direction) => (count, direction, Wanted::FirstWord),
            ScreenMotion::Line(direction) => (
                count,
                direction,
                self.wanted.unwrap_or(Wanted::Column(from.column)),
            ),
            ScreenMotion::LinePos(MovePosition::Beginning) => {
                (0, MoveDir1D::Next, Wanted::Column(0))
            }
            ScreenMotion::LinePos(MovePosition::Middle) => (
                0,
                MoveDir1D::Next,
                Wanted::Column(self.geometry.columns().get() / 2),
            ),
            ScreenMotion::LinePos(MovePosition::End) => (count, MoveDir1D::Next, Wanted::End),
        };
        let (line, rows, row) = self.step(at.line, rows, row, direction, steps, text);
        let grapheme = grapheme_at(&rows[row], wanted);
        self.wanted = Some(match motion {
            ScreenMotion::Line(_) | ScreenMotion::LinePos(MovePosition::End) => wanted,
            _ => Wanted::Column(column_of(&rows[row], grapheme)),
        });

        Some(LogicalPosition { line, grapheme })
    }

    /// Walks `steps` display rows in `direction`, starting from the row `row` of the logical line
    /// `line`, whose rows are `rows`. Each logical line the walk crosses into is laid out as the
    /// walk reaches it, and a walk that runs out of text stops on the row it ran out on.
    ///
    /// # Type Parameters
    ///
    /// * `TextType` - The text the walk steps over.
    ///
    /// # Returns
    ///
    /// The logical line the walk ended on, that line's display rows, and the row within them.
    fn step<TextType: Text>(
        &self,
        line: usize,
        rows: Vec<DisplayRow>,
        row: usize,
        direction: MoveDir1D,
        steps: usize,
        text: &TextType,
    ) -> (usize, Vec<DisplayRow>, usize) {
        let last = text.line_count().saturating_sub(1);
        let mut line = line;
        let mut rows = rows;
        let mut row = row;
        let mut remaining = steps;

        while 0 < remaining {
            match direction {
                MoveDir1D::Next if row + 1 < rows.len() => row += 1,
                MoveDir1D::Next if line < last => {
                    line += 1;
                    rows = self.lay_out(line, text);
                    row = 0;
                }
                MoveDir1D::Previous if 0 < row => row -= 1,
                MoveDir1D::Previous if 0 < line => {
                    line -= 1;
                    rows = self.lay_out(line, text);
                    row = rows.len() - 1;
                }
                MoveDir1D::Next | MoveDir1D::Previous => break,
            }
            remaining -= 1;
        }

        (line, rows, row)
    }

    /// # Type Parameters
    ///
    /// * `TextType` - The text the line is read from.
    ///
    /// # Returns
    ///
    /// The display rows the logical line at `index` is drawn in, which are the rows of an empty
    /// line for an index the text does not hold.
    fn lay_out<TextType: Text>(&self, index: usize, text: &TextType) -> Vec<DisplayRow> {
        line::lay_out(
            index,
            &text.line(index).unwrap_or_default(),
            self.geometry.columns(),
            self.geometry.metrics(),
            self.geometry.options(),
        )
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

/// # Returns
///
/// Whether `action` is the characterwise motion to the end of a line, which is vim's `$`.
fn ends_a_line(action: &EditorAction) -> bool {
    matches!(
        action,
        EditorAction::Edit(
            _,
            EditTarget::Motion(MoveType::LinePos(MovePosition::End), _)
        )
    )
}

/// # Returns
///
/// The display column `row` draws the grapheme at `grapheme` at, which is the column just past the
/// row's text for a position past its last grapheme.
///
/// A tab is the one grapheme vim draws a cursor at the far end of: a cursor resting on one stands
/// in the last of the blanks the tab advances by rather than in the first, and that is the column
/// a screen motion leaving it is counted from.
fn column_of(row: &DisplayRow, grapheme: usize) -> usize {
    let columns = row.columns();
    let offset = grapheme.saturating_sub(row.start()).min(columns.len() - 1);
    if Some(TAB) == graphemes(row.text()).nth(offset) {
        return columns[offset + 1] - 1;
    }

    columns[offset]
}

/// # Returns
///
/// The grapheme of the logical line that `row` draws at `wanted`, which is the row's last grapheme
/// wherever the row is too short to reach it.
fn grapheme_at(row: &DisplayRow, wanted: Wanted) -> usize {
    let held = row.end() - row.start();
    if 0 == held {
        return row.start();
    }

    let offset = match wanted {
        Wanted::Column(column) => row.columns()[..held]
            .partition_point(|drawn| *drawn <= column)
            .saturating_sub(1),
        Wanted::End => held - 1,
        Wanted::FirstWord => first_word(row.text()).min(held - 1),
    };

    row.start() + offset
}

/// # Returns
///
/// The offset of the first grapheme of `text` that is not a blank, which is the offset past its
/// last grapheme where every one of them is.
fn first_word(text: &str) -> usize {
    let mut offset = 0;
    for grapheme in graphemes(text) {
        if !grapheme.chars().all(char::is_whitespace) {
            break;
        }
        offset += 1;
    }

    offset
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

    /// A text whose logical lines lay out into a different number of rows apiece, which is what
    /// makes a walk down its rows cross into a line at a row other than its first.
    const RAGGED: [&str; 3] = ["abcdefghijklmnopqrstuvwxyz", "short", "0123456789abcde"];

    /// A text held in memory as the lines it is written from.
    struct Lines(Vec<String>);

    impl Text for Lines {
        fn line_count(&self) -> usize {
            self.0.len()
        }

        fn line(&self, index: usize) -> Option<Cow<'_, str>> {
            self.0.get(index).map(|line| Cow::Borrowed(line.as_str()))
        }
    }

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
            1,
            LogicalPosition {
                line: 0,
                grapheme: 6,
            },
            &text(&[WIDE]),
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
            &text(&["abc"]),
        );

        assert_eq!(
            DisplayPosition { row: 0, column: 3 },
            shim.intercepted()[0].from
        );
    }

    #[test]
    fn a_screen_line_down_lands_on_the_grapheme_drawn_under_the_cursor() {
        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 8,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Next),
                1,
                LogicalPosition {
                    line: 0,
                    grapheme: 3,
                },
                &text(&[WIDE]),
            ),
            "a screen line down was answered in characters rather than in cells"
        );
    }

    #[test]
    fn a_walk_crosses_into_the_logical_line_below_at_its_first_row() {
        assert_eq!(
            Some(LogicalPosition {
                line: 1,
                grapheme: 3,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Next),
                3,
                LogicalPosition {
                    line: 0,
                    grapheme: 3,
                },
                &text(&RAGGED),
            )
        );
    }

    #[test]
    fn a_walk_that_runs_out_of_text_stops_on_the_row_it_ran_out_on() {
        assert_eq!(
            Some(LogicalPosition {
                line: 2,
                grapheme: 10,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Next),
                99,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &text(&RAGGED),
            )
        );
        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 0,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Previous),
                99,
                LogicalPosition {
                    line: 2,
                    grapheme: 0,
                },
                &text(&RAGGED),
            )
        );
    }

    #[test]
    fn a_chain_of_screen_lines_returns_to_the_column_it_left_from() {
        let mut shim = Shim::new(geometry());
        let at = LogicalPosition {
            line: 0,
            grapheme: 7,
        };
        let held = text(&RAGGED);
        let first = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 3, at, &held)
            .expect("a screen line down is answered");
        let second = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 1, first, &held)
            .expect("a screen line down is answered");

        assert_eq!(4, first.grapheme, "the short line reaches only its own end");
        assert_eq!(
            7, second.grapheme,
            "the chain forgot the column it left from"
        );
    }

    #[test]
    fn an_action_between_two_screen_motions_breaks_the_chain() {
        let mut shim = Shim::new(geometry());
        let at = LogicalPosition {
            line: 0,
            grapheme: 7,
        };
        let held = text(&RAGGED);
        let first = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 3, at, &held)
            .expect("a screen line down is answered");
        shim.note(&edit(EditTarget::Motion(
            MoveType::Column(MoveDir1D::Next, false),
            Count::Contextual,
        )));
        let second = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 1, first, &held)
            .expect("a screen line down is answered");

        assert_eq!(4, second.grapheme);
    }

    #[test]
    fn the_end_of_a_line_leaves_the_chain_wanting_the_end_of_a_row() {
        let mut shim = Shim::new(geometry());
        shim.note(&edit(EditTarget::Motion(
            MoveType::LinePos(MovePosition::End),
            Count::Contextual,
        )));
        let below = shim
            .intercept(
                ScreenMotion::Line(MoveDir1D::Next),
                3,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &text(&RAGGED),
            )
            .expect("a screen line down is answered");

        assert_eq!(
            LogicalPosition {
                line: 1,
                grapheme: 4,
            },
            below
        );
    }

    #[test]
    fn the_end_of_a_screen_line_sticks_to_the_end_of_every_row_the_chain_passes() {
        let mut shim = Shim::new(geometry());
        let at = LogicalPosition {
            line: 0,
            grapheme: 0,
        };
        let held = text(&RAGGED);
        let end = shim
            .intercept(ScreenMotion::LinePos(MovePosition::End), 0, at, &held)
            .expect("the end of a screen line is answered");
        let below = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 3, end, &held)
            .expect("a screen line down is answered");

        assert_eq!(9, end.grapheme, "the end of the first row was not reached");
        assert_eq!(
            LogicalPosition {
                line: 1,
                grapheme: 4,
            },
            below,
            "the chain did not stick to the end of the row it landed on"
        );
    }

    #[test]
    fn the_ends_of_a_screen_line_are_answered_against_the_row_the_cursor_is_on() {
        let indented = text(&["   ab cd"]);
        let at = LogicalPosition {
            line: 0,
            grapheme: 5,
        };

        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 0,
            }),
            answered(
                ScreenMotion::LinePos(MovePosition::Beginning),
                0,
                at,
                &indented,
            )
        );
        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 3,
            }),
            answered(ScreenMotion::FirstWord(MoveDir1D::Next), 0, at, &indented)
        );
        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 7,
            }),
            answered(ScreenMotion::LinePos(MovePosition::End), 0, at, &indented)
        );
        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 5,
            }),
            answered(
                ScreenMotion::LinePos(MovePosition::Middle),
                0,
                at,
                &indented
            )
        );
    }

    #[test]
    fn a_motion_counted_against_the_window_is_left_to_modalkit() {
        assert_eq!(
            None,
            answered(
                ScreenMotion::ViewportPos(MovePosition::Middle),
                1,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &text(&RAGGED),
            )
        );
    }

    /// # Returns
    ///
    /// Where a shim measuring in [`geometry`] answers `motion`, resolved to `count`, with the
    /// cursor standing at `at` in `held`.
    fn answered(
        motion: ScreenMotion,
        count: usize,
        at: LogicalPosition,
        held: &Lines,
    ) -> Option<LogicalPosition> {
        Shim::new(geometry()).intercept(motion, count, at, held)
    }

    /// # Returns
    ///
    /// A text written from the given lines.
    fn text(lines: &[&str]) -> Lines {
        Lines(lines.iter().map(|line| (*line).to_owned()).collect())
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
