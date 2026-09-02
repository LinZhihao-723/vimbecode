//! vimbecode: a wrapped, scrollable view of a text in a terminal.
//!
//! The program is the smallest one that is a program rather than a probe. It reads a file named on
//! the command line, or a built-in passage where none is, draws it through the editor, and scrolls
//! it with vim's own scrolling keys until `q` ends it. Everything it draws with is the library's:
//! the binary contributes the terminal it draws into and the keys it reads, and nothing else.
//!
//! The terminal is put back the way it was found on every exit, including the one an error takes,
//! because a program that leaves a terminal in raw mode leaves a shell nobody can type in.

use std::error::Error;
use std::io::{self, Stdout};
use std::process::ExitCode;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use vbc_editor::app::App;
use vbc_editor::event::reader::TerminalReader;
use vbc_editor::event::{Config, Event, Source};
use vbc_layout::buffer::Buffer;
use vbc_layout::viewport::Command;

/// The passage the program shows when it is started without a file, chosen to wrap: its lines are
/// longer than a terminal is wide and its text is a width no terminal measures by counting
/// characters.
const PASSAGE: &str = "\
中文的段落在窄窗口里会折行，行号只出现在第一行上。
The gutter numbers logical lines, so the rows that continue one are left blank.
混合了 ASCII 和中文的一行也照样折行，字宽由布局引擎测量。
Press CTRL-D and CTRL-U to scroll half a window, CTRL-E and CTRL-Y one row.
每一段都是一个逻辑行，可能会占据屏幕上的很多行。
Press q to quit.";

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

    Ok(App::new(Buffer::from_text(text.trim_end_matches('\n'))))
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
/// * Forwards [`crossterm::terminal::disable_raw_mode`]'s return values on failure.
/// * Forwards [`ratatui::Terminal::show_cursor`]'s return values on failure.
fn leave(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<(), Box<dyn Error>> {
    vbc_editor::event::reader::disable_bracketed_paste()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    terminal.show_cursor()?;

    Ok(())
}

/// Draws the editor and scrolls it until a key ends the program.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`ratatui::Terminal::draw`]'s return values on failure.
/// * Forwards [`App::scroll`]'s return values on failure.
fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
) -> Result<(), Box<dyn Error>> {
    let events = Source::start(TerminalReader::new(), Config::default());
    terminal.draw(|frame| app.render(frame))?;

    while let Ok(event) = events.recv() {
        match event {
            Event::Key(key) if quits(key) => break,
            Event::Key(key) => {
                if let Some(command) = scrolled_by(key) {
                    app.scroll(area(terminal)?, command)?;
                    terminal.draw(|frame| app.render(frame))?;
                }
            }
            Event::Resize { .. } => {
                terminal.autoresize()?;
                terminal.draw(|frame| app.render(frame))?;
            }
            Event::Paste(_) | Event::Redraw | Event::Notice(_) => {}
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

/// # Returns
///
/// Whether `key` ends the program, which `q` does and so does the interrupt a terminal sends when
/// nothing has taken the keyboard over.
fn quits(key: KeyEvent) -> bool {
    let interrupted =
        KeyCode::Char('c') == key.code && key.modifiers.contains(KeyModifiers::CONTROL);

    KeyCode::Char('q') == key.code || interrupted
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
