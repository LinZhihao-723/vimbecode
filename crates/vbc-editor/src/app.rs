//! The editor as a program: the text it holds, the vim engine that edits it, the window it is
//! scrolled inside, and the frame it paints into a terminal.
//!
//! An application is where the pieces meet. It owns the [`Engine`] the keys are typed at, the one
//! [`Buffer`] the engine's text is laid out from, the [`Viewport`] that says which part of it is
//! on screen, and the cursor's logical position; a frame is the four of them turned into cells.
//! Nothing about that turning consults the anchor mapping: the rows come from [`Screen`], already
//! laid out and already carrying the row the cursor is drawn on, and the drawing spends itself on
//! the cells it fills.
//!
//! A keystroke reaches the engine first and the application's own keys are what the engine bound
//! none of. That order is what keeps one dispatch rather than two: a key that carries a sequence
//! further -- the motion an operator is waiting for, a character typed in insert mode -- is the
//! engine's, and only a key the table answers with nothing is offered to the window to scroll or
//! to the program to stop. A key neither of them answers is said rather than swallowed, because a
//! keystroke that vanishes is the hardest fault in an editor to notice.
//!
//! The engine is the authority on the text, the cursor, the mode and the registers; the viewport
//! is the authority on what is drawn. The two are reconciled after every keystroke, in both
//! directions: what the engine did to the text and the cursor is read back, and the window follows
//! the cursor by the fewest rows that draw it. A scroll carries the cursor the other way, so what
//! the scroll left is written back into the engine and the next keystroke edits where the cursor
//! is drawn rather than where it was before the window moved.
//!
//! What that reconciliation costs is the window rather than the text. The engine holds its text as
//! a rope and the layout reads it as one string per line, so reading the text back lays out every
//! line of the file; the engine therefore says whether the keystroke could have changed the text at
//! all, and a motion, a scroll, a selection and a search skip that work entirely. What is read back
//! after every keystroke is the cursor and the selection, each of which costs the line it stands
//! on. A frame costs the window it draws, which is the property the anchor-relative layout was
//! built for, so a keystroke that moves the cursor costs the same over a hundred lines and over a
//! hundred thousand, which `keystroke_cost.rs` measures rather than argues.
//!
//! The application is a program a reader can leave, so it can be written to and searched. `:`
//! opens the ex command line and `/` a search, and while either of them is open every key typed
//! belongs to that line rather than to the engine, because the `w` of `:wq` is not a word motion.
//! `:w` writes the file the text was read from, `:q` refuses to leave a text nothing has written,
//! `:q!` leaves it anyway and `:wq` writes and leaves. A search is over the literal bytes typed at
//! it -- this editor has no regular expressions and does not pretend to -- and `n` and `N` repeat
//! it the way it ran and the other way.
//!
//! What is selected is drawn, in either of the two views: the range `v`, `V` and `CTRL-V` are
//! moving over the file, and the range `viac` took out of a block of the transcript. The painting
//! is laid over the cells the rows were already drawn in rather than folded into the styles they
//! were drawn with, which is what lets a selection cross a wrap boundary without the layout being
//! told anything about it. A panel nothing can be written to is a panel whose whole point is what
//! it selects, so a selection nobody can see there is a feature nobody can use.
//!
//! The application draws two things and gives the keys to one of them at a time. `<C-T>` moves
//! between the file being edited and the transcript of what was said, and it is read ahead of
//! everything else because a panel reachable only from normal mode is a panel insert mode hides.
//! While the transcript has the keys they go to its own panel, which reads them through the same
//! table with the transcript's own sequences bound in it and refuses every one that would write.
//! Both of them follow their cursor, and for the same reason: a `j` past the bottom row moves a
//! cursor nobody can see. What the panel's own following costs is the rows it walks over rather
//! than the transcript it walks through, so a step over a closed fold costs one row however many
//! lines that fold hides.
//!
//! Those two are two engines and one register file. Each of them has a text, a cursor and a mode
//! of its own, and neither has registers of its own, because what a reader takes out of the
//! transcript they mean to put into the file: an application that let each engine keep a file of
//! its own would answer `yac` in the panel and `p` in the file with a yank into a drawer nothing
//! opens, which is the gesture this editor exists for going nowhere. So the file is built once,
//! where the engine is, and every panel the application builds afterwards is handed it.
//!
//! The window is measured from the area a frame is drawn into rather than stored, so a terminal
//! that was resized between two frames draws the second one at its new size without being told,
//! and the engine is laid out in that same window so that a display motion is measured in the
//! terminal it was typed at. The gutter takes its columns off the left of that area and the text
//! wraps into what is left, so a wider gutter narrows the text rather than pushing it off the
//! screen.

use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyModifiers};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use ratatui::Frame;
use vbc_layout::buffer::Buffer;
use vbc_layout::line::{DisplayRow, Options};
use vbc_layout::position::LogicalPosition;
use vbc_layout::viewport::{Command, Viewport};
use vbc_layout::width::{grapheme_indices, graphemes, Metrics};

use crate::chat::block::RenderedRow;
use crate::chat::fold::Position as Placed;
use crate::chat::object::Position as Resting;
use crate::chat::policy::{Drawn, Panel, Selected, REFUSAL};
use crate::chat::selection::Source as Selectable;
use crate::chat::transcript::Transcript;
use crate::engine::{self, Engine, Position as Caret, Shape};
use crate::event::{Event, KeyEvent};
use crate::gutter::{Gutter, Options as GutterOptions};
use crate::render::{cursor_cell, paint, painted_columns, Renderer};
use crate::screen::{self, Error, Geometry, Screen};
use crate::style::StyledRow;

/// What the status line says in each of the modes vim names in it, which is nothing at all in
/// normal mode because vim says nothing there either.
const INSERTING: &str = "-- INSERT --";
const SELECTING: &str = "-- SELECT --";
const VISUAL: &str = "-- VISUAL --";

/// What the status line says while the transcript panel has the keys.
const READING: &str = "-- TRANSCRIPT --";

/// How the cells a selection covers are drawn, which is what vim's `Visual` highlight is by
/// default: the colours the text was already drawn in, swapped.
pub const SELECTION: Style = Style::new().add_modifier(Modifier::REVERSED);

/// The keys a line is typed at the status line by: the ex command line, and the two directions a
/// search is started in.
const COMMAND: char = ':';
const FORWARD: char = '/';
const BACKWARD: char = '?';

/// What the status line says about a command line that could not do what it asked for.
const UNNAMED: &str = "no file name";
const UNWRITTEN: &str = "no write since the last change (add `!` to override)";
const UNSEARCHED: &str = "there is no search to repeat";

/// How many rows the transcript panel is walked over looking for the row its cursor is on before a
/// follow gives up and leaves the panel where it stands.
const FOLLOWED: usize = 256;

/// What a line typed at the status line asks for once it is entered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Asked {
    /// An ex command: `w`, `q`, `wq` and the `!` that overrides them.
    Command,

    /// A search for a literal, forwards where the flag is set and backwards where it is not.
    Search(bool),
}

/// A line being typed at the status line, which holds the keys typed at it rather than handing
/// them to the engine.
///
/// The key that opened the line is the first character of it, so what the status line says is the
/// line itself and a reader sees the `:` or the `/` they typed where vim puts it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Prompt {
    asked: Asked,
    line: String,
}

impl Prompt {
    /// # Returns
    ///
    /// A newly opened line asking for `asked`, holding nothing but the key `opened` that opened
    /// it.
    fn new(asked: Asked, opened: char) -> Self {
        Self {
            asked,
            line: opened.to_string(),
        }
    }

    /// # Returns
    ///
    /// What was typed after the key that opened the line.
    fn typed(&self) -> &str {
        let mut characters = self.line.chars();
        characters.next();

        characters.as_str()
    }
}

/// What a keystroke left the application asking for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The application goes on reading keys.
    Continues,

    /// The application was asked to stop.
    Stops,
}

/// An editor: the text being edited, the engine editing it, the part of it the window shows, and
/// where the cursor rests.
///
/// The gutter numbers lines by default, as vim with `'number'` set, because a wrapped transcript
/// is unreadable without the blanks that say which rows continue a line. The status line is not
/// drawn by default, as vim with `'laststatus'` at zero, because an application drawn into an area
/// of somebody else's choosing has no row to spare unless it was given one.
pub struct App {
    engine: Engine,
    text: Buffer,
    viewport: Viewport,
    cursor: LogicalPosition,
    metrics: Metrics,
    options: Options,
    gutter: GutterOptions,
    scrolloff: usize,
    status: bool,
    notice: Option<String>,
    panel: Panel,
    focus: Focus,
    top: Placed,
    revision: u64,
    path: Option<PathBuf>,
    saved: String,
    prompt: Option<Prompt>,
    pattern: Option<String>,
    forward: bool,
    selection: Option<(Caret, Caret, Shape)>,
    held: Option<Selected>,
}

/// Which of the two things the application draws the keys are typed at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    /// The file being edited.
    Text,

    /// The transcript of what was said, which is read rather than written.
    Transcript,
}

impl App {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created application showing `text` from its first row, with the cursor on its first
    /// grapheme and a vim engine over it.
    #[must_use]
    pub fn new(text: Buffer) -> Self {
        let engine = Engine::new(&written(&text));
        let panel = Panel::new(Transcript::new()).sharing(engine.register_file().clone());
        let mut app = Self {
            engine,
            text,
            viewport: Viewport::new(),
            cursor: LogicalPosition {
                line: 0,
                grapheme: 0,
            },
            metrics: Metrics::default(),
            options: Options::new(),
            gutter: GutterOptions::new().with_number(true),
            scrolloff: 0,
            status: false,
            notice: None,
            panel,
            focus: Focus::Text,
            top: Placed::new(0, 0),
            revision: u64::MAX,
            path: None,
            saved: String::new(),
            prompt: None,
            pattern: None,
            forward: true,
            selection: None,
            held: None,
        };
        app.adopt();
        app.saved = app.written();

        app
    }

    /// # Returns
    ///
    /// This application over the file named `path`, which is the file `:w` writes back.
    ///
    /// The read is the write's own inverse, so it takes one line ending off the bytes it read
    /// rather than every one of them: the write puts exactly one back, and a read that stripped
    /// them all would let a file whose last lines are empty lose those lines to a `:w` that
    /// changed nothing.
    ///
    /// # Errors
    ///
    /// Forwards [`std::fs::read_to_string`]'s return values on failure.
    pub fn opened(path: PathBuf) -> std::io::Result<Self> {
        let read = std::fs::read_to_string(&path)?;
        let text = read.strip_suffix('\n').unwrap_or(&read);

        Ok(Self::new(Buffer::from_text(text)).with_path(path))
    }

    /// # Returns
    ///
    /// This application editing the file named `path`, which is the file `:w` writes and `:q`
    /// refuses to leave unwritten.
    #[must_use]
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);

        self
    }

    /// # Returns
    ///
    /// This application showing `transcript` in the panel `<C-T>` reaches.
    #[must_use]
    pub fn with_transcript(mut self, transcript: Transcript) -> Self {
        self.panel = Panel::new(transcript).sharing(self.engine.register_file().clone());
        self.top = Placed::new(0, 0);

        self
    }

    /// # Returns
    ///
    /// This application measuring its text under `metrics`.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// # Returns
    ///
    /// This application wrapping its text as `options` says.
    #[must_use]
    pub fn with_options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// # Returns
    ///
    /// This application drawing the gutter `gutter` describes.
    #[must_use]
    pub fn with_gutter(mut self, gutter: GutterOptions) -> Self {
        self.gutter = gutter;
        self
    }

    /// # Returns
    ///
    /// This application keeping `rows` rows between the cursor and an edge, as vim's `'scrolloff'`.
    #[must_use]
    pub fn with_scrolloff(mut self, rows: usize) -> Self {
        self.scrolloff = rows;
        self
    }

    /// # Returns
    ///
    /// This application drawing a status line along the bottom row of its area, or leaving that
    /// row to the text, as vim's `'laststatus'`.
    #[must_use]
    pub fn with_status(mut self, status: bool) -> Self {
        self.status = status;
        self
    }

    /// # Returns
    ///
    /// The text being edited.
    #[must_use]
    pub fn text(&self) -> &Buffer {
        &self.text
    }

    /// # Returns
    ///
    /// The file the text is written to, and [`None`] where the application was given no file to
    /// write to.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// # Returns
    ///
    /// Whether the text holds something other than what was last read or written, which is what
    /// `:q` refuses to leave behind.
    ///
    /// What this costs is the text rather than the keystroke, because it is asked once at the end
    /// of a session rather than after every key: the bytes the editor would write are built and
    /// compared against the bytes it last wrote, so a change and its undo leave the file unmodified
    /// exactly as vim does.
    #[must_use]
    pub fn modified(&self) -> bool {
        self.written() != self.saved
    }

    /// # Returns
    ///
    /// The part of the text the window shows.
    #[must_use]
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// # Returns
    ///
    /// Where the cursor rests in the text.
    #[must_use]
    pub fn cursor(&self) -> LogicalPosition {
        self.cursor
    }

    /// # Returns
    ///
    /// The mode the keys typed so far left the editor in.
    #[must_use]
    pub fn mode(&self) -> VimMode {
        self.engine.mode()
    }

    /// # Returns
    ///
    /// Which of the two things the application draws the keys are typed at.
    #[must_use]
    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// # Returns
    ///
    /// The transcript panel the keys reach while it has the focus.
    pub fn panel(&mut self) -> &mut Panel {
        &mut self.panel
    }

    /// # Returns
    ///
    /// What the last keystroke could not do, and [`None`] where it did what it asked for.
    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// # Returns
    ///
    /// What the status line says: the line being typed at it, what the last keystroke could not do,
    /// or the mode the editor is in, which is nothing at all in normal mode.
    #[must_use]
    pub fn status(&self) -> &str {
        if let Some(prompt) = &self.prompt {
            return &prompt.line;
        }
        if let Some(notice) = &self.notice {
            return notice;
        }
        if Focus::Transcript == self.focus {
            return READING;
        }

        match self.mode() {
            VimMode::Insert => INSERTING,
            VimMode::Select => SELECTING,
            VimMode::Visual => VISUAL,
            _ => "",
        }
    }

    /// Measures the window an area draws, which is the area's rows less the status line's, and the
    /// columns the gutter leaves the text.
    ///
    /// # Returns
    ///
    /// The geometry a frame drawn into `area` is laid out to, or [`None`] where the area is too
    /// small to draw a column of text or a row of one in.
    #[must_use]
    pub fn geometry(&self, area: Rect) -> Option<Geometry> {
        let text = self.split(area).0;
        let columns = usize::from(text.width).checked_sub(self.gutter_columns())?;

        Some(
            Geometry::new(
                NonZeroUsize::new(columns)?,
                NonZeroUsize::new(usize::from(text.height))?,
            )
            .with_metrics(self.metrics)
            .with_options(self.options.clone())
            .with_scrolloff(self.scrolloff),
        )
    }

    /// Draws one frame: the gutter down the left of the area, the rows of text beside it, the
    /// status line along the bottom where the application was given one, and nothing at all where
    /// the area is too small to hold either.
    ///
    /// # Returns
    ///
    /// The cell of `area` a terminal should rest the cursor in, or [`None`] where the frame does
    /// not draw the cursor's own row.
    pub fn draw(&self, cells: &mut Cells, area: Rect) -> Option<Position> {
        let (body, status) = self.split(area);
        self.draw_status(cells, status);
        let drawn = if Focus::Transcript == self.focus {
            self.draw_panel(cells, body)
        } else {
            self.draw_text(cells, area, body)
        };
        if self.prompt.is_some() {
            return self.prompt_cell(status);
        }

        drawn
    }

    /// Draws the file being edited: the gutter, the rows of text beside it, and the selection
    /// painted over the cells those rows were drawn in.
    ///
    /// # Returns
    ///
    /// The cell of `area` a terminal should rest the cursor in, or [`None`] where the frame does
    /// not draw the cursor's own row.
    fn draw_text(&self, cells: &mut Cells, area: Rect, body: Rect) -> Option<Position> {
        let geometry = self.geometry(area)?;
        let screen = Screen::of(&self.text, &self.viewport, self.cursor, &geometry);
        let gutter = Rect {
            width: narrowed(self.gutter_columns()).min(body.width),
            ..body
        };
        let text = Rect {
            x: body.x + gutter.width,
            width: body.width - gutter.width,
            ..body
        };

        Gutter::new(
            &self.gutter,
            screen.rows(),
            self.cursor.line,
            self.text.line_count(),
        )
        .render(gutter, cells);

        let renderer = Renderer::new(self.metrics);
        let mut top = 0;
        for rows in screen.lines() {
            let drawn = renderer.draw_line(cells, text, top, rows);
            self.paint_line(cells, text, top, &rows[..usize::from(drawn)]);
            top += drawn;
        }
        blank(cells, text, top);

        let row = screen.cursor_row()?;
        cursor_cell(
            text,
            narrowed(row),
            screen.rows().get(row)?,
            self.cursor.grapheme,
        )
    }

    /// Paints the selection over the rows one logical line was drawn in.
    ///
    /// What is painted is worked out once for the logical line and intersected with each of its
    /// rows, so a selection reaching across a wrap boundary paints its part of every row it
    /// reaches and the layout is never asked about the selection at all.
    fn paint_line(&self, cells: &mut Cells, area: Rect, top: u16, rows: &[DisplayRow]) {
        let Some(first) = rows.first() else {
            return;
        };
        let Some(covered) = self.covered(first.line()) else {
            return;
        };
        for (index, row) in rows.iter().enumerate() {
            let Some(columns) = painted_columns(row, &covered) else {
                continue;
            };
            paint(cells, area, top + narrowed(index), &columns, SELECTION);
        }
    }

    /// # Returns
    ///
    /// The graphemes of the logical line `line` the selection covers, or [`None`] where it covers
    /// none of them.
    ///
    /// A blockwise selection is cut out of the line by the virtual columns it takes, measured on
    /// the unwrapped logical line as vim measures them, so a line drawn in three rows is cut at
    /// the same columns as one drawn in one.
    fn covered(&self, line: usize) -> Option<Range<usize>> {
        let (first, last, shape) = self.span()?;
        if line < first.line || last.line < line {
            return None;
        }
        let text = self.text.line(line).unwrap_or_default();
        let count = graphemes(text).count();

        match shape {
            Shape::Linewise => Some(0..count),
            Shape::Charwise => {
                let start = if line == first.line {
                    first.grapheme
                } else {
                    0
                };
                let end = if line == last.line {
                    count.min(last.grapheme + 1)
                } else {
                    count
                };

                Some(start.min(end)..end)
            }
            Shape::Blockwise => {
                let one = self.column_span(first);
                let other = self.column_span(last);
                let window = one.start.min(other.start)..one.end.max(other.end);

                Some(self.cut(text, &window))
            }
        }
    }

    /// # Returns
    ///
    /// The two ends of what is drawn as selected, nearer end first, and the shape it takes, or
    /// [`None`] where nothing is selected.
    ///
    /// What is selected is what the engine says is selected rather than what the mode suggests: a
    /// selection is a range of the text, and a range the keys left resting is a range whether or
    /// not the mode is still the one that made it.
    fn span(&self) -> Option<(LogicalPosition, LogicalPosition, Shape)> {
        let (moving, started, shape) = self.selection?;
        let one = self.placed(moving);
        let other = self.placed(started);

        Some((one.min(other), one.max(other), shape))
    }

    /// # Returns
    ///
    /// Where `at` rests, counted in the graphemes the screen draws rather than in the bytes the
    /// engine counts a column in.
    fn placed(&self, at: Caret) -> LogicalPosition {
        LogicalPosition {
            line: at.line,
            grapheme: grapheme_at(self.text.line(at.line).unwrap_or_default(), at.column),
        }
    }

    /// # Returns
    ///
    /// The virtual columns the grapheme at `at` occupies on the unwrapped logical line it sits in,
    /// which is one column wide past the end of that line.
    fn column_span(&self, at: LogicalPosition) -> Range<usize> {
        let line = self.text.line(at.line).unwrap_or_default();
        let mut column = 0;
        for (index, grapheme) in graphemes(line).enumerate() {
            let width = self.metrics.grapheme_width(grapheme, column).max(1);
            if index == at.grapheme {
                return column..column + width;
            }
            column += width;
        }

        column..column + 1
    }

    /// # Returns
    ///
    /// The graphemes of `line` a blockwise selection taking the virtual columns `window` covers,
    /// which is empty on a line that is not drawn as far as those columns.
    fn cut(&self, line: &str, window: &Range<usize>) -> Range<usize> {
        let mut column = 0;
        let mut first = None;
        let mut last = 0;
        for (index, grapheme) in graphemes(line).enumerate() {
            let width = self.metrics.grapheme_width(grapheme, column).max(1);
            if column < window.end && window.start < column + width {
                first.get_or_insert(index);
                last = index + 1;
            }
            column += width;
        }
        let first = first.unwrap_or(0);

        first..last.max(first)
    }

    /// # Returns
    ///
    /// The cell of the status line the cursor rests in while a line is being typed at it, which is
    /// the cell after the last one that line is drawn in, or [`None`] where the application draws
    /// no status line.
    fn prompt_cell(&self, status: Rect) -> Option<Position> {
        let prompt = self.prompt.as_ref()?;
        if status.is_empty() {
            return None;
        }
        let mut column = 0;
        for grapheme in graphemes(&prompt.line) {
            column += self.metrics.grapheme_width(grapheme, column);
        }

        Some(Position {
            x: status.x + narrowed(column).min(status.width - 1),
            y: status.y,
        })
    }

    /// Draws one frame into a terminal's own frame, leaving the cursor where the frame draws it.
    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if let Some(position) = self.draw(frame.buffer_mut(), area) {
            frame.set_cursor_position(position);
        }
    }

    /// Types one key at the editor, running everything it asks for.
    ///
    /// The engine reads the key first, and the application's own keys -- the scrolls, and the `q`
    /// that ends the program -- are the ones it bound nothing to, so a key that carries a sequence
    /// further belongs to the sequence rather than to the window. Nor are they read in an
    /// inserting mode, where every key is either text or a key vim answers itself. The interrupt
    /// is the one key read ahead of the engine, because a program that can only be stopped from
    /// normal mode is a program insert mode traps a terminal in.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    pub fn press(&mut self, area: Rect, key: KeyEvent) -> Outcome {
        self.notice = None;
        if interrupts(key) {
            return Outcome::Stops;
        }
        if self.prompt.is_some() {
            return self.typing(area, key);
        }
        if transcribes(key) {
            self.focus = match self.focus {
                Focus::Text => Focus::Transcript,
                Focus::Transcript => Focus::Text,
            };
            if Focus::Transcript == self.focus {
                self.held = self.panel.selection();
                self.follow_panel(area);
            }

            return Outcome::Continues;
        }
        if Focus::Transcript == self.focus {
            return self.read(area, key);
        }
        self.dispatch(area, |engine| engine.press(key));

        let unbound = self
            .engine
            .unbound()
            .map(|keys| (spelled(keys), keys.len()));
        let Some((keys, typed)) = unbound else {
            self.follow(area);

            return Outcome::Continues;
        };
        if 1 == typed && VimMode::Insert != self.mode() {
            if let Some((asked, opened)) = opened_by(key) {
                self.prompt = Some(Prompt::new(asked, opened));

                return Outcome::Continues;
            }
            if let Some(again) = repeats(key) {
                self.seek(area, again);

                return Outcome::Continues;
            }
            if quits(key) {
                return self.stop(false);
            }
            if let Some(command) = scrolled_by(key) {
                if let Err(error) = self.scroll(area, command) {
                    self.notice = Some(error.to_string());
                }

                return Outcome::Continues;
            }
        }
        self.notice = Some(format!("`{keys}` is bound to nothing"));

        Outcome::Continues
    }

    /// Types one key at the line being typed at the status line.
    ///
    /// The line holds every key typed at it, so nothing typed into a command or a search reaches
    /// the engine: the `w` of `:wq` is not a word motion. The interrupt is the one key read ahead
    /// of it, as it is read ahead of everything else.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    fn typing(&mut self, area: Rect, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Backspace => {
                if let Some(prompt) = self.prompt.as_mut() {
                    prompt.line.pop();
                    if prompt.line.is_empty() {
                        self.prompt = None;
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(prompt) = self.prompt.take() {
                    return self.entered(area, &prompt);
                }
            }
            KeyCode::Char(character) if types(key) => {
                if let Some(prompt) = self.prompt.as_mut() {
                    prompt.line.push(character);
                }
            }
            _ => {}
        }

        Outcome::Continues
    }

    /// Runs the line that was typed at the status line.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    fn entered(&mut self, area: Rect, prompt: &Prompt) -> Outcome {
        match prompt.asked {
            Asked::Command => self.run(prompt.typed().trim()),
            Asked::Search(forward) => {
                let pattern = prompt.typed();
                if !pattern.is_empty() {
                    self.pattern = Some(pattern.to_owned());
                }
                self.forward = forward;
                self.seek(area, true);

                Outcome::Continues
            }
        }
    }

    /// Runs one ex command, which is `w`, `q`, `wq` and the `!` that overrides what they refuse.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    fn run(&mut self, command: &str) -> Outcome {
        let (asked, rest) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let named = rest.trim();
        let (asked, forced) = asked
            .strip_suffix('!')
            .map_or((asked, false), |asked| (asked, true));

        match asked {
            "" => Outcome::Continues,
            "w" | "write" => {
                self.write(named);

                Outcome::Continues
            }
            "q" | "quit" => self.stop(forced),
            "wq" | "x" | "xit" => {
                if self.write(named) {
                    return self.stop(true);
                }

                Outcome::Continues
            }
            asked => {
                self.notice = Some(format!("`{asked}` is not an editor command"));

                Outcome::Continues
            }
        }
    }

    /// Writes the text to the file it was read from, or to `named` where a command named one.
    ///
    /// A write to a file of somebody else's name leaves the text modified, as vim's does: what was
    /// written elsewhere is not the file the reader is editing, and calling it written would let
    /// the next `:q` throw the work away.
    ///
    /// # Returns
    ///
    /// Whether the file was written.
    fn write(&mut self, named: &str) -> bool {
        let path = if named.is_empty() {
            self.path.clone()
        } else {
            Some(PathBuf::from(named))
        };
        let Some(path) = path else {
            self.notice = Some(UNNAMED.to_owned());

            return false;
        };
        let written = self.written();
        if let Err(error) = std::fs::write(&path, &written) {
            self.notice = Some(format!("{}: {error}", path.display()));

            return false;
        }
        if Some(path.as_path()) == self.path.as_deref() {
            self.saved = written;
        }
        self.notice = Some(format!("{} written", path.display()));

        true
    }

    /// # Returns
    ///
    /// Whether the application goes on reading keys, which it does where the text holds something
    /// nothing has written and the command did not insist.
    fn stop(&mut self, forced: bool) -> Outcome {
        if !forced && self.modified() {
            self.notice = Some(UNWRITTEN.to_owned());

            return Outcome::Continues;
        }

        Outcome::Stops
    }

    /// Carries the cursor to the next place the last pattern typed at the status line is found,
    /// searching the way the search was started where `onward` is set and the other way where it
    /// is not.
    ///
    /// The search wraps around the end of the text as vim's does, so a pattern the text holds is
    /// found wherever the cursor was left rather than only below it.
    ///
    /// # Returns
    ///
    /// Whether the pattern was found.
    fn seek(&mut self, area: Rect, onward: bool) -> bool {
        let Some(pattern) = self.pattern.clone() else {
            self.notice = Some(UNSEARCHED.to_owned());

            return false;
        };
        let Some(found) = self.found(&pattern, onward == self.forward) else {
            self.notice = Some(format!("`{pattern}` not found"));

            return false;
        };
        self.cursor = found;
        self.engine.place(found);
        self.selection = self.engine.selection();
        self.follow(area);

        true
    }

    /// # Returns
    ///
    /// Where `pattern` is next found in the text, from the cursor onwards where `forward` is set
    /// and from the cursor backwards where it is not, wrapping around the end of the text, or
    /// [`None`] where the text holds it nowhere.
    ///
    /// The pattern is matched as the literal bytes it holds rather than as a regular expression,
    /// which is what this editor's search is and all it claims to be.
    fn found(&self, pattern: &str, forward: bool) -> Option<LogicalPosition> {
        let lines = self.text.lines();
        let count = lines.len();
        if 0 == count {
            return None;
        }
        let held = lines.get(self.cursor.line)?;
        let at: usize = graphemes(held)
            .take(self.cursor.grapheme)
            .map(str::len)
            .sum();

        for step in 0..=count {
            let index = if forward {
                (self.cursor.line + step) % count
            } else {
                (self.cursor.line + count - step % count) % count
            };
            let line = lines.get(index)?;
            let found = if forward {
                let from = if 0 == step {
                    (at + line.get(at..)?.chars().next().map_or(0, char::len_utf8)).min(line.len())
                } else {
                    0
                };

                line.get(from..)?.find(pattern).map(|offset| from + offset)
            } else {
                let to = if 0 == step { at } else { line.len() };

                line.get(..to)?.rfind(pattern)
            };
            if let Some(offset) = found {
                return Some(LogicalPosition {
                    line: index,
                    grapheme: grapheme_indices(line.get(..offset)?).count(),
                });
            }
        }

        None
    }

    /// Hands the editor one of the events an application loop delivers, as it would be delivered
    /// into an area of `area`.
    ///
    /// Pasted text reaches the engine as the keys it stands for and reaches the application's own
    /// keys not at all, so a paste ends no program and scrolls no window whatever it holds. A
    /// paste while the transcript has the keys reaches neither: what a reader pasted belongs to
    /// the thing they were typing at, and that thing is one nothing writes to.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    pub fn handle(&mut self, area: Rect, event: &Event) -> Outcome {
        match event {
            Event::Key(key) => self.press(area, *key),
            Event::Paste(_) => {
                self.notice = None;
                if Focus::Transcript == self.focus {
                    self.notice = Some(REFUSAL.to_owned());

                    return Outcome::Continues;
                }
                self.dispatch(area, |engine| engine.handle(event));
                self.follow(area);

                Outcome::Continues
            }
            Event::Resize { .. } => {
                if let Some(geometry) = self.panel_geometry(area) {
                    self.panel.resize(geometry);
                }
                if let Some(geometry) = self.geometry(area) {
                    self.engine.resize(geometry);
                }
                self.follow(area);
                self.follow_panel(area);

                Outcome::Continues
            }
            Event::Redraw => Outcome::Continues,
            Event::Notice(notice) => {
                self.notice = Some(notice.to_string());

                Outcome::Continues
            }
        }
    }

    /// Scrolls the window by one command, as it would be scrolled in an area of `area`.
    ///
    /// A scroll that carries the cursor along carries the engine's with it, so the keystroke after
    /// a scroll edits the text where the cursor is drawn.
    ///
    /// An area too small to draw text in scrolls nothing, because there is no window for a scroll
    /// to count the rows of.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`screen::scroll`]'s return values on failure.
    pub fn scroll(&mut self, area: Rect, command: Command) -> Result<(), Error> {
        let Some(geometry) = self.geometry(area) else {
            return Ok(());
        };
        let scrolled = screen::scroll(&self.text, &self.viewport, self.cursor, &geometry, command)?;
        self.viewport = scrolled.viewport;
        self.cursor = scrolled.cursor;
        self.engine.place(self.cursor);
        self.selection = self.engine.selection();

        Ok(())
    }

    /// Types one key at the transcript panel, scrolling it by the keys the panel binds nothing to.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    fn read(&mut self, area: Rect, key: KeyEvent) -> Outcome {
        if let Some(scrolled) = rolled_by(key) {
            let moved = if scrolled {
                self.panel.below(self.top)
            } else {
                self.panel.above(self.top)
            };
            if let Some(top) = moved {
                self.top = top;
            }

            return Outcome::Continues;
        }
        if let Some(geometry) = self.panel_geometry(area) {
            self.panel.resize(geometry);
        }
        if let Err(error) = self.panel.press(key) {
            self.notice = Some(error.to_string());
        } else if let Some(refusal) = self.panel.refusal() {
            self.notice = Some(refusal.to_string());
        } else if let Some(notice) = self.panel.notice() {
            self.notice = Some(notice.to_owned());
        }
        self.held = self.panel.selection();
        self.follow_panel(area);

        Outcome::Continues
    }

    /// Scrolls the transcript panel so that it draws the row its cursor rests on.
    ///
    /// The panel follows its cursor the way the file's window does, and for the same reason: a `j`
    /// past the bottom row moves a cursor nobody can see. What it costs is the rows it walks over
    /// rather than the transcript it walks through, so a step over a closed fold costs one row
    /// however many lines that fold hides -- and a cursor carried further than a follow walks
    /// leaves the panel where it stands rather than walking the whole of what was said.
    ///
    /// A scroll is not a follow. `CTRL-E` and `CTRL-Y` move the panel away from its cursor on
    /// purpose, which is why they are answered before this is ever reached.
    fn follow_panel(&mut self, area: Rect) {
        let rows = usize::from(self.split(area).0.height);
        if 0 == rows {
            return;
        }
        let at = self.panel.at();
        if self
            .panel
            .rows(self.top, rows)
            .iter()
            .any(|row| holds(row, at))
        {
            return;
        }

        let below = self.panel.rows(self.top, rows + FOLLOWED);
        if let Some(index) = below.iter().position(|row| holds(row, at)) {
            let mut top = self.top;
            for _ in 0..(index + 1).saturating_sub(rows) {
                let Some(next) = self.panel.below(top) else {
                    break;
                };
                top = next;
            }
            self.top = top;

            return;
        }

        let mut top = self.top;
        for _ in 0..FOLLOWED {
            let Some(next) = self.panel.above(top) else {
                return;
            };
            top = next;
            if self
                .panel
                .rows(top, 1)
                .first()
                .is_some_and(|row| holds(row, at))
            {
                self.top = top;

                return;
            }
        }
    }

    /// Draws the rows of the transcript panel, top to bottom, and blanks what is left of the area
    /// below them.
    ///
    /// A closed fold is drawn in the one row its summary is, unwrapped and cut to the columns
    /// there are, and every other row is drawn from the block's own source in the styles the
    /// block carries.
    fn draw_panel(&self, cells: &mut Cells, area: Rect) -> Option<Position> {
        let renderer = Renderer::new(self.metrics);
        let drawn = self.panel.rows(self.top, usize::from(area.height));
        for (index, row) in drawn.iter().enumerate() {
            let Ok(at) = u16::try_from(index) else {
                break;
            };
            match row {
                Drawn::Summary(summary) => {
                    for x in area.x..area.right() {
                        cells[(x, area.y + at)].reset();
                    }
                    cells.set_stringn(
                        area.x,
                        area.y + at,
                        summary.text(),
                        usize::from(area.width),
                        Style::default(),
                    );
                }
                Drawn::Body { block, row } => {
                    renderer.draw_styled_row(
                        cells,
                        area,
                        at,
                        row.styled(),
                        continues(drawn.get(index + 1), *block, row),
                    );
                }
            }
        }
        blank(cells, area, narrowed(drawn.len()));
        self.paint_panel(cells, area, &drawn);

        self.panel_cursor(&drawn, area)
    }

    /// Paints the selection the panel's keys are making over the rows it was drawn in.
    ///
    /// The rows are handed to the selection rather than the selection walked into the rows, so
    /// what a frame costs is the screenful it draws however many lines the selection covers, which
    /// is what `ggVG` over what a tool wrote asks for. A selection made by a text object -- `viac`
    /// over a fenced code block -- is drawn for the same reason a plain visual one is: a reader who
    /// cannot see what `iac` took cannot tell it from what `iam` would have taken.
    fn paint_panel(&self, cells: &mut Cells, area: Rect, drawn: &[Drawn]) {
        let Some(held) = &self.held else {
            return;
        };
        let Some(block) = self.panel.transcript().block(held.block()) else {
            return;
        };
        let mut screen_rows = Vec::new();
        let mut rows = Vec::new();
        for (index, row) in drawn.iter().enumerate() {
            if let Drawn::Body { block: from, row } = row {
                if *from == held.block() {
                    screen_rows.push(index);
                    rows.push(row);
                }
            }
        }

        let source = Selectable::new(block.source(), self.metrics);
        for highlight in held.selection().painted(source, rows) {
            let Some(screen_row) = screen_rows.get(highlight.row()) else {
                continue;
            };
            paint(
                cells,
                area,
                narrowed(*screen_row),
                highlight.columns(),
                SELECTION,
            );
        }
    }

    /// # Returns
    ///
    /// The cell of `area` a terminal should rest the cursor in while the transcript has the keys,
    /// or [`None`] where the rows drawn do not hold the byte the cursor rests on, which is what a
    /// panel scrolled away from its cursor has.
    ///
    /// A cursor resting on a closed fold rests on the first column of the one row that fold is
    /// drawn in, as vim's does, because the row is drawn from no byte of the block it stands for
    /// and there is no byte of it to place the cursor at.
    fn panel_cursor(&self, drawn: &[Drawn], area: Rect) -> Option<Position> {
        let at = self.panel.at();
        let block = self.panel.transcript().block(at.block())?;
        let source = block.source();
        let start = source
            .get(..at.offset())?
            .rfind('\n')
            .map_or(0, |separator| separator + 1);
        let grapheme = grapheme_indices(source.get(start..at.offset())?).count();
        for (index, drawn_row) in drawn.iter().enumerate() {
            if !holds(drawn_row, at) {
                continue;
            }

            return match drawn_row {
                Drawn::Summary(_) => folded_cell(area, narrowed(index)),
                Drawn::Body { row, .. } => {
                    cursor_cell(area, narrowed(index), row.styled().row(), grapheme)
                }
            };
        }

        None
    }

    /// # Returns
    ///
    /// The geometry the transcript panel is laid out in, which is the whole of the area the text
    /// would be drawn in because a transcript is drawn without a gutter, or [`None`] where the
    /// area is too small to draw a column of text or a row of one in.
    fn panel_geometry(&self, area: Rect) -> Option<Geometry> {
        let text = self.split(area).0;

        Some(
            Geometry::new(
                NonZeroUsize::new(usize::from(text.width))?,
                NonZeroUsize::new(usize::from(text.height))?,
            )
            .with_metrics(self.metrics)
            .with_options(self.options.clone()),
        )
    }

    /// Draws the status line into the row it was given, which is nothing at all where it was given
    /// no row.
    fn draw_status(&self, cells: &mut Cells, area: Rect) {
        if area.is_empty() {
            return;
        }
        for x in area.x..area.right() {
            cells[(x, area.y)].reset();
        }
        cells.set_stringn(
            area.x,
            area.y,
            self.status(),
            usize::from(area.width),
            Style::default(),
        );
    }

    /// Runs one keystroke against the engine, laid out in the window `area` draws, and reads back
    /// what it left behind.
    ///
    /// # Type Parameters
    ///
    /// * `PressType` - What the keystroke asks of the engine.
    fn dispatch<PressType>(&mut self, area: Rect, press: PressType)
    where
        PressType: FnOnce(&mut Engine) -> Result<(), engine::Error>,
    {
        if let Some(geometry) = self.geometry(area) {
            self.engine.resize(geometry);
        }
        if let Err(error) = press(&mut self.engine) {
            self.notice = Some(error.to_string());
        }
        self.adopt();
    }

    /// Reads the text, the cursor and the selection back out of the engine, which is the authority
    /// on all three.
    ///
    /// The text is read back only over the keystrokes that could have changed it, which is what
    /// keeps a keystroke from costing the file. The engine holds its text as a rope and the layout
    /// reads it as one string per line, so laying it out again costs every line of it; a motion, a
    /// scroll and a selection change none of those lines, and the engine says so. What is read
    /// back after every keystroke is the cursor and the selection alone, and each of those costs
    /// the line it stands on.
    ///
    /// A viewport left anchored past the end of a text an edit shortened is taken back to the top,
    /// since the row it was anchored to is no longer in the text and the window is about to follow
    /// the cursor anyway.
    fn adopt(&mut self) {
        let revision = self.engine.revision();
        if revision != self.revision {
            self.revision = revision;
            let text = self.engine.text();
            self.text = Buffer::from_text(text.strip_suffix('\n').unwrap_or(&text));
            self.viewport = screen::held(&self.text, self.viewport);
        }

        let at = self.engine.cursor();
        let line = self.text.line(at.line).unwrap_or_default();
        self.cursor = self.text.clamp(LogicalPosition {
            line: at.line,
            grapheme: grapheme_at(line, at.column),
        });
        self.selection = self.engine.selection();
    }

    /// # Returns
    ///
    /// The bytes the editor would write the text out as, which is every line of it followed by a
    /// line ending, as vim writes a file with `'endofline'` set.
    fn written(&self) -> String {
        written(&self.text)
    }

    /// Scrolls the window so that it draws the row the cursor rests on.
    ///
    /// The window follows the cursor rather than the other way about, so the cursor the engine
    /// left is the cursor that stays: a cursor below the window is drawn on its bottom row and one
    /// above it on its top row, which costs the rows of a window rather than the distance the
    /// cursor jumped.
    ///
    /// What counts as drawn is the band `'scrolloff'` leaves the cursor rather than the whole
    /// window, since vim moves the window to keep those rows beside the cursor rather than moving
    /// the cursor out of them, and the placing commands the window is moved by keep the same rows.
    fn follow(&mut self, area: Rect) {
        let Some(geometry) = self.geometry(area) else {
            return;
        };
        let screen = Screen::of(&self.text, &self.viewport, self.cursor, &geometry);
        let rows = geometry.window().height().get();
        let kept = self.scrolloff.min((rows - 1) / 2);
        let above = match screen.cursor_row() {
            Some(row) if kept <= row && row + kept < rows => return,
            Some(row) => row < kept,
            None => screen.rows().first().is_some_and(|row| {
                (self.cursor.line, self.cursor.grapheme) < (row.line(), row.start())
            }),
        };
        let command = if above {
            Command::CursorToTop
        } else {
            Command::CursorToBottom
        };

        match screen::scroll(&self.text, &self.viewport, self.cursor, &geometry, command) {
            Ok(scrolled) => self.viewport = scrolled.viewport,
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    /// # Returns
    ///
    /// The rows of `area` the text is drawn into, and the row the status line is drawn into, which
    /// is empty where the application draws no status line or the area holds no row to spare.
    fn split(&self, area: Rect) -> (Rect, Rect) {
        if !self.status || area.height < 2 {
            return (area, Rect::ZERO);
        }

        (
            Rect {
                height: area.height - 1,
                ..area
            },
            Rect {
                y: area.bottom() - 1,
                height: 1,
                ..area
            },
        )
    }

    /// # Returns
    ///
    /// The display columns the gutter takes off the left of an area.
    fn gutter_columns(&self) -> usize {
        self.gutter.width(self.text.line_count())
    }
}

/// Resets the rows of `area` from `top` down, so that no row the text does not reach keeps what an
/// earlier frame drew there.
fn blank(cells: &mut Cells, area: Rect, top: u16) {
    for y in (area.y + top)..area.bottom() {
        for x in area.x..area.right() {
            cells[(x, y)].reset();
        }
    }
}

/// # Returns
///
/// The cell of `area` the cursor rests in on the row a closed fold is drawn in, which is the
/// first column of the row `screen_row`, or [`None`] where the area has no such row.
fn folded_cell(area: Rect, screen_row: u16) -> Option<Position> {
    (screen_row < area.height).then(|| Position {
        x: area.x,
        y: area.y + screen_row,
    })
}

/// # Returns
///
/// `columns` as a terminal coordinate, saturated at the widest a terminal can be.
fn narrowed(columns: usize) -> u16 {
    u16::try_from(columns).unwrap_or(u16::MAX)
}

/// # Returns
///
/// The grapheme of `line` the byte `offset` falls in, which is the offset past its last grapheme
/// where the line is shorter than that. A cursor counted in bytes can only stand where a grapheme
/// begins, but a grapheme is where a screen draws it, so a byte in the middle of one is reported
/// as the whole of it.
fn grapheme_at(line: &str, offset: usize) -> usize {
    let mut counted = 0;
    for (start, grapheme) in grapheme_indices(line) {
        if offset < start + grapheme.len() {
            return counted;
        }
        counted += 1;
    }

    counted
}

/// # Returns
///
/// How a run of keys is spelled in a message about it.
fn spelled(keys: &[TerminalKey]) -> String {
    keys.iter().map(ToString::to_string).collect()
}

/// # Returns
///
/// Whether `key` is the interrupt a terminal sends, which stops the program from any mode.
fn interrupts(key: KeyEvent) -> bool {
    KeyCode::Char('c') == key.code && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// # Returns
///
/// Whether `key` moves the keys between the file and the transcript, which `<C-T>` does from
/// either of them and from any mode, because a panel that could only be reached from normal mode
/// is a panel insert mode hides.
fn transcribes(key: KeyEvent) -> bool {
    KeyCode::Char('t') == key.code && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// # Returns
///
/// Whether `key` scrolls the transcript panel and whether it scrolls it downward, or [`None`]
/// where it scrolls it not at all.
fn rolled_by(key: KeyEvent) -> Option<bool> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }

    match key.code {
        KeyCode::Char('e') => Some(true),
        KeyCode::Char('y') => Some(false),
        _ => None,
    }
}

/// # Returns
///
/// The row that follows `row` of the block `block` within the same logical line, which is what
/// says whether the cells the row has left over are the ones a wide character is marked in, and
/// [`None`] where the next row drawn begins a logical line of its own.
fn continues<'row>(
    next: Option<&'row Drawn>,
    block: usize,
    row: &RenderedRow,
) -> Option<&'row StyledRow> {
    match next {
        Some(Drawn::Body {
            block: below,
            row: following,
        }) if block == *below && row.styled().row().line() == following.styled().row().line() => {
            Some(following.styled())
        }
        _ => None,
    }
}

/// # Returns
///
/// The bytes `text` is spelled out as: every line of it followed by a line ending, as vim writes a
/// file with `'endofline'` set.
///
/// This is the one spelling the text crosses every boundary in, because the boundaries have to be
/// each other's inverse. The engine's rope terminates its last line and a [`Buffer`] does not, so a
/// buffer whose last line is empty spells out as one line fewer than it holds unless the ending is
/// put back: handing the rope `Buffer::text` and reading it back with the ending taken off drops a
/// line each time it is done, and `:w` then wrote a file shorter than the one it read.
fn written(text: &Buffer) -> String {
    let mut written = text.text();
    written.push('\n');

    written
}

/// # Returns
///
/// Whether `key` ends the program, which `q` does where the engine bound nothing to it.
fn quits(key: KeyEvent) -> bool {
    KeyCode::Char('q') == key.code && key.modifiers.is_empty()
}

/// # Returns
///
/// What `key` opens a line at the status line for and the character that line begins with, or
/// [`None`] where it opens none.
///
/// The shift a terminal reports beside a `:` or a `?` is the shift that typed the character, so it
/// is not a modifier that makes the key another key.
fn opened_by(key: KeyEvent) -> Option<(Asked, char)> {
    if !types(key) {
        return None;
    }

    match key.code {
        KeyCode::Char(COMMAND) => Some((Asked::Command, COMMAND)),
        KeyCode::Char(FORWARD) => Some((Asked::Search(true), FORWARD)),
        KeyCode::Char(BACKWARD) => Some((Asked::Search(false), BACKWARD)),
        _ => None,
    }
}

/// # Returns
///
/// Whether `key` is a character typed into a line at the status line rather than a command over
/// it, which every key held with something other than shift is: `<C-T>` is not the letter `t`.
fn types(key: KeyEvent) -> bool {
    key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
}

/// # Returns
///
/// Whether `key` repeats the last search, and whether it repeats it the way that search ran rather
/// than the other way, or [`None`] where it repeats it not at all.
fn repeats(key: KeyEvent) -> Option<bool> {
    if !types(key) {
        return None;
    }

    match key.code {
        KeyCode::Char('n') => Some(true),
        KeyCode::Char('N') => Some(false),
        _ => None,
    }
}

/// # Returns
///
/// Whether `row` of the transcript panel draws the byte the panel's cursor rests on, which the one
/// row a closed fold is drawn in does for every byte of the block that fold heads.
fn holds(row: &Drawn, at: Resting) -> bool {
    match row {
        Drawn::Summary(summary) => at.block() == summary.head(),
        Drawn::Body { block, row } => {
            let source = row.source();

            at.block() == *block && source.start <= at.offset() && at.offset() <= source.end
        }
    }
}

/// # Returns
///
/// The scroll `key` asks for, or [`None`] where it asks for none.
fn scrolled_by(key: KeyEvent) -> Option<Command> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }

    match key.code {
        KeyCode::Char('d') => Some(Command::HalfPageDown),
        KeyCode::Char('u') => Some(Command::HalfPageUp),
        KeyCode::Char('f') => Some(Command::PageDown),
        KeyCode::Char('b') => Some(Command::PageUp),
        KeyCode::Char('e') => Some(Command::RowDown),
        KeyCode::Char('y') => Some(Command::RowUp),
        _ => None,
    }
}
