//! vimbecode: a wrapped, scrollable, editable view of a text in a terminal, beside a Claude Code
//! session read in the same window.
//!
//! The program reads a file named on the command line, or a built-in passage where none is, draws
//! it through the editor, and types vim's own keys at it until `:q` or the interrupt ends it.
//! Everything it draws and edits with is the library's: the binary contributes the terminal it
//! draws into, the keys it reads, and the command line it is started from.
//!
//! `<C-T>` moves the keys to the transcript, which is read rather than written: `yac` takes the
//! code that was fenced, `yad` takes an edit as the patch it was, `za` folds away what a tool
//! wrote, and `x` says why it will not. With no arguments the transcript is a compiled-in exchange,
//! because a binary that could only show a transcript it was handed is a binary nobody can see one
//! in. With `--session` or `--resume` it is a real Claude Code session instead: `:ask` says
//! something to it, what it answers arrives in the panel as it is said, and `:allow` and `:deny`
//! answer what it stops to ask before it will go on.
//!
//! A session is started only after the directory it would run in has been through the trust gate,
//! and the question is put here rather than inside the editor because it has to be answered before
//! the child exists. Headless Claude Code has no workspace-trust dialog of its own: in a directory
//! it has never seen it reads the project's memory, runs the project's session hook and starts the
//! project's MCP servers without asking. A directory the reader does not trust is run on their own
//! settings and none of the project's, which is what a question nobody answered has to count as.
//!
//! The terminal is put back the way it was found on every exit, including the one an error takes,
//! because a program that leaves a terminal in raw mode leaves a shell nobody can type in.

use std::error::Error;
use std::io::{self, Stdout};
use std::path::PathBuf;
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
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::{Plan, Session};
use vbc_editor::session::trust::{Admission, Answer, Gate, Standing};
use vbc_layout::buffer::Buffer;

/// What the program says about how it is started.
const USAGE: &str = "\
usage: vimbecode [options] [file]

    -s, --session            read a new Claude Code session in the transcript panel
    -r, --resume <id>        read the session named <id>, resumed, in the panel
    -C, --directory <path>   run the session in <path> rather than in this directory
    -m, --model <name>       run the session on <name> rather than on its own choice
    -h, --help               say this and stop

With neither -s nor -r the panel shows a built-in exchange and no session is started.
In the panel: `:ask <text>` says something to the session, `:allow` and `:deny` answer
what it is waiting on, and `:answer <text>` answers a question it asked in words.";

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

/// The exchange the panel shows where no session was asked for, which is a short one of each kind
/// of block there is so that the keys a transcript answers have something to answer over: `yac`
/// over the fenced code, `yad` over the diff, `yat` over what the tool wrote, and `za` over the
/// fold the tool result heads.
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

/// What the reader is asked before a session is started in a directory they have not trusted, and
/// the answers that count as yes. Everything else, the empty line included, counts as no.
const QUESTION: &str = "\
A session started there runs the project's own code: the CLAUDE.md it ships, the hooks its
settings declare and the MCP servers its .mcp.json starts. Claude Code asks nothing about this
when it is driven the way vimbecode drives it, so this asks instead.";
const GRANTS: [&str; 2] = ["y", "yes"];

/// # Returns
///
/// [`ExitCode::SUCCESS`] if the program took the terminal over, drew into it and gave it back,
/// and [`ExitCode::FAILURE`] otherwise.
fn main() -> ExitCode {
    let arguments = match Arguments::read(std::env::args().skip(1)) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("vimbecode: {error}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if arguments.helped {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let app = match open(&arguments) {
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

/// What the command line asked for.
struct Arguments {
    path: Option<PathBuf>,
    identity: Option<Identity>,
    directory: Option<PathBuf>,
    model: Option<String>,
    helped: bool,
}

impl Arguments {
    /// # Returns
    ///
    /// What the command line asked for, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`String`] if a flag was one the program does not answer, was given no value where it
    ///   takes one, or was given a second file to edit.
    fn read<GivenArguments: Iterator<Item = String>>(
        mut given: GivenArguments,
    ) -> Result<Self, String> {
        let mut read = Self {
            path: None,
            identity: None,
            directory: None,
            model: None,
            helped: false,
        };

        while let Some(argument) = given.next() {
            let mut valued = |flag: &str| given.next().ok_or(format!("`{flag}` takes a value"));
            match argument.as_str() {
                "-h" | "--help" => read.helped = true,
                "-s" | "--session" => read.identity = Some(Identity::Fresh(SessionId::generated())),
                "-r" | "--resume" => {
                    read.identity = Some(Identity::Resumed(SessionId::known(valued(&argument)?)));
                }
                "-C" | "--directory" => read.directory = Some(PathBuf::from(valued(&argument)?)),
                "-m" | "--model" => read.model = Some(valued(&argument)?),
                flag if flag.starts_with('-') && "-" != flag => {
                    return Err(format!("`{flag}` is not an option this program answers"));
                }
                _ if read.path.is_some() => {
                    return Err(format!("`{argument}` is a second file to edit"));
                }
                _ => read.path = Some(PathBuf::from(argument)),
            }
        }

        Ok(read)
    }
}

/// # Returns
///
/// The editor over the file the command line named, or over [`PASSAGE`] where it named none,
/// showing the session it asked for or the built-in exchange where it asked for none, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`App::opened`]'s return values on failure.
/// * Forwards [`std::env::current_dir`]'s return values on failure.
/// * Forwards [`Gate::of_reader`]'s return values on failure.
/// * Forwards [`Session::opened`]'s return values on failure.
fn open(arguments: &Arguments) -> Result<App, Box<dyn Error>> {
    let app = match &arguments.path {
        Some(path) => App::opened(path.clone())?,
        None => App::new(Buffer::from_text(PASSAGE.trim_end_matches('\n'))),
    }
    .with_status(true);

    let Some(identity) = arguments.identity.clone() else {
        return Ok(app.with_transcript(said()));
    };

    let directory = match &arguments.directory {
        Some(directory) => directory.clone(),
        None => std::env::current_dir()?,
    };
    let mut plan = Plan::new(identity, directory, Gate::of_reader()?);
    if let Some(model) = &arguments.model {
        plan = plan.with_model(model.clone());
    }

    let session = Session::opened(&plan, trusted)?;
    if Standing::Restricted == session.standing() {
        eprintln!(
            "vimbecode: running {} on your own settings and none of the project's.",
            plan.directory().display()
        );
    }

    Ok(app.with_session(session))
}

/// Puts to the reader the question of whether a directory may run its own code, on the terminal
/// they started the program at and before that program has taken it over.
///
/// # Returns
///
/// What they answered, which is [`Answer::Withheld`] for everything but a yes -- an empty line, an
/// input that has ended, and an input that could not be read among them.
fn trusted(admission: &Admission) -> Answer {
    eprintln!(
        "vimbecode: you have not trusted {}.\n{QUESTION}\nTrust it, and record that you did? [y/N] ",
        admission.directory().display()
    );

    let mut answered = String::new();
    if io::stdin().read_line(&mut answered).is_err() {
        return Answer::Withheld;
    }
    if GRANTS.contains(&answered.trim().to_lowercase().as_str()) {
        return Answer::Granted;
    }

    Answer::Withheld
}

/// # Returns
///
/// The exchange the transcript panel shows where no session was asked for.
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
/// A frame is drawn for every event but the timer's own tick, because a terminal written to sixty
/// times a second is a terminal nothing else can read -- and for a tick that a session said
/// something during, because a tick is the only event a turn nobody is typing through arrives on.
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
        let refreshed = app.refreshed();
        if Event::Redraw != event || refreshed {
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
