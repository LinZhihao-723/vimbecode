//! vimbecode: a wrapped, scrollable, editable view of a text in a terminal.
//!
//! The program is the smallest one that is a program rather than a probe. It reads a file named on
//! the command line, or a built-in passage where none is, draws it through the editor, and types
//! vim's own keys at it until `q` ends it. Everything it draws and edits with is the library's:
//! the binary contributes the terminal it draws into and the keys it reads, and nothing else.
//!
//! `<C-T>` moves the keys to the transcript of an exchange, which is read rather than written:
//! `yac` takes the code that was fenced, `yad` takes an edit as the patch it was, `za` folds away
//! what a tool wrote, and `x` says why it will not. The exchange is a built-in one, because a
//! binary that could only show a transcript it was handed is a binary nobody can see one in.
//!
//! The terminal is put back the way it was found on every exit, including the one an error takes,
//! because a program that leaves a terminal in raw mode leaves a shell nobody can type in.

use std::error::Error;
use std::io::{self, Stdout};
use std::process::ExitCode;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use vbc_editor::app::{App, Outcome};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::event::reader::TerminalReader;
use vbc_editor::event::{Config, Event, Source};
use vbc_layout::buffer::Buffer;

/// The passage the program shows when it is started without a file, chosen to wrap: its lines are
/// longer than a terminal is wide and its text is a width no terminal measures by counting
/// characters.
const PASSAGE: &str = "\
中文的段落在窄窗口里会折行，行号只出现在第一行上。
The gutter numbers logical lines, so the rows that continue one are left blank.
混合了 ASCII 和中文的一行也照样折行，字宽由布局引擎测量。
Type vim's own keys: motions, counts, registers, operators such as dw and 3dd, and gj by row.
每一段都是一个逻辑行，可能会占据屏幕上的很多行。
Press CTRL-D and CTRL-U to scroll half a window, CTRL-E and CTRL-Y one row.
Press CTRL-T to read the transcript, and CTRL-T again to come back.
Press q to quit.";

/// The exchange the panel shows, which is a short one of each kind of block there is so that the
/// keys a transcript answers have something to answer over: `yac` over the fenced code, `yad`
/// over the diff, `yat` over what the tool wrote, and `za` over the fold the tool result heads.
const ASKED: &str = "add a todo to main, and show me the diff";
const ANSWERED: &str = "\
Here is the line to add:

```rust
fn main() {
    todo!();
}
```

I ran the build to check it.";
const THOUGHT: &str = "The file is tiny, so replacing the body is safe.";
const RAN: &str = "cargo build";
const WROTE: &str = "\
   Compiling vimbecode v0.0.0
    Finished `dev` profile in 0.42s";
const EDITED: &str = "src/main.rs";
const BEFORE: &str = "fn main() {}\n";
const AFTER: &str = "fn main() {\n    todo!();\n}\n";

/// # Returns
///
/// [`ExitCode::SUCCESS`] if the program took the terminal over, drew into it and gave it back,
/// and [`ExitCode::FAILURE`] otherwise.
fn main() -> ExitCode {
    let app = match open() {
        Ok(app) => app,
        Err(error) => {
            eprintln!("vimbecode: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut terminal = match enter() {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("vimbecode: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = run(&mut terminal, app);
    let left = leave(terminal);

    match result.and(left) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vimbecode: {error}");
            ExitCode::FAILURE
        }
    }
}

/// # Returns
///
/// The editor over the file named on the command line, or over [`PASSAGE`] where none is, on
/// success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
fn open() -> Result<App, Box<dyn Error>> {
    let text = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(path)?,
        None => PASSAGE.to_owned(),
    };

    Ok(App::new(Buffer::from_text(text.trim_end_matches('\n')))
        .with_status(true)
        .with_transcript(said()))
}

/// # Returns
///
/// The exchange the transcript panel shows.
fn said() -> Transcript {
    [
        Block::new(Kind::Message(Role::User), ASKED.to_owned()),
        Block::new(Kind::Thinking, THOUGHT.to_owned()),
        Block::new(Kind::Message(Role::Assistant), ANSWERED.to_owned()),
        Block::new(
            Kind::ToolCall {
                name: "Bash".to_owned(),
            },
            RAN.to_owned(),
        ),
        Block::from_ansi(Kind::ToolResult, WROTE),
        Block::diff(EDITED.to_owned(), BEFORE, AFTER),
    ]
    .into_iter()
    .collect()
}

/// Takes the terminal over: raw mode, a screen of its own, and bracketed paste.
///
/// # Returns
///
/// The terminal to draw into on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`crossterm::terminal::enable_raw_mode`]'s return values on failure.
/// * Forwards [`crossterm::execute`]'s return values on failure.
/// * Forwards [`vbc_editor::event::reader::enable_bracketed_paste`]'s return values on failure.
/// * Forwards [`ratatui::Terminal::new`]'s return values on failure.
fn enter() -> Result<Terminal<CrosstermBackend<Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    vbc_editor::event::reader::enable_bracketed_paste()?;

    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

/// Puts the terminal back the way it was found.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`vbc_editor::event::reader::disable_bracketed_paste`]'s return values on failure.
/// * Forwards [`crossterm::execute`]'s return values on failure.
/// * Forwards [`crossterm::terminal::disable_raw_mode`]'s return values on failure.
/// * Forwards [`ratatui::Terminal::show_cursor`]'s return values on failure.
fn leave(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<(), Box<dyn Error>> {
    vbc_editor::event::reader::disable_bracketed_paste()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    terminal.show_cursor()?;

    Ok(())
}

/// Draws the editor and hands it every event until one of them ends the program.
///
/// A frame is drawn for every event but the timer's own tick, because a tick changes nothing and a
/// terminal written to sixty times a second is a terminal nothing else can read.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`ratatui::Terminal::draw`]'s return values on failure.
/// * Forwards [`ratatui::Terminal::autoresize`]'s return values on failure.
/// * Forwards [`area`]'s return values on failure.
fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
) -> Result<(), Box<dyn Error>> {
    let events = Source::start(TerminalReader::new(), Config::default());
    terminal.draw(|frame| app.render(frame))?;

    while let Ok(event) = events.recv() {
        if let Event::Resize { .. } = event {
            terminal.autoresize()?;
        }
        let outcome = app.handle(area(terminal)?, &event);
        if Event::Redraw != event {
            terminal.draw(|frame| app.render(frame))?;
        }
        if Outcome::Stops == outcome {
            break;
        }
    }

    Ok(())
}

/// # Returns
///
/// The area a frame is drawn into, which is the whole of the terminal, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`ratatui::Terminal::size`]'s return values on failure.
fn area(terminal: &Terminal<CrosstermBackend<Stdout>>) -> Result<Rect, Box<dyn Error>> {
    let size = terminal.size()?;

    Ok(Rect::new(0, 0, size.width, size.height))
}
