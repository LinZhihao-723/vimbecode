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
//! What the shim answers is the motions counted against a line's own rows -- `gj` and `gk` a
//! number of screen lines away, `g0` and `g$` along the screen line the cursor is on, and `g^` at
//! its first word -- together with the motions counted against the window the text is scrolled
//! inside, which are `H`, `M` and `L`. A window with `'wrap'` unset draws one row of a line and
//! scrolls it sideways, so the row a motion is counted against there is the viewport's to say as
//! well; a line is laid out here as a wrapped line whatever the window does with it, and the
//! answer is right only for a window that wraps.
//!
//! `H`, `M` and `L` are counted against a window rather than against a text, so the seam has to be
//! handed the one the text is scrolled inside; a shim nobody has told answers them against a
//! window resting at the top of the text, which is where one starts. What each of them names is
//! counted in the display rows the window draws rather than in the lines it holds -- `M` is the
//! line halfway down the rows, not the line halfway down the lines -- and that is what makes them
//! the layout engine's: modalkit divides a character count by the window's width to guess at how
//! many rows a line takes, and reads a viewport this engine never wrote to.
//!
//! They are also the one family the seam answers in whole lines. Each lands on the first non-blank
//! of a line the window draws, which makes an operator over one linewise: `dH` takes every line
//! from the top of the window down to the cursor's own and fills a linewise register with them.
//!
//! `'scrolloff'` is part of that answer rather than a decoration on it. vim pulls a cursor away
//! from the edge of a window it would otherwise land against, counting the rows it pulls it over
//! rather than the lines, and it does so for a cursor move alone: an operator waiting on the
//! motion takes the line the window's edge names and no other. Both halves are measured here.
//!
//! Which motions the seam is for is a decision rather than an observation, so it is written down
//! here as one. Every motion modalkit can hand it is classified: one whose answer is counted in
//! cells and which the shim measures is intercepted, one whose answer is counted in cells and
//! which nothing here measures is out of scope, and one whose answer is the same number counted
//! either way is modalkit's. The middle bucket is refused rather than run. `gm`, `gM` and `|` all
//! land where display geometry says and modalkit answers all three by counting characters, and on
//! a line of CJK vim was measured landing somewhere else for each of them; a wrong cursor that
//! reports itself is worth more than a wrong cursor that does not, so those three fail rather than
//! land. `gm` is the shape `g0` and `g$` are, but half a window's width is not something the seam
//! is handed, so it stays refused with the other two.
//!
//! Two things the classification does not cover are named here so that they are not mistaken for
//! covered. A motion is classified by the place it names, which makes `j` and `k` characterwise:
//! the line they land on is a position in a text. The column they keep is not one -- vim carries a
//! screen column across a vertical motion where modalkit carries a character index -- and that is
//! a seam of its own, held to vim by a test that pins the divergence rather than left to be
//! rediscovered. And every motion the audit intercepts is answered here: the one shape that
//! reaches the seam's own match without a measurement behind it is a screen-line position naming
//! the middle of a row, which the audit refuses as `gm` before it is ever taken.
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
//! The exception is vim's own exception: a bare `j` or `k` is the one cursor move that leaves
//! `curswant` alone, so a chain that a `$` left wanting the end of a row reaches the screen motion
//! behind one still wanting it. Nothing else is carried across one. vim's `curswant` is a virtual
//! column of a whole logical line, and every other column a chain can be walking down is a display
//! column of the row it was walking down, which is a different number in the line a vertical
//! motion lands in; `$` is the one place the two agree, because the end of a row is the same place
//! in whatever line it is measured against. Carrying a number instead was measured against vim
//! and moved forty-eight cases off it that were on it.
//!
//! `H`, `M` and `L` forget that column as anything else does. vim sets `curswant` from each of
//! them, so a chain reaching past one would be walking down a column vim had already forgotten.
//!
//! What a motion is answered with is a place together with the three facts an operator applied
//! over it turns on and a cursor moved by it does not: whether what it takes is whole lines, which
//! is what separates `H` from every other motion here; whether the grapheme landed on is part of
//! what it takes, which is what separates `g$` from `gj`; and whether the walk travelled the whole
//! count it was asked for, because vim abandons an operator whose motion ran out of text. What is
//! done with the place is not the shim's -- it knows nothing about operators, marks or registers
//! -- but a place alone is not enough to decide an edit from, so the three are reported rather
//! than left to be inferred from where the answer landed.

use std::borrow::Cow;

use editor_types::prelude::{Count, EditTarget, MoveDir1D, MovePosition, MoveType};
use modalkit::actions::EditorAction;
use vbc_layout::line::{self, DisplayRow};
use vbc_layout::position::{DisplayPosition, LogicalPosition};
use vbc_layout::viewport::Viewport;
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

/// Where a screen motion goes, in the terms a cursor moved by it and an operator applied over it
/// are both decided from.
///
/// A cursor needs the place alone. An operator needs three more facts about the walk that reached
/// it: whether what it takes is whole lines, whether the grapheme landed on is part of what it
/// takes, and whether the walk travelled the whole count it was asked for, because vim abandons an
/// operator whose motion ran out of text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Landing {
    /// The place in the logical text the motion reached.
    pub at: LogicalPosition,

    /// Whether an operator applied over the motion takes the whole of every line between the
    /// cursor and the place reached. `H`, `M` and `L` are the motions it takes them over.
    pub linewise: bool,

    /// Whether the grapheme reached is part of what an operator applied over the motion takes.
    /// `g$` takes it and `gj`, `gk`, `g0` and `g^` stop in front of it. A linewise motion carries
    /// whole lines whatever this says.
    pub inclusive: bool,

    /// Whether the walk travelled the whole count the motion was asked for, which a walk that ran
    /// out of text did not.
    pub complete: bool,
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

    /// The last grapheme of whatever row the motion lands on, however long that row is, and
    /// whether an operator over a motion walking towards it takes the grapheme it stops on. vim's
    /// `$` leaves a chain wanting a column it holds no number for, which is what makes the motions
    /// after it inclusive; `g$` leaves it wanting the column it landed in, which is a column like
    /// any other and which the motions after it stop in front of.
    End {
        /// Whether a motion walking towards this end takes the grapheme it stops on.
        inclusive: bool,
    },

    /// The first grapheme of the landing row that is not a blank.
    FirstWord,
}

/// The lines a window draws, which is what a motion counted against the window is measured
/// against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Drawn {
    /// The line the window is anchored to, which `H` counts down from.
    top: usize,

    /// The last line the window draws every row of, which `L` counts back from.
    bottom: usize,

    /// The rows at the bottom of the window that no line is drawn in, which `M` measures its half
    /// against. A window taller than the rest of its text has them, and so does one whose next
    /// line takes more rows than it has left: vim fills those rows with the marker that says a
    /// line was left undrawn and counts them as empty either way.
    empty: usize,

    /// Whether the window draws the last line of the text, which is what says a cursor has no
    /// rows below it for `'scrolloff'` to want.
    ends: bool,
}

/// The seam itself: the layout the engine's screen motions are measured in, the window the text is
/// scrolled inside, the motions the seam has taken so far, and the column the chain of them is
/// walking down.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shim {
    geometry: Geometry,
    viewport: Viewport,
    intercepted: Vec<Intercepted>,
    wanted: Option<Wanted>,
}

impl Shim {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created shim measuring the motions it takes in `geometry`, against a window resting
    /// at the top of the text, having taken none.
    #[must_use]
    pub fn new(geometry: Geometry) -> Self {
        Self {
            geometry,
            viewport: Viewport::new(),
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

    /// Measures the motions counted against a window in `viewport`, which is where the text is
    /// scrolled to now.
    ///
    /// `H`, `M` and `L` name a line of the window rather than a line of the text, so a seam that
    /// has not been told where the window is answers them against the top of the text. Nothing
    /// else the shim measures reads the viewport.
    pub fn scrolled_to(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    /// # Returns
    ///
    /// The window the motions counted against one are measured against.
    #[must_use]
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// # Returns
    ///
    /// Every screen motion the shim has taken, oldest first.
    #[must_use]
    pub fn intercepted(&self) -> &[Intercepted] {
        &self.intercepted
    }

    /// Reads what an action this seam does not answer leaves a chain of screen motions wanting,
    /// where `bare` says the action is a motion with no operator applied to it.
    ///
    /// vim's own `$` wants the end of every row a screen motion after it lands on, which is the
    /// one column a chain can be walking down without a screen motion having started it. A bare
    /// `j` or `k` is the one cursor move vim leaves `curswant` alone across, and a chain wanting
    /// that end reaches the screen motion behind one still wanting it. Anything else that moves
    /// the cursor wants nothing at all, and the next screen motion is measured from wherever the
    /// cursor was left.
    ///
    /// What is carried across a vertical motion is that end alone. vim's `curswant` is a virtual
    /// column of a whole logical line, and everything else a chain can be walking down -- a
    /// numbered column, or the end a `g$` landed in -- is a display column of the row it was
    /// walking down, which is a different number in the line a vertical motion lands in. `$` is
    /// the one place the two agree, because the end of a row is the same place in whatever line it
    /// is measured against.
    ///
    /// An action that only records where the text has been moves no cursor and so ends no chain.
    /// modalkit files a history checkpoint after every key, and a chain such a checkpoint broke
    /// would be a chain no second motion could ever join.
    pub fn note(&mut self, action: &EditorAction, bare: bool) {
        if matches!(action, EditorAction::History(_)) {
            return;
        }
        if bare && Some(Wanted::End { inclusive: true }) == self.wanted && steps_a_line(action) {
            return;
        }

        self.wanted = (bare && ends_a_line(action)).then_some(Wanted::End { inclusive: true });
    }

    /// Takes the screen motion `motion`, resolved to the count `count`, with the cursor standing
    /// at `at` in `text`, and answers it from the layout engine. `bare` says the motion has no
    /// operator waiting on it, which is what decides whether `'scrolloff'` moves the answer.
    ///
    /// # Type Parameters
    ///
    /// * `TextType` - The text the motion is walked over.
    ///
    /// # Returns
    ///
    /// Where the motion goes, and [`None`] for a motion this seam does not answer, which the
    /// caller is then to leave to modalkit. A motion left to modalkit still ends the chain: it
    /// moves the cursor, and vim sets `curswant` from every cursor move but a bare `j` or `k`.
    pub fn intercept<TextType: Text>(
        &mut self,
        motion: ScreenMotion,
        count: usize,
        at: LogicalPosition,
        text: &TextType,
        bare: bool,
    ) -> Option<Landing> {
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
            ScreenMotion::ViewportPos(position) => {
                self.wanted = None;

                return Some(self.in_window(position, count, bare, text));
            }
            ScreenMotion::LinePos(MovePosition::Middle) => {
                self.wanted = None;

                return None;
            }
            ScreenMotion::FirstWord(direction) => (count, direction, Wanted::FirstWord),
            ScreenMotion::Line(direction) => (
                count,
                direction,
                self.wanted.unwrap_or(Wanted::Column(from.column)),
            ),
            ScreenMotion::LinePos(MovePosition::Beginning) => {
                (0, MoveDir1D::Next, Wanted::Column(0))
            }
            ScreenMotion::LinePos(MovePosition::End) => {
                (count, MoveDir1D::Next, Wanted::End { inclusive: false })
            }
        };
        let (line, rows, row, travelled) = self.step(at.line, rows, row, direction, steps, text);
        let grapheme = grapheme_at(&rows[row], wanted, self.geometry.columns().get());
        self.wanted = Some(match motion {
            ScreenMotion::Line(_) | ScreenMotion::LinePos(MovePosition::End) => wanted,
            _ => Wanted::Column(column_of(&rows[row], grapheme)),
        });

        Some(Landing {
            at: LogicalPosition { line, grapheme },
            linewise: false,
            inclusive: match motion {
                ScreenMotion::LinePos(MovePosition::End) => true,
                _ => matches!(wanted, Wanted::End { inclusive: true }),
            },
            complete: steps == travelled,
        })
    }

    /// Answers the motion counted against the window the text is scrolled inside that names
    /// `position`, asked for with the count `count`, where `bare` says no operator is waiting on
    /// it.
    ///
    /// # Type Parameters
    ///
    /// * `TextType` - The text the window draws.
    ///
    /// # Returns
    ///
    /// Where the motion goes, which is the first non-blank of a line the window draws.
    fn in_window<TextType: Text>(
        &self,
        position: MovePosition,
        count: usize,
        bare: bool,
        text: &TextType,
    ) -> Landing {
        let last = text.line_count().saturating_sub(1);
        let drawn = self.drawn(text);
        let named = match position {
            MovePosition::Beginning => (drawn.top + count.saturating_sub(1)).min(last),
            MovePosition::Middle => self.halfway(&drawn, text),
            MovePosition::End => drawn.bottom.saturating_sub(count.saturating_sub(1)),
        }
        .max(drawn.top);
        let line = if bare {
            self.corrected(named, &drawn, text)
        } else {
            named
        };

        Landing {
            at: LogicalPosition {
                line,
                grapheme: self.first_word_of(line, text),
            },
            linewise: true,
            inclusive: false,
            complete: true,
        }
    }

    /// # Type Parameters
    ///
    /// * `TextType` - The text the window draws.
    ///
    /// # Returns
    ///
    /// The lines the window draws of `text`, walked from the row the viewport is anchored to down
    /// to the bottom of the window.
    ///
    /// The line the window is anchored to is drawn whether or not the window has the rows for the
    /// whole of it, which is what makes the walk's first step unconditional; every line after it
    /// is drawn only where the rows it takes are all there, as vim draws one.
    fn drawn<TextType: Text>(&self, text: &TextType) -> Drawn {
        let last = text.line_count().saturating_sub(1);
        let top = self.viewport.anchor().min(last);
        let height = self.geometry.window().height().get();
        let mut bottom = top;
        let mut used = 0;
        loop {
            let rows = self.rows_of(bottom, top, text);
            if top < bottom && height < used + rows {
                bottom -= 1;

                break;
            }
            used += rows;
            if bottom == last || height <= used {
                break;
            }
            bottom += 1;
        }

        Drawn {
            top,
            bottom,
            empty: height.saturating_sub(used),
            ends: bottom == last,
        }
    }

    /// # Type Parameters
    ///
    /// * `TextType` - The text the window draws.
    ///
    /// # Returns
    ///
    /// The line halfway down the rows the window draws, which is vim's `M`.
    ///
    /// The walk runs over the lines of the text rather than over the lines the window draws, and
    /// steps back off a line whose rows the window had no room for, because that is the walk vim
    /// runs: a window whose second line is taller than the rows it has left answers `M` with its
    /// first line rather than with the line it could not draw.
    fn halfway<TextType: Text>(&self, drawn: &Drawn, text: &TextType) -> usize {
        let last = text.line_count().saturating_sub(1);
        let height = self.geometry.window().height().get();
        let half = height.saturating_sub(drawn.empty).div_ceil(2);
        let mut line = drawn.top;
        let mut used = 0;
        while line < last {
            if drawn.top < line && half <= used {
                break;
            }
            used += self.rows_of(line, drawn.top, text);
            if half <= used {
                break;
            }
            line += 1;
        }
        if drawn.top < line && height < used {
            line -= 1;
        }

        line
    }

    /// # Type Parameters
    ///
    /// * `TextType` - The text the window draws.
    ///
    /// # Returns
    ///
    /// The line `'scrolloff'` leaves a cursor sent to `line` on, which is `line` itself wherever
    /// the window already draws enough rows on both sides of it.
    ///
    /// The rows wanted above and below are taken from the window's own ends: a window drawing the
    /// first line of the text wants none above it, one drawing the last wants none below, and each
    /// of those two caps what the other side may ask for. What is then counted off each end of the
    /// window is display rows rather than lines, one line at a time from whichever end is further
    /// from what it wants, until both ends have what they asked for or the two meet.
    fn corrected<TextType: Text>(&self, line: usize, drawn: &Drawn, text: &TextType) -> usize {
        let height = self.geometry.window().height().get();
        let mut above_wanted = self.geometry.window().scrolloff();
        let mut below_wanted = above_wanted;
        if 0 == drawn.top {
            above_wanted = 0;
            below_wanted = below_wanted.min(height / 2);
        }
        if drawn.ends {
            below_wanted = 0;
            above_wanted = above_wanted.min((height - 1) / 2);
        }
        if drawn.top + above_wanted <= line && line + below_wanted <= drawn.bottom {
            return line;
        }

        let mut top = drawn.top;
        let mut bottom = drawn.bottom;
        let mut above = 0;
        let mut below = 0;
        while (above < above_wanted || below < below_wanted) && top < bottom {
            if below < below_wanted && (below <= above || above_wanted <= above) {
                below += self.rows_of(bottom, drawn.top, text);
                bottom -= 1;
            }
            if above < above_wanted && (above < below || below_wanted <= below) {
                above += self.rows_of(top, drawn.top, text);
                top += 1;
            }
        }
        if bottom <= top {
            return top.min(bottom);
        }
        if line < top && 0 < drawn.top {
            return top;
        }
        if bottom < line && !drawn.ends {
            return bottom;
        }

        line
    }

    /// # Type Parameters
    ///
    /// * `TextType` - The text the window draws.
    ///
    /// # Returns
    ///
    /// The display rows the window draws the logical line at `index` in, which is the rows the
    /// line lays out into less the rows of `top` the viewport is scrolled past.
    fn rows_of<TextType: Text>(&self, index: usize, top: usize, text: &TextType) -> usize {
        let rows = self.lay_out(index, text).len();
        if index != top {
            return rows;
        }

        rows.saturating_sub(self.viewport.vertical_offset()).max(1)
    }

    /// # Type Parameters
    ///
    /// * `TextType` - The text the line is read from.
    ///
    /// # Returns
    ///
    /// The first grapheme of the logical line at `index` that is not a blank, which is that line's
    /// last grapheme where every one of them is and its first where it holds none at all.
    fn first_word_of<TextType: Text>(&self, index: usize, text: &TextType) -> usize {
        let line = text.line(index).unwrap_or_default();
        let held = graphemes(&line).count();

        first_word(&line).min(held.saturating_sub(1))
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
    /// The logical line the walk ended on, that line's display rows, the row within them, and the
    /// display rows the walk travelled, which is fewer than `steps` where the text ran out.
    fn step<TextType: Text>(
        &self,
        line: usize,
        rows: Vec<DisplayRow>,
        row: usize,
        direction: MoveDir1D,
        steps: usize,
        text: &TextType,
    ) -> (usize, Vec<DisplayRow>, usize, usize) {
        let last = text.line_count().saturating_sub(1);
        let mut line = line;
        let mut rows = rows;
        let mut row = row;
        let mut travelled = 0;

        while travelled < steps {
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
            travelled += 1;
        }

        (line, rows, row, travelled)
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
/// Whether `action` is one of the motions counted in whole logical lines, which is vim's `j` and
/// `k` and is what it leaves `curswant` alone across.
fn steps_a_line(action: &EditorAction) -> bool {
    matches!(
        action,
        EditorAction::Edit(_, EditTarget::Motion(MoveType::Line(_), _))
    )
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
/// The grapheme of the logical line that `row`, drawn in a window `width` columns wide, draws at
/// `wanted`, which is the row's last grapheme wherever the row is too short to reach it.
///
/// A grapheme wider than the cell a column names is one the column can land in the middle of, and
/// vim steps back off one it landed in the second half of a row: a tab reaching across the end of
/// a row would otherwise carry the cursor a row further along than the motion asked for. Only the
/// far half of a row is stepped back from, which is where vim steps back.
///
/// The half is a half of the row's own text rather than of the window, because a continuation row
/// draws its decoration in front of that text and both the column carried onto the row and the
/// columns the row is measured in count those cells. Measuring against the window instead moves
/// the mark by half the decoration, which is why an undecorated window cannot tell the two apart.
fn grapheme_at(row: &DisplayRow, wanted: Wanted, width: usize) -> usize {
    let held = row.end() - row.start();
    if 0 == held {
        return row.start();
    }

    let offset = match wanted {
        Wanted::Column(column) => {
            let offset = row.columns()[..held]
                .partition_point(|drawn| *drawn <= column)
                .saturating_sub(1);
            let grapheme = row.start() + offset;
            let decoration = row.columns()[0];
            let beside = width.saturating_sub(decoration);
            if 0 < grapheme
                && column < column_of(row, grapheme)
                && beside / 2 < column.saturating_sub(decoration)
            {
                return grapheme - 1;
            }

            offset
        }
        Wanted::End { .. } => held - 1,
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

    /// A text whose second line draws a tab across the cells four to seven, which is a grapheme a
    /// column carried down from the line above can land in the middle of.
    const STRADDLED: [&str; 2] = ["0123456789", "abcd\tefgh"];

    /// A text longer than the window below draws, whose third line takes three of its five rows
    /// and whose last line is indented, so a window over it holds fewer lines than rows and a
    /// motion counted against it lands past the first column.
    const SCROLLED: [&str; 7] = [
        "one",
        "two",
        "abcdefghijklmnopqrstuvwxyz",
        "four",
        "five",
        "six",
        "  seven",
    ];

    /// A text whose fourth line takes more rows than the window below has left for it, so the
    /// window draws three of its five rows and leaves the other two to the marker that says a line
    /// was too tall to draw.
    const BRIMMING: [&str; 6] = [
        "one",
        "two",
        "three",
        "abcdefghijklmnopqrstuvwxyz0123",
        "five",
        "six",
    ];

    /// The place the motions counted against a window are taken from, which they ignore: each of
    /// them names a line of the window rather than a line a step away from the cursor.
    const TOP: LogicalPosition = LogicalPosition {
        line: 0,
        grapheme: 0,
    };

    /// The columns the decorated measurements below are made in. Half of the window and half of a
    /// decorated row's own text are different columns there, which is what makes the two rules a
    /// row can be stepped back from tell each other apart.
    const DECORATED_COLUMNS: usize = 15;

    /// The continuation marker the decorated measurements below are drawn with.
    const SHOW_BREAK: &str = "> ";

    /// A line of tabs, every row of which ends part-way through one. Drawn behind a continuation
    /// marker, a column carried down it lands in the middle of a tab on a row whose text starts
    /// two cells in.
    const TABS: [&str; 1] = ["a\tb\tc\td\te\tf\tg\th\ti\tj\tk\tl"];

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
            &text(&[WIDE]),
            true,
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
            true,
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
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 3, at, &held, true)
            .expect("a screen line down is answered")
            .at;
        let second = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 1, first, &held, true)
            .expect("a screen line down is answered")
            .at;

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
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 3, at, &held, true)
            .expect("a screen line down is answered")
            .at;
        shim.note(
            &edit(EditTarget::Motion(
                MoveType::Column(MoveDir1D::Next, false),
                Count::Contextual,
            )),
            true,
        );
        let second = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 1, first, &held, true)
            .expect("a screen line down is answered")
            .at;

        assert_eq!(4, second.grapheme);
    }

    #[test]
    fn a_bare_vertical_motion_carries_a_chain_wanting_an_end_across_itself() {
        let mut shim = Shim::new(geometry());
        let held = text(&RAGGED);
        shim.note(&ended_a_line(), true);
        let first = shim
            .intercept(
                ScreenMotion::Line(MoveDir1D::Next),
                3,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &held,
                true,
            )
            .expect("a screen line down is answered")
            .at;
        shim.note(&stepped_down(), true);
        let second = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 1, first, &held, true)
            .expect("a screen line down is answered")
            .at;

        assert_eq!(
            LogicalPosition {
                line: 2,
                grapheme: 9,
            },
            second,
            "a bare `j` between the two ended a chain wanting an end, which vim's own `curswant` \
             outlives"
        );
    }

    #[test]
    fn a_bare_vertical_motion_ends_a_chain_wanting_a_numbered_column() {
        let mut shim = Shim::new(geometry());
        let held = text(&RAGGED);
        let first = shim
            .intercept(
                ScreenMotion::Line(MoveDir1D::Next),
                3,
                LogicalPosition {
                    line: 0,
                    grapheme: 7,
                },
                &held,
                true,
            )
            .expect("a screen line down is answered")
            .at;
        shim.note(&stepped_down(), true);
        let second = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 1, first, &held, true)
            .expect("a screen line down is answered")
            .at;

        assert_eq!(4, second.grapheme);
    }

    #[test]
    fn an_operator_over_a_vertical_motion_breaks_a_chain_wanting_an_end() {
        let mut shim = Shim::new(geometry());
        let held = text(&RAGGED);
        shim.note(&ended_a_line(), true);
        let first = shim
            .intercept(
                ScreenMotion::Line(MoveDir1D::Next),
                3,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &held,
                true,
            )
            .expect("a screen line down is answered")
            .at;
        shim.note(&stepped_down(), false);
        let second = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 1, first, &held, true)
            .expect("a screen line down is answered")
            .at;

        assert_eq!(
            LogicalPosition {
                line: 2,
                grapheme: 4,
            },
            second
        );
    }

    #[test]
    fn an_operator_over_an_end_of_line_leaves_the_chain_wanting_nothing() {
        let mut shim = Shim::new(geometry());
        shim.note(&ended_a_line(), false);
        let below = shim
            .intercept(
                ScreenMotion::Line(MoveDir1D::Next),
                3,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &text(&RAGGED),
                true,
            )
            .expect("a screen line down is answered")
            .at;

        assert_eq!(
            LogicalPosition {
                line: 1,
                grapheme: 0,
            },
            below
        );
    }

    #[test]
    fn a_motion_counted_against_the_window_ends_the_chain() {
        let mut shim = Shim::new(geometry());
        let held = text(&RAGGED);
        shim.note(&ended_a_line(), true);
        let at = LogicalPosition {
            line: 0,
            grapheme: 0,
        };

        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 0,
            }),
            shim.intercept(
                ScreenMotion::ViewportPos(MovePosition::Beginning),
                1,
                at,
                &held,
                true,
            )
            .map(|landing| landing.at)
        );

        let below = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 3, at, &held, true)
            .expect("a screen line down is answered")
            .at;

        assert_eq!(
            LogicalPosition {
                line: 1,
                grapheme: 0,
            },
            below,
            "an `H` between the two carried a chain vim's own `curswant` does not outlive"
        );
    }

    #[test]
    fn the_end_of_a_line_leaves_the_chain_wanting_the_end_of_a_row() {
        let mut shim = Shim::new(geometry());
        shim.note(&ended_a_line(), true);
        let below = shim
            .intercept(
                ScreenMotion::Line(MoveDir1D::Next),
                3,
                LogicalPosition {
                    line: 0,
                    grapheme: 0,
                },
                &text(&RAGGED),
                true,
            )
            .expect("a screen line down is answered")
            .at;

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
            .intercept(ScreenMotion::LinePos(MovePosition::End), 0, at, &held, true)
            .expect("the end of a screen line is answered")
            .at;
        let below = shim
            .intercept(ScreenMotion::Line(MoveDir1D::Next), 3, end, &held, true)
            .expect("a screen line down is answered")
            .at;

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
    }

    #[test]
    fn the_end_of_a_screen_line_takes_the_grapheme_it_lands_on_and_the_others_do_not() {
        let at = LogicalPosition {
            line: 0,
            grapheme: 0,
        };
        let held = text(&RAGGED);

        for (motion, inclusive) in [
            (ScreenMotion::LinePos(MovePosition::End), true),
            (ScreenMotion::LinePos(MovePosition::Beginning), false),
            (ScreenMotion::Line(MoveDir1D::Next), false),
            (ScreenMotion::FirstWord(MoveDir1D::Next), false),
        ] {
            assert_eq!(
                inclusive,
                landed(motion, 1, at, &held)
                    .expect("the motion is answered")
                    .inclusive,
                "`{motion:?}` reports the wrong grapheme as the one an operator takes"
            );
        }
    }

    #[test]
    fn a_walk_that_ran_out_of_text_reports_that_it_did_not_travel_its_count() {
        let held = text(&RAGGED);
        let at = LogicalPosition {
            line: 0,
            grapheme: 0,
        };

        assert!(
            landed(ScreenMotion::Line(MoveDir1D::Next), 3, at, &held)
                .expect("a screen line down is answered")
                .complete
        );
        assert!(
            !landed(ScreenMotion::Line(MoveDir1D::Next), 99, at, &held)
                .expect("a screen line down is answered")
                .complete
        );
        assert!(
            !landed(ScreenMotion::Line(MoveDir1D::Previous), 1, at, &held)
                .expect("a screen line up is answered")
                .complete
        );
        assert!(
            !landed(ScreenMotion::LinePos(MovePosition::End), 99, at, &held)
                .expect("the end of a screen line is answered")
                .complete
        );
    }

    #[test]
    fn a_column_landing_in_the_far_half_of_a_row_steps_back_off_the_grapheme_it_split() {
        let held = text(&STRADDLED);

        assert_eq!(
            Some(LogicalPosition {
                line: 1,
                grapheme: 3,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Next),
                1,
                LogicalPosition {
                    line: 0,
                    grapheme: 6,
                },
                &held,
            ),
            "a column carried into the middle of a tab was carried a row further along than the \
             motion asked for"
        );
        assert_eq!(
            Some(LogicalPosition {
                line: 1,
                grapheme: 4,
            }),
            answered(
                ScreenMotion::Line(MoveDir1D::Next),
                1,
                LogicalPosition {
                    line: 0,
                    grapheme: 4,
                },
                &held,
            ),
            "a column landing in the near half of a row was stepped back from, which vim does not"
        );
    }

    #[test]
    fn the_half_a_row_is_stepped_back_from_is_a_half_of_its_own_text() {
        let held = text(&TABS);

        assert_eq!(
            Some(LogicalPosition {
                line: 0,
                grapheme: 9,
            }),
            Shim::new(decorated_geometry())
                .intercept(
                    ScreenMotion::Line(MoveDir1D::Next),
                    1,
                    LogicalPosition {
                        line: 0,
                        grapheme: 5,
                    },
                    &held,
                    true,
                )
                .map(|landing| landing.at),
            "a column in the near half of a decorated row's own text was stepped back from, which \
             it is only in the near half of the window the row is drawn in"
        );
    }

    #[test]
    fn a_motion_counted_against_the_window_names_a_line_the_window_draws() {
        let held = text(&SCROLLED);

        assert_eq!(
            [Some(0), Some(2), Some(2)],
            [
                MovePosition::Beginning,
                MovePosition::Middle,
                MovePosition::End,
            ]
            .map(|position| in_window(position, 1, &held, true).map(|at| at.line)),
            "the window draws three of its five rows out of one line, so the line halfway down its \
             rows is the last line it draws rather than the middle of the three"
        );
    }

    #[test]
    fn a_motion_counted_against_a_scrolled_window_counts_from_where_the_window_is() {
        let held = text(&SCROLLED);
        let mut shim = Shim::new(geometry());
        shim.scrolled_to(Viewport::anchored_at(1, 0));

        assert_eq!(
            [Some(1), Some(2), Some(3)],
            [
                MovePosition::Beginning,
                MovePosition::Middle,
                MovePosition::End,
            ]
            .map(|position| shim
                .intercept(ScreenMotion::ViewportPos(position), 1, TOP, &held, true)
                .map(|landing| landing.at.line))
        );
    }

    #[test]
    fn a_window_scrolled_part_way_down_a_line_draws_the_rows_it_has_left_of_it() {
        let held = text(&SCROLLED);
        let mut shim = Shim::new(geometry());
        shim.scrolled_to(Viewport::anchored_at(2, 2));

        assert_eq!(
            Some(6),
            shim.intercept(
                ScreenMotion::ViewportPos(MovePosition::End),
                1,
                TOP,
                &held,
                true,
            )
            .map(|landing| landing.at.line),
            "the window drew the last row of the wrapped line alone, so it reached four lines \
             further down the text than a window anchored to the whole of that line does"
        );
    }

    #[test]
    fn the_rows_a_line_the_window_could_not_draw_leaves_over_are_rows_no_line_is_drawn_in() {
        let held = text(&BRIMMING);

        assert_eq!(
            Some(1),
            in_window(MovePosition::Middle, 1, &held, true).map(|at| at.line),
            "the window drew three of its five rows and left the other two to the marker that \
             says a line was too tall to draw, so the half `M` is counted against is a half of \
             the three rather than of the five"
        );
    }

    #[test]
    fn a_count_walks_down_from_the_top_of_the_window_and_up_from_its_bottom() {
        let held = text(&SCROLLED);

        assert_eq!(
            [Some(1), Some(2), Some(1), Some(0)],
            [
                (MovePosition::Beginning, 2),
                (MovePosition::Beginning, 3),
                (MovePosition::End, 2),
                (MovePosition::End, 3),
            ]
            .map(|(position, count)| in_window(position, count, &held, true).map(|at| at.line)),
            "a count larger than the lines the window draws is clamped to its far end"
        );
    }

    #[test]
    fn a_motion_counted_against_the_window_lands_on_the_first_word_of_its_line() {
        let held = text(&SCROLLED);
        let mut shim = Shim::new(geometry());
        shim.scrolled_to(Viewport::anchored_at(6, 0));

        assert_eq!(
            Some(LogicalPosition {
                line: 6,
                grapheme: 2,
            }),
            shim.intercept(
                ScreenMotion::ViewportPos(MovePosition::Beginning),
                1,
                TOP,
                &held,
                true,
            )
            .map(|landing| landing.at)
        );
    }

    #[test]
    fn an_operator_over_a_motion_counted_against_the_window_takes_whole_lines() {
        let held = text(&SCROLLED);

        assert_eq!(
            Some(Landing {
                at: LogicalPosition {
                    line: 2,
                    grapheme: 0,
                },
                linewise: true,
                inclusive: false,
                complete: true,
            }),
            Shim::new(geometry()).intercept(
                ScreenMotion::ViewportPos(MovePosition::End),
                1,
                TOP,
                &held,
                false,
            )
        );
    }

    #[test]
    fn scrolloff_pulls_a_cursor_off_the_top_of_the_window_and_leaves_an_operator_alone() {
        let held = text(&SCROLLED);
        let mut shim = Shim::new(geometry().with_scrolloff(1));
        shim.scrolled_to(Viewport::anchored_at(1, 0));
        let landed = |bare| {
            shim.clone()
                .intercept(
                    ScreenMotion::ViewportPos(MovePosition::Beginning),
                    1,
                    TOP,
                    &held,
                    bare,
                )
                .map(|landing| landing.at.line)
        };

        assert_eq!(
            (Some(2), Some(1)),
            (landed(true), landed(false)),
            "an operator waiting on the motion takes the line the window's own edge names"
        );
    }

    #[test]
    fn scrolloff_wants_no_rows_above_a_window_drawing_the_first_line_of_the_text() {
        let held = text(&SCROLLED);

        assert_eq!(
            Some(0),
            Shim::new(geometry().with_scrolloff(1))
                .intercept(
                    ScreenMotion::ViewportPos(MovePosition::Beginning),
                    1,
                    TOP,
                    &held,
                    true,
                )
                .map(|landing| landing.at.line),
            "a window at the top of the text has no rows above it to keep the cursor off"
        );
    }

    /// # Returns
    ///
    /// Where a shim measuring in [`geometry`], against a window at the top of the text, answers
    /// the motion counted against that window naming `position`, asked for with the count `count`
    /// over `held`, where `bare` says no operator is waiting on it.
    fn in_window(
        position: MovePosition,
        count: usize,
        held: &Lines,
        bare: bool,
    ) -> Option<LogicalPosition> {
        Shim::new(geometry())
            .intercept(ScreenMotion::ViewportPos(position), count, TOP, held, bare)
            .map(|landing| landing.at)
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
        landed(motion, count, at, held).map(|landing| landing.at)
    }

    /// # Returns
    ///
    /// What a shim measuring in [`geometry`] makes of `motion`, resolved to `count`, with the
    /// cursor standing at `at` in `held`.
    fn landed(
        motion: ScreenMotion,
        count: usize,
        at: LogicalPosition,
        held: &Lines,
    ) -> Option<Landing> {
        Shim::new(geometry()).intercept(motion, count, at, held, true)
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
    /// The layout the decorated measurements above are made in.
    fn decorated_geometry() -> Geometry {
        Geometry::new(
            NonZeroUsize::new(DECORATED_COLUMNS).expect("the columns are not zero"),
            NonZeroUsize::new(ROWS).expect("the rows are not zero"),
        )
        .with_options(line::Options::new().with_show_break(SHOW_BREAK.to_owned()))
    }

    /// # Returns
    ///
    /// The action modalkit produces for a motion over `target` with no operator applied to it.
    fn edit(target: EditTarget) -> EditorAction {
        EditorAction::Edit(Specifier::Contextual, target)
    }

    /// # Returns
    ///
    /// The action modalkit produces for vim's `j`.
    fn stepped_down() -> EditorAction {
        edit(EditTarget::Motion(
            MoveType::Line(MoveDir1D::Next),
            Count::Contextual,
        ))
    }

    /// # Returns
    ///
    /// The action modalkit produces for vim's `$`.
    fn ended_a_line() -> EditorAction {
        edit(EditTarget::Motion(
            MoveType::LinePos(MovePosition::End),
            Count::Contextual,
        ))
    }
}
