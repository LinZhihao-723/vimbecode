//! vimbecode: a Claude Code session held in a terminal, whose next message is written in vim.
//!
//! The program starts a session in the directory it is run in, or resumes the one it is named in
//! the directory that one was started in, and draws the conversation over it: what was said in the
//! history panel, the draft of the next message in the prompt panel below it, and the status bar
//! along the bottom. Everything it draws and edits with is the library's: the binary contributes
//! the terminal it draws into, the keys it reads, and the command line it is started from.
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
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use vbc_editor::app::{App, Outcome};
use vbc_editor::event::reader::TerminalReader;
use vbc_editor::event::{Config, Event, Source};
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::{Plan, Session};
use vbc_editor::session::stored::{Store, Stored};
use vbc_editor::session::trust::{Admission, Answer, Gate, Standing};

/// What the program says about how it is started.
const USAGE: &str = "\
usage: vimbecode [options]

    -r, --resume <id>        resume the session named <id>, with what it already said, in the
                             directory it was started in, rather than start a new one
    -C, --directory <path>   run the session in <path> rather than in this directory, or in
                             the one a resumed session was started in
    -m, --model <name>       run the session on <name> rather than on its own choice
    -h, --help               say this and stop

The next message is written in the prompt panel: `:wq` sends it, `:q!` discards it and `:qa`
leaves. `<C-W>k` and `<C-W>j` move between the history and the prompt, `<C-C>` interrupts the
turn, and `:allow`, `:deny` and `:answer <text>` answer what the session is waiting on.";

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

    let mut app = match open(&arguments) {
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
    let result = run(&mut terminal, &mut app);
    let left = leave(terminal);
    if let Some(session) = app.session() {
        eprintln!(
            "vimbecode: resume this session with `vimbecode -r {}`",
            session.id()
        );
    }

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
    identity: Identity,
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
    /// * [`String`] if an argument was one the program does not answer, or a flag was given no
    ///   value where it takes one.
    fn read<GivenArguments: Iterator<Item = String>>(
        mut given: GivenArguments,
    ) -> Result<Self, String> {
        let mut read = Self {
            identity: Identity::Fresh(SessionId::generated()),
            directory: None,
            model: None,
            helped: false,
        };

        while let Some(argument) = given.next() {
            let mut valued = |flag: &str| given.next().ok_or(format!("`{flag}` takes a value"));
            match argument.as_str() {
                "-h" | "--help" => read.helped = true,
                "-r" | "--resume" => {
                    read.identity = Identity::Resumed(SessionId::known(valued(&argument)?));
                }
                "-C" | "--directory" => read.directory = Some(PathBuf::from(valued(&argument)?)),
                "-m" | "--model" => read.model = Some(valued(&argument)?),
                _ => {
                    return Err(format!(
                        "`{argument}` is not an argument this program answers"
                    ))
                }
            }
        }

        Ok(read)
    }
}

/// # Returns
///
/// The conversation screen over the session the command line asked for, on success: a new one
/// started in the directory it named or in this one, or a resumed one started in the directory it
/// named or in the one it was first started in, with what it already said read in ahead of it.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::env::current_dir`]'s return values on failure.
/// * Forwards [`Store::of_reader`]'s return values on failure.
/// * Forwards [`Store::find`]'s return values on failure.
/// * Forwards [`placed`]'s return values on failure.
/// * Forwards [`Gate::of_reader`]'s return values on failure.
/// * Forwards [`Session::opened`]'s return values on failure.
fn open(arguments: &Arguments) -> Result<App, Box<dyn Error>> {
    let here = std::env::current_dir()?;
    let named = arguments.directory.as_deref();
    let (directory, history) = match &arguments.identity {
        Identity::Resumed(id) => {
            let stored = Store::of_reader()?.find(id, named.unwrap_or(&here))?;
            (placed(id, &stored, named)?, Some(stored))
        }
        Identity::Fresh(_) | Identity::Forked(_) => (named.map_or(here, Path::to_owned), None),
    };
    let mut plan = Plan::new(arguments.identity.clone(), directory, Gate::of_reader()?);
    if let Some(model) = &arguments.model {
        plan = plan.with_model(model.clone());
    }
    if let Some(stored) = history {
        plan = plan.with_history(stored);
    }

    let session = Session::opened(&plan, trusted)?;
    if Standing::Restricted == session.standing() {
        eprintln!(
            "vimbecode: running {} on your own settings and none of the project's.",
            plan.directory().display()
        );
    }

    Ok(App::chat().with_session(session))
}

/// Decides the directory a resumed session runs in, and says which: the one `named` on the command
/// line, or else the one the session was started in, which is where Claude Code finds it again.
///
/// # Returns
///
/// The directory on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`String`] if no directory was named and the transcript names none, or the directory decided
///   on is not a directory.
/// * Forwards [`Stored::directory`]'s return values on failure.
fn placed(
    id: &SessionId,
    stored: &Stored,
    named: Option<&Path>,
) -> Result<PathBuf, Box<dyn Error>> {
    let started = stored.directory()?;
    let directory = match (named, started) {
        (None, None) => {
            return Err(format!(
                "`{}` names no directory session `{id}` was started in; name one with \
                 -C",
                stored.path().display()
            )
            .into())
        }
        (None, Some(started)) => {
            eprintln!(
                "vimbecode: resuming {id} in {}, where it was started.",
                started.display()
            );
            started
        }
        (Some(named), started) => {
            let elsewhere = started.filter(|started| {
                std::fs::canonicalize(named).ok() != std::fs::canonicalize(started).ok()
            });
            match elsewhere {
                Some(started) => eprintln!(
                    "vimbecode: resuming {id} in {}, as -C asked, rather than in {}, where it was \
                     started.",
                    named.display(),
                    started.display()
                ),
                None => eprintln!("vimbecode: resuming {id} in {}.", named.display()),
            }
            named.to_owned()
        }
    };
    if !directory.is_dir() {
        return Err(format!(
            "session `{id}` was started in `{}`, which is not a directory any more; name one \
             with -C",
            directory.display()
        )
        .into());
    }

    Ok(directory)
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

/// Draws the application and hands it every event until one of them ends the program.
///
/// The first frame is drawn after the application has been handed the timer's tick once, which is
/// what lays it out in the terminal it is drawn into before anything is typed. A frame is drawn
/// for every event but the timer's own tick, because a terminal written to sixty times a second
/// is a terminal nothing else can read -- and for a tick that a session said something during,
/// because a tick is the only event a turn nobody is typing through arrives on.
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
    app: &mut App,
) -> Result<(), Box<dyn Error>> {
    let events = Source::start(TerminalReader::new(), Config::default());
    app.handle(area(terminal)?, &Event::Redraw);
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
