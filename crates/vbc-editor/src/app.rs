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
//! What that reconciliation costs is the text rather than the window: the engine holds its text as
//! a rope and the layout reads it as one string per line, so every keystroke reads the whole of it
//! back and lays the lines out again. A frame still costs the window it draws, which is the
//! property the anchor-relative layout was built for, and a keystroke costs about two and a half
//! milliseconds over fifty thousand lines against a twentieth of one over a hundred.
//!
//! The window is measured from the area a frame is drawn into rather than stored, so a terminal
//! that was resized between two frames draws the second one at its new size without being told,
//! and the engine is laid out in that same window so that a display motion is measured in the
//! terminal it was typed at. The gutter takes its columns off the left of that area and the text
//! wraps into what is left, so a wider gutter narrows the text rather than pushing it off the
//! screen.

use std::num::NonZeroUsize;

use crossterm::event::{KeyCode, KeyModifiers};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::Widget;
use ratatui::Frame;
use vbc_layout::buffer::Buffer;
use vbc_layout::line::Options;
use vbc_layout::position::LogicalPosition;
use vbc_layout::viewport::{Command, Viewport};
use vbc_layout::width::{grapheme_indices, Metrics};

use crate::engine::{self, Engine};
use crate::event::{Event, KeyEvent};
use crate::gutter::{Gutter, Options as GutterOptions};
use crate::render::{cursor_cell, Renderer};
use crate::screen::{self, Error, Geometry, Screen};

/// What the status line says in each of the modes vim names in it, which is nothing at all in
/// normal mode because vim says nothing there either.
const INSERTING: &str = "-- INSERT --";
const SELECTING: &str = "-- SELECT --";
const VISUAL: &str = "-- VISUAL --";

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
        let mut app = Self {
            engine: Engine::new(&text.text()),
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
        };
        app.adopt();

        app
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
    /// What the last keystroke could not do, and [`None`] where it did what it asked for.
    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// # Returns
    ///
    /// What the status line says: what the last keystroke could not do, or the mode the editor is
    /// in, which is nothing at all in normal mode.
    #[must_use]
    pub fn status(&self) -> &str {
        if let Some(notice) = &self.notice {
            return notice;
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
            top += renderer.draw_line(cells, text, top, rows);
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
            if quits(key) {
                return Outcome::Stops;
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

    /// Hands the editor one of the events an application loop delivers, as it would be delivered
    /// into an area of `area`.
    ///
    /// Pasted text reaches the engine as the keys it stands for and reaches the application's own
    /// keys not at all, so a paste ends no program and scrolls no window whatever it holds.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    pub fn handle(&mut self, area: Rect, event: &Event) -> Outcome {
        match event {
            Event::Key(key) => self.press(area, *key),
            Event::Paste(_) => {
                self.notice = None;
                self.dispatch(area, |engine| engine.handle(event));
                self.follow(area);

                Outcome::Continues
            }
            Event::Resize { .. } => {
                if let Some(geometry) = self.geometry(area) {
                    self.engine.resize(geometry);
                }
                self.follow(area);

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

        Ok(())
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

    /// Reads the text and the cursor back out of the engine, which is the authority on both.
    ///
    /// A viewport left anchored past the end of a text an edit shortened is taken back to the top,
    /// since the row it was anchored to is no longer in the text and the window is about to follow
    /// the cursor anyway.
    fn adopt(&mut self) {
        let text = self.engine.text();
        self.text = Buffer::from_text(text.strip_suffix('\n').unwrap_or(&text));

        let at = self.engine.cursor();
        let line = self.text.line(at.line).unwrap_or_default();
        self.cursor = self.text.clamp(LogicalPosition {
            line: at.line,
            grapheme: grapheme_at(line, at.column),
        });
        self.viewport = screen::held(&self.text, self.viewport);
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
/// Whether `key` ends the program, which `q` does where the engine bound nothing to it.
fn quits(key: KeyEvent) -> bool {
    KeyCode::Char('q') == key.code && key.modifiers.is_empty()
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
