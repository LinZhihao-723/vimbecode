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
//! The text is the draft of the reader's next message, so it can be sent and searched. `:` opens
//! the ex command line and `/` a search, and while either of them is open every key typed belongs
//! to that line rather than to the engine, because the `w` of `:wq` is not a word motion. `:wq`
//! sends the draft and leaves an empty one, `:q!` discards it, and `:qa` refuses to leave a draft
//! nothing has sent unless it is told `:qa!`, so the only command that throws a reader's words
//! away is one that says it will. The interrupt stops the session's turn and never the program. A
//! search is over the literal bytes typed at it -- this editor has no regular expressions and does
//! not pretend to -- and `n` and `N` repeat it the way it ran and the other way.
//!
//! What is selected is drawn, in either of the two panels: the range `v`, `V` and `CTRL-V` are
//! moving over the draft, and the range `viac` took out of a block of the transcript. The painting
//! is laid over the cells the rows were already drawn in rather than folded into the styles they
//! were drawn with, which is what lets a selection cross a wrap boundary without the layout being
//! told anything about it. A panel nothing can be written to is a panel whose whole point is what
//! it selects, so a selection nobody can see there is a feature nobody can use.
//!
//! The application draws two panels and gives the keys to one of them at a time: the history of
//! what was said, and the prompt the draft is written in. Laid out as the conversation screen, it
//! draws both in every frame, the history over the prompt. `<C-W>` moves the keys between them
//! from normal and visual mode, and `<C-T>` from any mode, because a panel reachable only from
//! normal mode is a panel insert mode hides. While the history has the keys they go to its own
//! panel, which reads them through the same table with the transcript's own sequences bound in it
//! and refuses every one that would write. Both of them follow their cursor, and for the same
//! reason: a `j` past the bottom row moves a cursor nobody can see. What the panel's own following
//! costs is the rows it walks over rather than the transcript it walks through, so a step over a
//! closed fold costs one row however many lines that fold hides.
//!
//! Those two are two engines and one register file. Each of them has a text, a cursor and a mode
//! of its own, and neither has registers of its own, because what a reader takes out of the
//! history they mean to put into the draft: an application that let each engine keep a file of
//! its own would answer `yac` in the history and `p` in the prompt with a yank into a drawer
//! nothing opens, which is the gesture this editor exists for going nowhere. So the register file
//! is built once, where the engine is, and every panel the application builds afterwards is handed
//! it.
//!
//! One of the registers those two engines share is not a register at all. `"+` is the desktop's
//! clipboard and `"*` is another name for it, so `"+yy` here leaves the line in a window the editor
//! has nothing to do with and `"+p` brings back whatever the reader last copied somewhere else.
//! Neither of those is a thing the drawing thread may wait on: the desktop answers in a fraction of
//! a millisecond when it is warm and in over a second when it is locked or when the window holding
//! the clipboard is busy. So a yank is handed over and not waited for, and a put is held -- the key
//! that would run it is kept, the frames go on being drawn, and the put runs on the frame the
//! answer arrives at or is abandoned with nothing pasted at the frame the deadline passes. Keys
//! typed while a put is held are kept behind it rather than run ahead of it, because a keystroke
//! that overtakes the put it followed is a keystroke that edits the wrong text.
//!
//! The window is measured from the area a frame is drawn into rather than stored, so a terminal
//! that was resized between two frames draws the second one at its new size without being told,
//! and the engine is laid out in that same window so that a display motion is measured in the
//! terminal it was typed at. The gutter takes its columns off the left of that area and the text
//! wraps into what is left, so a wider gutter narrows the text rather than pushing it off the
//! screen.

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::ops::Range;

use crossterm::event::{KeyCode, KeyModifiers};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use ratatui::Frame;
use vbc_layout::buffer::Buffer;
use vbc_layout::line::{DisplayRow, Options};
use vbc_layout::position::LogicalPosition;
use vbc_layout::viewport::{Command, Viewport};
use vbc_layout::width::{grapheme_indices, graphemes, Metrics};

use crate::chat::block::RenderedRow;
use crate::chat::fold::{Position as Placed, Tag};
use crate::chat::object::Position as Resting;
use crate::chat::policy::{Drawn, Panel, Selected, REFUSAL};
use crate::chat::selection::Source as Selectable;
use crate::chat::transcript::Transcript;
use crate::chat::yank::{CLIPBOARD, YANK};
use crate::clipboard::register::{Bridge, Settled};
use crate::engine::{self, typed, Engine, Position as Caret, Shape, Yanked};
use crate::event::{Event, KeyEvent};
use crate::gutter::{Gutter, Options as GutterOptions};
use crate::keys::Argument;
use crate::render::{cursor_cell, paint, painted_columns, Renderer};
use crate::screen::{self, Error, Geometry, Screen};
use crate::session::control::Decision;
use crate::session::live::{Session, REFUSED};
use crate::style::StyledRow;

/// What the status line says in each of the modes vim names in it, which is nothing at all in
/// normal mode because vim says nothing there either.
const INSERTING: &str = "-- INSERT --";
const SELECTING: &str = "-- SELECT --";
const VISUAL: &str = "-- VISUAL --";

/// What the status line says while the history panel has the keys.
const READING: &str = "-- HISTORY --";

/// How the cells a selection covers are drawn, which is what vim's `Visual` highlight is by
/// default: the colours the text was already drawn in, swapped.
pub const SELECTION: Style = Style::new().add_modifier(Modifier::REVERSED);

/// The keys a line is typed at the status line by: the ex command line, and the two directions a
/// search is started in.
const COMMAND: char = ':';
const FORWARD: char = '/';
const BACKWARD: char = '?';

/// What the status line says about a command line that could not do what it asked for, or did
/// something other than what it usually does.
const UNSENT: &str = "the draft is not sent (`:wq` sends it, add `!` to override)";
const UNDRAFTED: &str = "the draft is empty, so nothing was sent";
const QUEUED: &str = "sent, and queued behind the turn that is running";
const UNSEARCHED: &str = "there is no search to repeat";
const UNSESSIONED: &str = "there is no session to say that to";
const UNASKED: &str = "the session is waiting on nothing";
const UNANSWERABLE: &str = "what the session is waiting on is not a question to answer in words";
const UNSAID: &str = "there is nothing to say";

/// How many rows the transcript panel is walked over looking for the row its cursor is on before a
/// follow gives up and leaves the panel where it stands.
const FOLLOWED: usize = 256;

/// What the status line says about the interrupt.
const INTERRUPTED: &str = "interrupted the turn";
const UNINTERRUPTED: &str = "no turn is running to interrupt";

/// What the status bar says about a session's turn while it runs and while none does, and what it
/// puts between the things it says.
const RESPONDING: &str = "Claude is responding";
const IDLE: &str = "idle";
const GAP: &str = "  ";

/// The fewest rows the prompt panel is drawn in, and the most it is drawn in as a percentage of
/// the window.
const PROMPT_FLOOR: usize = 3;
const PROMPT_PERCENT: usize = 40;

/// How the row between the history and the prompt is drawn, and the mark in it naming the panel
/// that has the keys, drawn that many columns in from the left.
const DIVIDER: &str = "─";
const DIVIDER_STYLE: Style = Style::new().fg(Color::DarkGray);
const HISTORY_MARK: &str = " ▲ history ";
const PROMPT_MARK: &str = " ▼ prompt ";
const MARK_STYLE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const MARK_INDENT: u16 = 2;

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
struct CommandLine {
    asked: Asked,
    line: String,
}

impl CommandLine {
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

/// Where each part of a frame is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Layout {
    history: Rect,
    divider: Rect,
    prompt: Rect,
    status: Rect,
}

/// A put held until the desktop's clipboard answers, and what was typed while it waits.
///
/// What is typed is kept rather than run, so that the put a reader asked for first is the edit that
/// happens first. It is handed to the editor again, in the order it arrived, on the frame the put
/// is over. What is not kept is what a terminal says about itself rather than about the text: a
/// resize changes the window a frame is drawn in and there is nothing to be gained by drawing the
/// old one until the desktop answers.
#[derive(Clone, Debug)]
struct Put {
    key: KeyEvent,
    behind: VecDeque<Event>,
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
    command_line: Option<CommandLine>,
    pattern: Option<String>,
    forward: bool,
    selection: Option<(Caret, Caret, Shape)>,
    held: Option<Selected>,
    taking: Option<String>,
    windowing: bool,
    split: bool,
    fitted: Option<Rect>,
    session: Option<Session>,
    waiting: Option<String>,
    drawn: u64,
    refreshed: bool,
    clipboard: Option<Bridge>,
    put: Option<Put>,
}

/// Which of the two panels the application draws the keys are typed at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    /// The prompt, where the draft of the next message is written.
    Prompt,

    /// The history of what was said, which is read rather than written.
    History,
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
            focus: Focus::Prompt,
            top: Placed::top(0),
            revision: u64::MAX,
            command_line: None,
            pattern: None,
            forward: true,
            selection: None,
            held: None,
            taking: None,
            windowing: false,
            split: false,
            fitted: None,
            session: None,
            waiting: None,
            drawn: 0,
            refreshed: false,
            clipboard: None,
            put: None,
        };
        app.adopt();

        app
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created application over an empty draft, laid out as [`App::composing`] lays one
    /// out.
    #[must_use]
    pub fn chat() -> Self {
        Self::new(Buffer::new()).composing()
    }

    /// # Returns
    ///
    /// This application laid out as the conversation screen: the history panel over the prompt
    /// panel over the status bar, all three drawn in every frame, with the keys at the prompt in
    /// insert mode.
    #[must_use]
    pub fn composing(mut self) -> Self {
        self.split = true;
        self.status = true;
        self.focus = Focus::Prompt;
        self.insert();

        self
    }

    /// # Returns
    ///
    /// This application showing `transcript` in the panel `<C-T>` reaches, with nothing nested
    /// inside anything else.
    #[must_use]
    pub fn with_transcript(self, transcript: Transcript) -> Self {
        let tags = vec![Tag::untagged(); transcript.len()];

        self.with_conversation(transcript, tags)
    }

    /// # Returns
    ///
    /// This application showing `transcript` in the panel `<C-T>` reaches, folded the way the
    /// calls its blocks arrived beneath nest. A block's tag names the call it is answered under
    /// and the call it was said beneath, which is what a subagent's output arrives tagged with at
    /// every depth.
    #[must_use]
    pub fn with_conversation(mut self, transcript: Transcript, tags: Vec<Tag>) -> Self {
        self.adopt_conversation(transcript, tags);

        self
    }

    /// # Returns
    ///
    /// This application showing `session` in the panel `<C-T>` reaches, live: what the session
    /// says is drawn as it arrives, and what it stops to ask is drawn under that until somebody
    /// answers it.
    ///
    /// The session is read from the events an application loop hands over rather than from a
    /// thread of its own, so an application that never calls [`App::handle`] never reads it.
    #[must_use]
    pub fn with_session(mut self, session: Session) -> Self {
        let (transcript, tags) = session.panel();
        self.drawn = session.revision();
        self.session = Some(session);
        self.adopt_conversation(transcript, tags);

        self
    }

    /// # Returns
    ///
    /// This application reaching the desktop's clipboard through `clipboard`, so that `"+` and
    /// `"*` are what another window copied rather than a drawer of the editor's own.
    ///
    /// The bridge is handed the one register file the engines share, because the register a
    /// keystroke names has to be the register the desktop is reached through. An application given
    /// no bridge has `"+` as a register like any other, which is what an engine under test wants.
    #[must_use]
    pub fn with_clipboard(mut self, clipboard: Bridge) -> Self {
        self.clipboard = Some(clipboard.sharing(self.engine.register_file().clone()));

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
    /// The history panel the keys reach while it has the focus.
    pub fn panel(&mut self) -> &mut Panel {
        &mut self.panel
    }

    /// # Returns
    ///
    /// The session the panel is reading, and [`None`] where the panel is reading an exchange
    /// nothing is still saying anything in.
    #[must_use]
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// # Returns
    ///
    /// The bridge to the desktop's clipboard, and [`None`] where the application was given none.
    #[must_use]
    pub fn clipboard(&self) -> Option<&Bridge> {
        self.clipboard.as_ref()
    }

    /// # Returns
    ///
    /// Whether a put is being held until the desktop's clipboard answers.
    #[must_use]
    pub fn awaits_clipboard(&self) -> bool {
        self.put.is_some()
    }

    /// # Returns
    ///
    /// Whether something other than a keystroke changed what a frame would draw since this was
    /// last asked, which is what says a redraw is owed to an event that asked for none.
    pub fn refreshed(&mut self) -> bool {
        std::mem::take(&mut self.refreshed)
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
    /// what the session has stopped and is waiting for, or the mode of the panel that has the keys,
    /// which is nothing at all in the prompt's normal mode.
    ///
    /// A session waits for as long as nobody answers it, so what it is waiting for is said where
    /// the line would otherwise say nothing rather than over the mode. A reader who cannot see
    /// `-- INSERT --` cannot see which keys they are typing, and that is the one thing a status
    /// line is for. The conversation screen says it beside the mode instead, as [`App::turn`].
    #[must_use]
    pub fn status(&self) -> &str {
        if let Some(command) = &self.command_line {
            return &command.line;
        }
        if let Some(notice) = &self.notice {
            return notice;
        }
        let waiting = if self.split {
            ""
        } else {
            self.waiting.as_deref().unwrap_or_default()
        };
        if Focus::History == self.focus {
            if VimMode::Visual == self.panel.mode() {
                return VISUAL;
            }
            if waiting.is_empty() {
                return READING;
            }

            return waiting;
        }

        match self.mode() {
            VimMode::Insert => INSERTING,
            VimMode::Select => SELECTING,
            VimMode::Visual => VISUAL,
            _ => waiting,
        }
    }

    /// # Returns
    ///
    /// What the session's turn is doing: what went wrong with the session, what it is waiting to
    /// be answered, whether it is responding or idle -- or [`None`] where there is no session.
    #[must_use]
    pub fn turn(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let state = if session.responding() {
            RESPONDING
        } else {
            IDLE
        };

        Some(waited(session).unwrap_or_else(|| state.to_owned()))
    }

    /// Measures the window the prompt panel draws in an area, which is the rows the layout gives
    /// it, and the columns the gutter leaves the text.
    ///
    /// # Returns
    ///
    /// The geometry a frame drawn into `area` lays the draft out to, or [`None`] where the area is
    /// too small to draw a column of text or a row of one in.
    #[must_use]
    pub fn geometry(&self, area: Rect) -> Option<Geometry> {
        self.text_geometry(self.layout(area).prompt)
    }

    /// Draws one frame: the panels the layout draws, the row between them, and the status line
    /// along the bottom where the application was given one, and nothing at all where the area is
    /// too small to hold them.
    ///
    /// # Returns
    ///
    /// The cell of `area` a terminal should rest the cursor in, or [`None`] where the frame does
    /// not draw the cursor's own row.
    pub fn draw(&self, cells: &mut Cells, area: Rect) -> Option<Position> {
        let layout = self.layout(area);
        self.draw_status(cells, layout.status);
        let drawn = if self.split {
            let history = self.draw_panel(cells, layout.history);
            self.draw_divider(cells, layout.divider);
            let prompt = self.draw_text(cells, layout.prompt);
            match self.focus {
                Focus::History => history,
                Focus::Prompt => prompt,
            }
        } else {
            match self.focus {
                Focus::History => self.draw_panel(cells, layout.history),
                Focus::Prompt => self.draw_text(cells, layout.prompt),
            }
        };
        if self.command_line.is_some() {
            return self.command_line_cell(layout.status);
        }

        drawn
    }

    /// Draws the draft: the gutter, the rows of text beside it, and the selection painted over the
    /// cells those rows were drawn in.
    ///
    /// # Returns
    ///
    /// The cell of `body` a terminal should rest the cursor in, or [`None`] where the frame does
    /// not draw the cursor's own row.
    fn draw_text(&self, cells: &mut Cells, body: Rect) -> Option<Position> {
        let geometry = self.text_geometry(body)?;
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
    fn command_line_cell(&self, status: Rect) -> Option<Position> {
        let command = self.command_line.as_ref()?;
        if status.is_empty() {
            return None;
        }
        let column = self.columns(&command.line);

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
    /// The engine reads the key first, and the application's own keys -- the scrolls, and the
    /// lines typed at the status line -- are the ones it bound nothing to, so a key that carries a
    /// sequence further belongs to the sequence rather than to the window. Nor are they read in an
    /// inserting mode, where every key is either text or a key vim answers itself. The interrupt
    /// is the one key read ahead of the engine, because a turn that can only be stopped from
    /// normal mode is a turn insert mode leaves running, and it stops the turn rather than the
    /// program. It abandons a line being typed at the status line on its way, so that what it did
    /// is said where that line would otherwise be drawn.
    ///
    /// A key vim reads a further key after and this editor implements nothing for takes that key
    /// here rather than letting it through: `ma` names a mark this editor does not keep and `za`
    /// opens a fold it does not fold, and an `a` handed on to normal mode opens insert mode
    /// instead of either. The interrupt abandons a key waiting to be taken as it abandons a line
    /// at the status line, because a command a reader stopped is not one whose next keystroke
    /// belongs to it.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    pub fn press(&mut self, area: Rect, key: KeyEvent) -> Outcome {
        if interrupts(key) {
            self.command_line = None;
            self.taking = None;
            self.windowing = false;
            self.interrupt();

            return Outcome::Continues;
        }
        if let Some(put) = self.put.as_mut() {
            put.behind.push_back(Event::Key(key));

            return self.settle(area);
        }
        self.notice = None;
        if std::mem::take(&mut self.windowing) {
            self.window(area, key);

            return Outcome::Continues;
        }
        if let Some(taking) = self.taking.take() {
            self.notice = Some(unimplemented(&taking));

            return Outcome::Continues;
        }
        if self.command_line.is_some() {
            return self.typing(area, key);
        }
        if transcribes(key) {
            self.focus_on(area, self.other());

            return Outcome::Continues;
        }
        if windows(key) && self.commanding() {
            self.windowing = true;

            return Outcome::Continues;
        }
        if Focus::History == self.focus {
            if commands(key) {
                self.command_line = Some(CommandLine::new(Asked::Command, COMMAND));

                return Outcome::Continues;
            }

            return self.read(area, key);
        }
        if self.awaited(key) {
            return self.settle(area);
        }
        let addressed = self.engine.named_register();
        self.dispatch(area, |engine| engine.press(key));
        self.mirror(addressed);

        let unbound = self
            .engine
            .unbound()
            .map(|keys| (spelled(keys), keys.len()));
        let Some((keys, typed)) = unbound else {
            self.follow(area);

            return Outcome::Continues;
        };
        if VimMode::Insert != self.mode() && self.takes_argument(key) {
            self.notice = Some(unimplemented(&keys));
            self.taking = Some(keys);

            return Outcome::Continues;
        }
        if 1 == typed && VimMode::Insert != self.mode() {
            if let Some((asked, opened)) = opened_by(key) {
                self.command_line = Some(CommandLine::new(asked, opened));

                return Outcome::Continues;
            }
            if let Some(again) = repeats(key) {
                self.seek(area, again);

                return Outcome::Continues;
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
            KeyCode::Esc => self.command_line = None,
            KeyCode::Backspace => {
                if let Some(command) = self.command_line.as_mut() {
                    command.line.pop();
                    if command.line.is_empty() {
                        self.command_line = None;
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(command) = self.command_line.take() {
                    return self.entered(area, &command);
                }
            }
            KeyCode::Char(character) if types(key) => {
                if let Some(command) = self.command_line.as_mut() {
                    command.line.push(character);
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
    fn entered(&mut self, area: Rect, command: &CommandLine) -> Outcome {
        match command.asked {
            Asked::Command => self.run(command.typed().trim()),
            Asked::Search(forward) => {
                let pattern = command.typed();
                if !pattern.is_empty() {
                    self.pattern = Some(pattern.to_owned());
                }
                self.forward = forward;
                self.seek(area, true);

                Outcome::Continues
            }
        }
    }

    /// Runs one ex command: `wq`, which sends the draft, `q`, which closes the panel that has the
    /// keys, `qa`, which leaves, the `!` that overrides what they refuse, and the commands that
    /// answer the session.
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
            "q" | "quit" => self.close(forced),
            "qa" | "qall" | "quita" | "quitall" => self.quit(forced),
            "wq" | "x" | "xit" | "exit" => {
                self.send();

                Outcome::Continues
            }
            "ask" => {
                self.ask(named);

                Outcome::Continues
            }
            "allow" => {
                self.decide(&Decision::Allowed);

                Outcome::Continues
            }
            "deny" => {
                let message = if named.is_empty() { REFUSED } else { named };
                self.decide(&Decision::Denied(message.to_owned()));

                Outcome::Continues
            }
            "answer" => {
                if named.is_empty() {
                    self.notice = Some(UNSAID.to_owned());
                } else {
                    self.respond(named);
                }

                Outcome::Continues
            }
            asked => {
                self.notice = Some(format!("`{asked}` is not an editor command"));

                Outcome::Continues
            }
        }
    }

    /// Sends one message from the reader to the session, which is a turn of it, saying at the
    /// status line why it did not where it could not.
    fn ask(&mut self, text: &str) {
        if text.is_empty() {
            self.notice = Some(UNSAID.to_owned());

            return;
        }
        let Some(session) = self.session.as_mut() else {
            self.notice = Some(UNSESSIONED.to_owned());

            return;
        };
        session.ask(text);
    }

    /// Answers the question the session has been waiting on longest, saying at the status line
    /// what was answered or why nothing was.
    fn decide(&mut self, decision: &Decision) {
        let Some(session) = self.session.as_mut() else {
            self.notice = Some(UNSESSIONED.to_owned());

            return;
        };
        let Some(answered) = session.answer(decision) else {
            self.notice = Some(UNASKED.to_owned());

            return;
        };
        self.notice = Some(format!("`{}` answered", answered.tool()));
    }

    /// Answers in words the question the session has been waiting on longest, which is the only
    /// shape an answer to one reaches the model in: an approval carrying none is read as the
    /// reader having declined to answer.
    fn respond(&mut self, answer: &str) {
        let Some(asking) = self.session.as_ref().and_then(Session::question) else {
            self.notice = Some(UNANSWERABLE.to_owned());

            return;
        };

        self.decide(&Decision::answering(&asking, answer));
    }

    /// Sends the draft to the session as the reader's next message and leaves an empty draft in
    /// insert mode, saying at the status line why it did not where it could not.
    fn send(&mut self) {
        if !self.drafted() {
            self.notice = Some(UNDRAFTED.to_owned());

            return;
        }
        let Some(session) = self.session.as_mut() else {
            self.notice = Some(UNSESSIONED.to_owned());

            return;
        };
        if session.responding() {
            self.notice = Some(QUEUED.to_owned());
        }
        session.ask(&self.text.text());
        self.clear();
    }

    /// Closes the panel that has the keys: the history's close leaves the program as `:qa` does,
    /// and the prompt's leaves it only over an empty draft, discarding the draft instead where the
    /// command insisted.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    fn close(&mut self, forced: bool) -> Outcome {
        if Focus::History == self.focus {
            return self.quit(forced);
        }
        if forced {
            self.clear();

            return Outcome::Continues;
        }

        self.quit(false)
    }

    /// # Returns
    ///
    /// Whether the application goes on reading keys, which it does where the draft holds words
    /// nothing has sent and the command did not insist.
    fn quit(&mut self, forced: bool) -> Outcome {
        if !forced && self.drafted() {
            self.notice = Some(UNSENT.to_owned());

            return Outcome::Continues;
        }

        Outcome::Stops
    }

    /// Empties the draft and leaves the prompt in insert mode, where the next message is typed.
    fn clear(&mut self) {
        self.engine.reload(&written(&Buffer::new()));
        self.adopt();
        self.viewport = Viewport::new();
        self.insert();
    }

    /// Puts the prompt in insert mode, from whichever mode it is in.
    fn insert(&mut self) {
        if VimMode::Insert == self.engine.mode() {
            return;
        }
        for key in [KeyCode::Esc, KeyCode::Char('i')] {
            if let Err(error) = self.engine.press(KeyEvent::new(key, KeyModifiers::NONE)) {
                self.notice = Some(error.to_string());
            }
        }
        self.adopt();
    }

    /// Stops the session's running turn and the messages queued behind it, saying at the status
    /// line whether there was one to stop.
    fn interrupt(&mut self) {
        let interrupted = self.session.as_mut().is_some_and(Session::interrupt);
        let said = if interrupted {
            INTERRUPTED
        } else {
            UNINTERRUPTED
        };
        self.notice = Some(said.to_owned());
    }

    /// # Returns
    ///
    /// Whether the draft holds anything but blanks, which is what is worth sending and what
    /// leaving would throw away.
    fn drafted(&self) -> bool {
        self.text.lines().iter().any(|line| !line.trim().is_empty())
    }

    /// Gives the keys to the panel `focus` names.
    fn focus_on(&mut self, area: Rect, focus: Focus) {
        self.focus = focus;
        if Focus::History == focus {
            self.held = self.panel.selection();
            self.follow_panel(area);
        }
    }

    /// # Returns
    ///
    /// The panel that does not have the keys.
    fn other(&self) -> Focus {
        match self.focus {
            Focus::Prompt => Focus::History,
            Focus::History => Focus::Prompt,
        }
    }

    /// # Returns
    ///
    /// Whether the panel that has the keys is in a mode `<C-W>` moves them from, which is normal
    /// and visual mode: in insert mode the same key deletes a word.
    fn commanding(&self) -> bool {
        let mode = match self.focus {
            Focus::Prompt => self.mode(),
            Focus::History => self.panel.mode(),
        };

        matches!(mode, VimMode::Normal | VimMode::Visual)
    }

    /// Runs the key typed after `<C-W>`, which moves the keys up to the history, down to the
    /// prompt, or to whichever of the two does not have them.
    fn window(&mut self, area: Rect, key: KeyEvent) {
        let focus = match key.code {
            KeyCode::Char('k') | KeyCode::Up => Focus::History,
            KeyCode::Char('j') | KeyCode::Down => Focus::Prompt,
            KeyCode::Char('w' | 'W' | 'p') => self.other(),
            KeyCode::Esc => return,
            _ => {
                let keys = spelled(&[key.into()]);
                self.notice = Some(format!("`<C-W>{keys}` is bound to nothing"));

                return;
            }
        };
        self.focus_on(area, focus);
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
        self.fit(area);
        self.pump(area);
        let outcome = self.acted(area, event);
        self.pump(area);

        outcome
    }

    /// Hands the editor one of the events an application loop delivers, with the session left
    /// exactly as it was found.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys.
    fn acted(&mut self, area: Rect, event: &Event) -> Outcome {
        match event {
            Event::Key(key) => self.press(area, *key),
            Event::Paste(_) => {
                if let Some(put) = self.put.as_mut() {
                    put.behind.push_back(event.clone());

                    return self.settle(area);
                }
                self.notice = None;
                if Focus::History == self.focus {
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
            Event::Redraw => self.settle(area),
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
        let yanked = self.engine.register_file().yanked();
        if let Err(error) = self.panel.press(key) {
            self.notice = Some(error.to_string());
        } else if let Some(refusal) = self.panel.refusal() {
            self.notice = Some(refusal.to_string());
        } else if let Some(notice) = self.panel.notice() {
            self.notice = Some(notice.to_owned());
        }
        self.held = self.panel.selection();
        let addressed = self.file_yank(yanked);
        self.mirror(addressed);
        self.follow_panel(area);

        Outcome::Continues
    }

    /// Holds a put that reads the desktop's clipboard until the desktop has answered.
    ///
    /// The key is kept rather than typed, and the desktop is asked what it holds. Nothing waits on
    /// the answer: the frames go on being drawn, and [`App::settle`] runs the put on the frame the
    /// answer arrives at. A put that reads any other register, and every keystroke of an editor
    /// that was given no clipboard, goes to the engine as it always did -- which is what keeps a
    /// plain `p` from ever asking the desktop anything.
    ///
    /// # Returns
    ///
    /// Whether the key was held.
    fn awaited(&mut self, key: KeyEvent) -> bool {
        let Some(clipboard) = self.clipboard.as_mut() else {
            return false;
        };
        let Some(name) = self.engine.pasted_register(key) else {
            return false;
        };
        if !Bridge::serves(name) {
            return false;
        }

        clipboard.read();
        self.put = Some(Put {
            key,
            behind: VecDeque::new(),
        });

        true
    }

    /// Runs a held put once the desktop has answered it, or abandons it once it has taken too
    /// long, and then types every key that was held behind it.
    ///
    /// What an abandoned put inserts is nothing at all. The register is left holding what the
    /// desktop handed over, which for a read that missed its deadline is nothing, so the put runs
    /// and puts nothing rather than putting whatever the register held before the reader asked --
    /// a paste of stale text being the one answer worse than no paste.
    ///
    /// # Returns
    ///
    /// Whether the application goes on reading keys, which what was held behind the put may say it
    /// does not: a `:qa` typed while the desktop was being waited on is a `:qa`.
    fn settle(&mut self, area: Rect) -> Outcome {
        let Some(clipboard) = self.clipboard.as_mut() else {
            return Outcome::Continues;
        };
        if self.put.is_none() {
            return Outcome::Continues;
        }

        let notice = match clipboard.settled() {
            Settled::Waiting => return Outcome::Continues,
            Settled::Slow(notice) => {
                if Some(notice) != self.notice.as_deref() {
                    self.notice = Some(notice.to_owned());
                    self.refreshed = true;
                }

                return Outcome::Continues;
            }
            Settled::Ready(notice) => notice,
        };
        let Some(put) = self.put.take() else {
            return Outcome::Continues;
        };
        self.notice = notice;
        self.refreshed = true;
        self.dispatch(area, |engine| engine.press(put.key));
        self.follow(area);
        for event in put.behind {
            if Outcome::Stops == self.handle(area, &event) {
                return Outcome::Stops;
            }
        }

        Outcome::Continues
    }

    /// Writes what the clipboard's register holds out to the desktop, where the keystroke just run
    /// left it holding something new.
    ///
    /// `addressed` is the register that keystroke named. What the history panel files in `"+`
    /// itself names no register, and the register file's own count is what finds that.
    fn mirror(&mut self, addressed: Option<char>) {
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        clipboard.mirror(addressed);
        if let Some(refusal) = clipboard.refusal() {
            self.notice = Some(refusal);
        }
    }

    /// Files a yank the history panel ran without naming a register into the clipboard's register
    /// as well, as vim does with `'clipboard'` set to `unnamedplus`.
    ///
    /// # Returns
    ///
    /// The register the last yank run since `since` named, and [`None`] where no yank has run
    /// since or the one that did named no register.
    fn file_yank(&self, since: Yanked) -> Option<char> {
        let registers = self.engine.register_file();
        let yanked = registers.yanked();
        if since == yanked {
            return None;
        }
        if yanked.register.is_some() {
            return yanked.register;
        }
        if let Some(held) = registers.get(YANK) {
            registers.fill(CLIPBOARD, &held);
        }

        None
    }

    /// Reads whatever the session has said since the last frame into the panel.
    ///
    /// Nothing here waits. A turn takes as long as a model takes and the keys go on arriving
    /// through all of it, so what is read is whatever has landed and the frame is drawn over that.
    /// A frame that would draw what the last one drew rebuilds nothing, so reading a session that
    /// has stopped talking costs what reading a compiled-in exchange costs; a frame that would
    /// differ costs the conversation, because the panel is built over a transcript rather than
    /// appended to, and it is drawn from its first row afterwards -- or from its last, where the
    /// history is drawn beside a prompt that has the keys.
    fn pump(&mut self, area: Rect) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.read();
        if session.revision() == self.drawn {
            return;
        }
        self.drawn = session.revision();
        let waiting = waited(session);
        let (transcript, tags) = session.panel();

        self.waiting = waiting;
        self.adopt_conversation(transcript, tags);
        self.fit(area);
        if self.split && Focus::Prompt == self.focus {
            self.tail_panel(area);
        } else {
            self.follow_panel(area);
        }
        self.refreshed = true;
    }

    /// Builds the transcript panel over what was said and the calls its blocks arrived beneath,
    /// leaving it drawn from its first row.
    fn adopt_conversation(&mut self, transcript: Transcript, tags: Vec<Tag>) {
        self.panel = Panel::new(transcript)
            .sharing(self.engine.register_file().clone())
            .tagged(tags);
        self.top = Placed::top(0);
        self.held = None;
        self.fitted = None;
    }

    /// Lays the history panel out in the area a frame is drawn into, where it was last laid out
    /// in another or has not been laid out at all.
    fn fit(&mut self, area: Rect) {
        if Some(area) == self.fitted {
            return;
        }
        if let Some(geometry) = self.panel_geometry(area) {
            self.panel.resize(geometry);
        }
        self.fitted = Some(area);
    }

    /// Scrolls the history panel to the last row of what was said, with its cursor on the last
    /// line, so that what arrived last is drawn along the bottom of the panel.
    fn tail_panel(&mut self, area: Rect) {
        if let Err(error) = self.panel.press(typed('G')) {
            self.notice = Some(error.to_string());
        }
        let Some(mut top) = self.panel.last() else {
            return;
        };
        for _ in 1..self.layout(area).history.height {
            let Some(above) = self.panel.above(top) else {
                break;
            };
            top = above;
        }
        self.top = top;
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
        let rows = usize::from(self.layout(area).history.height);
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
    /// The geometry the transcript panel is laid out in, which is the whole of the area the layout
    /// gives the history because a transcript is drawn without a gutter, or [`None`] where the
    /// area is too small to draw a column of text or a row of one in.
    fn panel_geometry(&self, area: Rect) -> Option<Geometry> {
        let text = self.layout(area).history;

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
    /// no row. The conversation screen's says what the session's turn is doing beside it, and
    /// along the right the model and the identifier the session runs under, as much of those two
    /// as the row has room for.
    fn draw_status(&self, cells: &mut Cells, area: Rect) {
        if area.is_empty() {
            return;
        }
        for x in area.x..area.right() {
            cells[(x, area.y)].reset();
        }
        let width = usize::from(area.width);
        if !self.split || self.command_line.is_some() {
            cells.set_stringn(area.x, area.y, self.status(), width, Style::default());

            return;
        }

        let mut said = self.status().to_owned();
        if let Some(turn) = self.turn() {
            if !said.is_empty() {
                said.push_str(GAP);
            }
            said.push_str(&turn);
        }
        cells.set_stringn(area.x, area.y, &said, width, Style::default());

        let Some(session) = self.session.as_ref() else {
            return;
        };
        let used = self.columns(&said) + self.columns(GAP);
        let id = session.id();
        let named = session.model().map(|model| format!("{model}{GAP}{id}"));
        for right in named.iter().map(String::as_str).chain([id]) {
            let needed = self.columns(right);
            if used + needed <= width {
                cells.set_stringn(
                    area.right() - narrowed(needed),
                    area.y,
                    right,
                    needed,
                    Style::default(),
                );

                return;
            }
        }
    }

    /// Draws the row between the history and the prompt, with the mark in it naming the panel that
    /// has the keys, which is nothing at all where the layout left no row for it.
    fn draw_divider(&self, cells: &mut Cells, area: Rect) {
        if area.is_empty() {
            return;
        }
        for x in area.x..area.right() {
            cells[(x, area.y)].reset();
        }
        let width = usize::from(area.width);
        cells.set_stringn(area.x, area.y, DIVIDER.repeat(width), width, DIVIDER_STYLE);
        let mark = match self.focus {
            Focus::History => HISTORY_MARK,
            Focus::Prompt => PROMPT_MARK,
        };
        let indent = MARK_INDENT.min(area.width);
        cells.set_stringn(
            area.x + indent,
            area.y,
            mark,
            usize::from(area.width - indent),
            MARK_STYLE,
        );
    }

    /// Runs one keystroke against the engine, laid out in the window `area` draws and scrolled to
    /// where the window is, and reads back what it left behind.
    ///
    /// The window is handed over as well as its size because `H`, `M` and `L` name a line of the
    /// window rather than a line of the text, and the window a reader typed at is the one they
    /// were looking at when they typed.
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
        self.engine.scrolled_to(self.viewport);
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
    /// Whether `key` is one vim reads a further key after that this editor implements nothing for,
    /// so that the further key is the application's to consume rather than the engine's to run.
    fn takes_argument(&self, key: KeyEvent) -> bool {
        Some(Argument::Unimplemented) == self.engine.argument(key.into())
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
        let rows = geometry.window().height().get();
        if self.split && self.drafted_rows(geometry.columns(), rows) <= rows {
            self.viewport = Viewport::new();

            return;
        }
        let screen = Screen::of(&self.text, &self.viewport, self.cursor, &geometry);
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
    /// Where each part of a frame drawn into `area` goes. The status line takes the bottom row,
    /// where the application draws one and the area has a row to spare. Laid out as the
    /// conversation screen, the prompt is as tall as its draft is drawn, never shorter than
    /// [`PROMPT_FLOOR`] rows nor taller than [`PROMPT_PERCENT`] of the area, a row divides it from
    /// the history, and the history takes what is left above them. Laid out otherwise, the history
    /// and the prompt are each the whole of what is above the status line, and only the one that
    /// has the keys is drawn.
    fn layout(&self, area: Rect) -> Layout {
        let (body, status) = if !self.status || area.height < 2 {
            (area, Rect::ZERO)
        } else {
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
        };
        if !self.split {
            return Layout {
                history: body,
                divider: Rect::ZERO,
                prompt: body,
                status,
            };
        }

        let cap = PROMPT_FLOOR.max(usize::from(area.height) * PROMPT_PERCENT / 100);
        let wanted = usize::from(body.width)
            .checked_sub(self.gutter_columns())
            .and_then(NonZeroUsize::new)
            .map_or(PROMPT_FLOOR, |columns| self.drafted_rows(columns, cap));
        let prompt = narrowed(wanted.clamp(PROMPT_FLOOR, cap)).min(body.height);
        let rest = body.height - prompt;
        let divider = u16::from(2 <= rest);
        let history = rest - divider;

        Layout {
            history: Rect {
                height: history,
                ..body
            },
            divider: Rect {
                y: body.y + history,
                height: divider,
                ..body
            },
            prompt: Rect {
                y: body.y + history + divider,
                height: prompt,
                ..body
            },
            status,
        }
    }

    /// # Returns
    ///
    /// The rows the draft is drawn in when it is wrapped into `columns`, counted from its first
    /// row and no further than one row past `cap`.
    fn drafted_rows(&self, columns: NonZeroUsize, cap: usize) -> usize {
        let geometry = Geometry::new(columns, NonZeroUsize::MIN.saturating_add(cap))
            .with_metrics(self.metrics)
            .with_options(self.options.clone());

        Screen::of(&self.text, &Viewport::new(), self.cursor, &geometry)
            .rows()
            .len()
    }

    /// # Returns
    ///
    /// The geometry the draft is laid out to in `body`, which is the columns the gutter leaves and
    /// every row, or [`None`] where it is too small to draw a column of text or a row of one in.
    fn text_geometry(&self, body: Rect) -> Option<Geometry> {
        let columns = usize::from(body.width).checked_sub(self.gutter_columns())?;

        Some(
            Geometry::new(
                NonZeroUsize::new(columns)?,
                NonZeroUsize::new(usize::from(body.height))?,
            )
            .with_metrics(self.metrics)
            .with_options(self.options.clone())
            .with_scrolloff(self.scrolloff),
        )
    }

    /// # Returns
    ///
    /// The display columns `text` is drawn in, measured as the layout measures a row.
    fn columns(&self, text: &str) -> usize {
        let mut column = 0;
        for grapheme in graphemes(text) {
            column += self.metrics.grapheme_width(grapheme, column);
        }

        column
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
/// What the status line says about a key vim reads a further key after that this editor
/// implements nothing for, whose further key was taken rather than run.
fn unimplemented(keys: &str) -> String {
    format!("`{keys}` takes a key after it that this editor does not implement")
}

/// # Returns
///
/// Whether `key` is the interrupt a terminal sends, which stops the session's turn from any panel
/// and any mode.
fn interrupts(key: KeyEvent) -> bool {
    KeyCode::Char('c') == key.code && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// # Returns
///
/// Whether `key` moves the keys between the prompt and the history, which `<C-T>` does from
/// either of them and from any mode, because a panel that could only be reached from normal mode
/// is a panel insert mode hides.
fn transcribes(key: KeyEvent) -> bool {
    KeyCode::Char('t') == key.code && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// # Returns
///
/// Whether `key` is `<C-W>`, which moves the keys between the panels by the key typed after it.
fn windows(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('w' | 'W')) && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// # Returns
///
/// Whether `key` opens the ex command line, which it does from the transcript panel as well as
/// from the file: the keys that answer a session are ex commands, and a panel a session cannot be
/// answered from is a panel that draws the question and nothing else.
fn commands(key: KeyEvent) -> bool {
    types(key) && KeyCode::Char(COMMAND) == key.code
}

/// # Returns
///
/// What the status line says about a session that is not simply running: what went wrong with it,
/// or what it has stopped and is waiting to be answered, or [`None`] where it is running and
/// waiting on nothing.
fn waited(session: &Session) -> Option<String> {
    if let Some(failure) = session.failure() {
        return Some(failure.to_owned());
    }
    let ask = session.outstanding().first()?;

    Some(format!(
        "waiting on `{}` -- `:allow` or `:deny`",
        ask.tool()
    ))
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
