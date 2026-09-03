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
//! What the shim measures costs the rows a motion crosses and nothing else. A motion counted in
//! screen lines can leave the line it started on, so the text reaches the shim as lines to ask for
//! rather than as a document to walk, and the walk lays out one logical line for each it steps
//! into. What that costs is the count the motion was typed with; what it never costs is the text
//! the count is counted inside, and nothing laid out is kept.
//!
//! A motion's answer is a place in the logical text together with the two things an operator
//! applied over it needs and a cursor moved by it does not: whether the grapheme landed on is part
//! of what the operator takes, which is what separates `g$` from `gj`, and whether the motion
//! travelled the whole count it was asked for, because vim abandons an operator whose motion ran
//! out of text and moves the cursor anyway.
//!
//! Where a motion is drawn is the shim's; what is done with the place it names is not. Turning an
//! answer into an edit is the engine's, and the shim knows nothing about operators, marks or
//! registers.
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
//! rediscovered. And an intercepted motion whose answer the shim does not hold is still answered
//! by modalkit, which is `H`, `M` and `L`: where they land is measured against a viewport this
//! shim is not the owner of.

use editor_types::prelude::{Count, EditTarget, MoveDir1D, MovePosition, MoveType};
use modalkit::actions::EditorAction;
use vbc_layout::line::{self, DisplayRow};
use vbc_layout::position::{DisplayPosition, LogicalPosition};
use vbc_layout::width::graphemes;

use crate::screen::{row_index, Geometry};

/// The logical lines a screen motion is measured against.
///
/// A motion counted in screen lines can leave the line it started on, so the shim needs more of
/// the text than the cursor's own line. It never needs the text as a whole, which is why the lines
/// are asked for one at a time rather than handed over together.
pub trait Text {
    /// # Returns
    ///
    /// The number of logical lines the text holds.
    fn lines(&self) -> usize;

    /// # Parameters
    ///
    /// * `line` - The zero-based index of the logical line to read.
    ///
    /// # Returns
    ///
    /// The text of the line without its line ending, and `None` past the last line.
    fn line(&self, line: usize) -> Option<String>;
}

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

/// Where a screen motion lands, in the terms both a cursor moved by it and an operator applied
/// over it are decided from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Landing {
    /// The place in the logical text the motion reached.
    pub at: LogicalPosition,

    /// Whether the grapheme reached is part of what an operator applied over the motion takes.
    /// `g$` takes it and `gj`, `gk`, `g0` and `g^` stop in front of it.
    pub inclusive: bool,

    /// Whether the motion travelled the whole count it was asked for. A motion that ran out of
    /// text moves the cursor as far as it reached and leaves an operator applied over it undone.
    pub complete: bool,
}

/// One screen motion the shim took, where the layout engine says the cursor stood when it arrived,
/// and where it says the motion goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

    /// Where the motion goes, and `None` for a motion the shim does not answer, which is left to
    /// modalkit exactly as it was before the seam existed.
    pub to: Option<Landing>,
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
    /// at `at` in `text`, measures where the layout engine draws that cursor, and answers the
    /// motion from that measurement.
    ///
    /// # Returns
    ///
    /// Where the motion goes, and `None` for a motion whose answer the shim does not hold, which
    /// is one measured against a viewport the shim is not the owner of, and `None` past the last
    /// line of the text.
    pub fn answer<TextType: Text>(
        &mut self,
        motion: ScreenMotion,
        count: usize,
        at: LogicalPosition,
        text: &TextType,
    ) -> Option<Landing> {
        let rows = self.rows(at.line, text)?;
        let index = row_index(&rows, at.grapheme);
        let from = DisplayPosition {
            row: index,
            column: column_of(&rows[index], at.grapheme),
        };
        let wanted = wanted_column(&rows[index], at.grapheme);
        let to = self.landed(motion, count, index, wanted, rows, text);
        self.intercepted.push(Intercepted {
            motion,
            count,
            from,
            to,
        });

        to
    }

    /// # Returns
    ///
    /// Where `motion` goes from the row `row` of `rows`, with the cursor standing at the display
    /// column `column`, and `None` where the shim does not answer the motion.
    fn landed<TextType: Text>(
        &self,
        motion: ScreenMotion,
        count: usize,
        row: usize,
        column: usize,
        rows: Vec<DisplayRow>,
        text: &TextType,
    ) -> Option<Landing> {
        match motion {
            ScreenMotion::FirstWord(_) => {
                Some(reached(&rows[row], first_word(&rows[row]), false, true))
            }
            ScreenMotion::Line(direction) => {
                let (walked, at, travelled) = self.walked(direction, count, row, rows, text)?;
                let landed = &walked[at];
                let width = self.geometry.columns().get();

                Some(reached(
                    landed,
                    stepped_back(landed, grapheme_at(landed, column), column, width),
                    false,
                    count == travelled,
                ))
            }
            ScreenMotion::LinePos(MovePosition::Beginning) => {
                Some(reached(&rows[row], rows[row].start(), false, true))
            }
            ScreenMotion::LinePos(MovePosition::End) => {
                let (walked, at, travelled) =
                    self.walked(MoveDir1D::Next, count, row, rows, text)?;
                let landed = &walked[at];

                Some(reached(
                    landed,
                    last_grapheme(landed),
                    true,
                    count == travelled,
                ))
            }
            ScreenMotion::LinePos(MovePosition::Middle) | ScreenMotion::ViewportPos(_) => None,
        }
    }

    /// Walks `count` display rows in the direction `direction`, from the row `row` of the laid-out
    /// logical line `rows`, laying out one further logical line for each the walk steps into and
    /// stopping at the edge of the text.
    ///
    /// # Returns
    ///
    /// The rows of the logical line the walk ended on, the row within them it ended at, and the
    /// display rows it travelled, which is fewer than `count` where the text ran out.
    fn walked<TextType: Text>(
        &self,
        direction: MoveDir1D,
        count: usize,
        row: usize,
        rows: Vec<DisplayRow>,
        text: &TextType,
    ) -> Option<(Vec<DisplayRow>, usize, usize)> {
        let mut walked = rows;
        let mut at = row;
        let mut travelled = 0;
        while travelled < count {
            match direction {
                MoveDir1D::Next if at + 1 < walked.len() => at += 1,
                MoveDir1D::Next if walked[at].line() + 1 < text.lines() => {
                    walked = self.rows(walked[at].line() + 1, text)?;
                    at = 0;
                }
                MoveDir1D::Previous if 0 < at => at -= 1,
                MoveDir1D::Previous if 0 < walked[at].line() => {
                    walked = self.rows(walked[at].line() - 1, text)?;
                    at = walked.len() - 1;
                }
                MoveDir1D::Next | MoveDir1D::Previous => break,
            }
            travelled += 1;
        }

        Some((walked, at, travelled))
    }

    /// # Returns
    ///
    /// The display rows the logical line `line` of `text` is drawn as, and `None` past the last
    /// line of the text.
    fn rows<TextType: Text>(&self, line: usize, text: &TextType) -> Option<Vec<DisplayRow>> {
        let held = text.line(line)?;

        Some(line::lay_out(
            line,
            &held,
            self.geometry.columns(),
            self.geometry.metrics(),
            self.geometry.options(),
        ))
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

/// # Returns
///
/// The landing on the grapheme `grapheme` of the logical line `row` draws a slice of.
fn reached(row: &DisplayRow, grapheme: usize, inclusive: bool, complete: bool) -> Landing {
    Landing {
        at: LogicalPosition {
            line: row.line(),
            grapheme,
        },
        inclusive,
        complete,
    }
}

/// # Returns
///
/// The display column the grapheme `grapheme` of `row`'s logical line is drawn at, which is the
/// column past the row's text where the grapheme is past its end.
fn column_of(row: &DisplayRow, grapheme: usize) -> usize {
    row.columns()[offset_of(row, grapheme)]
}

/// # Returns
///
/// The display column a motion counted in screen lines carries down from the grapheme `grapheme`
/// of `row`'s logical line.
///
/// That is the cell the grapheme is drawn from, except on a tab, which vim draws the cursor at the
/// last of the blanks it advances by rather than at the first. A motion is carried by the column
/// the cursor is drawn in, so a `gj` typed on a tab arrives a tab's width further along than one
/// typed on the character in front of it.
fn wanted_column(row: &DisplayRow, grapheme: usize) -> usize {
    let columns = row.columns();
    let offset = offset_of(row, grapheme);
    let tabbed = graphemes(row.text()).nth(offset) == Some("\t");

    match columns.get(offset + 1) {
        Some(next) if tabbed => next - 1,
        _ => columns[offset],
    }
}

/// # Returns
///
/// The offset within `row` of the grapheme `grapheme` of the row's logical line, which is the
/// offset of the column past the row's text where the grapheme is not one the row draws.
fn offset_of(row: &DisplayRow, grapheme: usize) -> usize {
    grapheme
        .saturating_sub(row.start())
        .min(row.columns().len() - 1)
}

/// # Returns
///
/// The grapheme of `row`'s logical line drawn at the display column `column`, which is the row's
/// first grapheme where the column falls in front of its text and its last where the column falls
/// past it, as vim's own cursor is drawn.
fn grapheme_at(row: &DisplayRow, column: usize) -> usize {
    let drawn = &row.columns()[..row.columns().len() - 1];
    let offset = drawn.partition_point(|at| *at <= column);

    row.start() + offset.saturating_sub(1)
}

/// # Returns
///
/// The grapheme a screen line's worth of motion rests on, having landed on the grapheme `grapheme`
/// of `row` while carrying the display column `column` down a row `width` columns wide.
///
/// vim steps back off a grapheme it landed in the middle of, so that a tab drawn across the second
/// half of a row carries the cursor no further along than the row it was asked for. A grapheme
/// whose cursor is drawn at the column the motion carried, which is every grapheme but a tab, is
/// one the motion rests on as it stands.
fn stepped_back(row: &DisplayRow, grapheme: usize, column: usize, width: usize) -> usize {
    let split = column < wanted_column(row, grapheme) && width / 2 < column;

    if split && 0 < grapheme {
        grapheme - 1
    } else {
        grapheme
    }
}

/// # Returns
///
/// The last grapheme of `row`'s logical line the row draws, which is the row's own start where the
/// row draws nothing.
fn last_grapheme(row: &DisplayRow) -> usize {
    row.end().saturating_sub(1).max(row.start())
}

/// # Returns
///
/// The first grapheme of `row`'s logical line the row draws that is not blank, and the row's last
/// grapheme where every one of them is.
fn first_word(row: &DisplayRow) -> usize {
    let offset = graphemes(row.text())
        .position(|grapheme| !grapheme.chars().all(char::is_whitespace))
        .map(|offset| row.start() + offset);

    offset.unwrap_or_else(|| last_grapheme(row))
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

    /// A line of tabs, on which a row the motions below reach is filled to its end by a tab and a
    /// motion carried onto that row lands in the middle of one.
    const TABBED: &str = "a\tb\tc\td\te\tf\tg\th\ti";

    /// The columns [`TABBED`] is measured in, in which its tabs fill a row rather than starting
    /// one.
    const TABBED_COLUMNS: usize = 20;

    /// The lines a measurement is made against.
    struct Held(Vec<String>);

    impl Text for Held {
        fn lines(&self) -> usize {
            self.0.len()
        }

        fn line(&self, line: usize) -> Option<String> {
            self.0.get(line).cloned()
        }
    }

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
        shim.answer(
            ScreenMotion::Line(MoveDir1D::Next),
            1,
            LogicalPosition {
                line: 0,
                grapheme: 6,
            },
            &held(&[WIDE]),
        );

        assert_eq!(
            [Intercepted {
                motion: ScreenMotion::Line(MoveDir1D::Next),
                count: 1,
                from: DisplayPosition { row: 1, column: 2 },
                to: Some(Landing {
                    at: LogicalPosition {
                        line: 0,
                        grapheme: 6,
                    },
                    inclusive: false,
                    complete: false,
                }),
            }],
            shim.intercepted()
        );
    }

    #[test]
    fn a_cursor_past_the_last_grapheme_of_a_line_is_measured_at_the_column_past_it() {
        let mut shim = Shim::new(geometry());
        shim.answer(
            ScreenMotion::LinePos(MovePosition::End),
            1,
            LogicalPosition {
                line: 0,
                grapheme: 5,
            },
            &held(&["abc"]),
        );

        assert_eq!(
            DisplayPosition { row: 0, column: 3 },
            shim.intercepted()[0].from
        );
    }

    #[test]
    fn a_screen_line_of_motion_carries_the_display_column_onto_the_row_it_reaches() {
        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 7,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Next),
                1,
                LogicalPosition {
                    line: 0,
                    grapheme: 2,
                },
                &[WIDE],
            )
            .map(|landing| landing.at),
            "a column of cells was carried down as though it were a column of characters"
        );
    }

    #[test]
    fn a_screen_line_of_motion_walks_out_of_the_line_it_started_on() {
        assert_eq!(
            Some(LogicalPosition {
                line: 1,
                grapheme: 7,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Next),
                2,
                LogicalPosition {
                    line: 0,
                    grapheme: 4,
                },
                &[WIDE, "abcdefgh"],
            )
            .map(|landing| landing.at)
        );
    }

    #[test]
    fn a_motion_that_runs_out_of_text_reports_the_row_it_reached() {
        let landing = answered(
            ScreenMotion::Line(MoveDir1D::Next),
            9,
            LogicalPosition {
                line: 0,
                grapheme: 0,
            },
            &[WIDE, "abc"],
        )
        .expect("a screen line of motion is answered");

        assert_eq!(
            LogicalPosition {
                line: 1,
                grapheme: 0,
            },
            landing.at
        );
        assert!(
            !landing.complete,
            "a motion that ran out of text reported travelling the whole count"
        );
    }

    #[test]
    fn the_end_of_a_screen_line_takes_the_grapheme_it_stops_on() {
        let landing = answered(
            ScreenMotion::LinePos(MovePosition::End),
            0,
            LogicalPosition {
                line: 0,
                grapheme: 0,
            },
            &[WIDE],
        )
        .expect("the end of a screen line is answered");

        assert_eq!(
            LogicalPosition {
                line: 0,
                grapheme: 4,
            },
            landing.at
        );
        assert!(
            landing.inclusive,
            "`g$` stopped in front of the grapheme it lands on"
        );
    }

    #[test]
    fn a_motion_carried_into_the_middle_of_a_tab_rests_in_front_of_it() {
        let columns = NonZeroUsize::new(TABBED_COLUMNS).expect("the columns are not zero");
        let rows = NonZeroUsize::new(ROWS).expect("the rows are not zero");
        let mut shim = Shim::new(Geometry::new(columns, rows));

        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 8,
            }),
            shim.answer(
                ScreenMotion::Line(MoveDir1D::Next),
                1,
                LogicalPosition {
                    line: 0,
                    grapheme: 3,
                },
                &held(&[TABBED]),
            )
            .map(|landing| landing.at),
            "a motion landing in the second half of a tab was carried a row further along"
        );
    }

    #[test]
    fn a_motion_measured_against_a_viewport_is_left_to_modalkit() {
        assert_eq!(
            None,
            answered(
                ScreenMotion::ViewportPos(MovePosition::Middle),
                1,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &[WIDE],
            )
        );
    }

    /// # Returns
    ///
    /// Where the screen motion `motion`, resolved to the count `count`, goes from `at` in a text
    /// whose lines are `lines`.
    fn answered(
        motion: ScreenMotion,
        count: usize,
        at: LogicalPosition,
        lines: &[&str],
    ) -> Option<Landing> {
        Shim::new(geometry()).answer(motion, count, at, &held(lines))
    }

    /// # Returns
    ///
    /// The lines `lines` as a text a screen motion is measured against.
    fn held(lines: &[&str]) -> Held {
        Held(lines.iter().map(|line| (*line).to_owned()).collect())
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
