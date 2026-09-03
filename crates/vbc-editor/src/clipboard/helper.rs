//! The clipboard helper's life: started with the session, kept for the whole of it, and let go at
//! the end of it.
//!
//! The helper is started before anything asks it for anything, because starting it is what costs.
//! PowerShell takes the better part of a second to come up and the WSL interop floor is sixty
//! milliseconds under that, so a helper started at the first paste puts all of that in front of the
//! paste, while a helper started with the session has already paid it. Launching therefore spawns
//! the process and exchanges one request with it, and what the editor's first paste pays is a pipe
//! round trip.
//!
//! What is kept is the process, not a promise that the clipboard works. Those are different things
//! and the difference is the whole of the restart policy. A locked workstation answers every
//! clipboard call with a refusal for as long as it stays locked, which is hours; the process
//! serving those refusals is healthy, is the same process that will serve the first paste after the
//! unlock, and restarting it accomplishes nothing but paying the startup cost again on the way to
//! the same refusal. So a refusal never restarts anything. A helper that has exited is the other
//! case, and that one is restarted, because there is nothing left to talk to.
//!
//! After a hard failure the clipboard is degraded, and a degraded clipboard is not retried for at
//! least [`BACKOFF_FLOOR`]. The floor is there because the failing calls are the slow ones: a
//! refusal costs over a second where a success costs a fraction of a millisecond, and a retry loop
//! over those turned a fifth of a millisecond into a thirteen second freeze once already. Inside
//! the floor a request is refused out of hand, in no time at all, which is the difference between
//! an editor that says the clipboard is unavailable and an editor that stops responding.

use std::error::Error as StdError;
use std::ffi::{OsStr, OsString};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::protocol::{self, Request, Response};

/// The shortest a degraded clipboard is left alone for, whatever it is configured with. A failing
/// clipboard call costs over a second, so this is what stands between one failure and an editor
/// that spends its time waiting on more of them.
pub const BACKOFF_FLOOR: Duration = Duration::from_secs(5);

/// The script the helper runs, which ships with the editor and is laid down where Windows can read
/// it without being able to hold it against us.
const HELPER_SCRIPT: &str = include_str!("helper.ps1");

/// The program the Windows clipboard is served by.
const POWERSHELL: &str = "powershell.exe";

/// The arguments it is started with. The profile is skipped because a user's profile is somebody
/// else's code in our pipe, and the execution policy is bypassed because a script read over the
/// WSL share is a script from another machine as far as Windows is concerned.
const POWERSHELL_ARGUMENTS: [&str; 4] = [
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
];

/// The program that says what Windows calls a path of this filesystem.
const PATH_TRANSLATOR: &str = "wslpath";

/// How long a helper is given to notice that its input has ended before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How often that wait looks.
const SHUTDOWN_POLL: Duration = Duration::from_millis(2);

/// What stopped an exchange with the helper from saying anything about the clipboard.
///
/// A clipboard that could not be read is not one of these. That answer arrives intact, as
/// [`Response::Failed`], and says why; these are the ways of not being answered at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// The helper could not be started.
    Spawn {
        /// The program that was to be started.
        program: String,

        /// What kind of failure starting it reported.
        kind: ErrorKind,

        /// What the system said about it.
        message: String,
    },

    /// The exchange with the helper broke, which is the helper being gone rather than the
    /// clipboard being unavailable.
    Broken(protocol::Error),

    /// The clipboard is degraded and this request was not put to it.
    Degraded {
        /// How long is left of the backoff.
        retry_in: Duration,
    },

    /// The helper script could not be put somewhere the helper could run it from.
    Script {
        /// The script's path.
        path: PathBuf,

        /// What went wrong with it.
        message: String,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Spawn {
                program,
                kind,
                message,
            } => write!(
                formatter,
                "{program} could not be started: {kind}: {message}"
            ),
            Self::Broken(error) => write!(formatter, "the helper stopped answering: {error}"),
            Self::Degraded { retry_in } => write!(
                formatter,
                "the clipboard is unavailable, and is not tried again for {retry_in:?}"
            ),
            Self::Script { path, message } => {
                write!(formatter, "the helper script at {path:?}: {message}")
            }
        }
    }
}

impl StdError for Error {}

impl From<protocol::Error> for Error {
    fn from(error: protocol::Error) -> Self {
        Self::Broken(error)
    }
}

/// The helper script, laid down where it is run from.
///
/// It lives on this filesystem rather than on Windows'. An unsigned file freshly written under
/// `C:\` is what Windows Defender exists to delete, and a helper that vanishes between sessions is
/// worse than one that is a little slower to read. It is taken away again when the session that
/// laid it down is done with it.
#[derive(Debug)]
pub struct Script {
    path: PathBuf,
}

impl Script {
    /// Writes the helper script into a directory of this filesystem.
    ///
    /// # Returns
    ///
    /// The script, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Script`] if the directory or the script could not be written.
    pub fn lay_down(directory: &Path) -> Result<Self, Error> {
        let path = directory.join(format!("vimbecode-clipboard-{}.ps1", std::process::id()));
        fs::create_dir_all(directory).map_err(|error| Error::Script {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?;
        fs::write(&path, HELPER_SCRIPT).map_err(|error| Error::Script {
            path: path.clone(),
            message: error.to_string(),
        })?;

        Ok(Self { path })
    }

    /// # Returns
    ///
    /// The path this filesystem knows the script by.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// # Returns
    ///
    /// The path Windows knows the script by, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Script`] if the translation could not be run, or reported a failure, or named
    ///   nothing.
    pub fn windows_path(&self) -> Result<OsString, Error> {
        let translated = Command::new(PATH_TRANSLATOR)
            .arg("-w")
            .arg(&self.path)
            .output()
            .map_err(|error| Error::Script {
                path: self.path.clone(),
                message: format!("{PATH_TRANSLATOR} could not be run: {error}"),
            })?;
        if !translated.status.success() {
            return Err(Error::Script {
                path: self.path.clone(),
                message: format!("{PATH_TRANSLATOR} failed: {}", translated.status),
            });
        }

        let windows = String::from_utf8_lossy(&translated.stdout)
            .trim_end()
            .to_owned();
        if windows.is_empty() {
            return Err(Error::Script {
                path: self.path.clone(),
                message: format!("{PATH_TRANSLATOR} named no Windows path"),
            });
        }

        Ok(windows.into())
    }
}

impl Drop for Script {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// How a helper is started, and how long a degraded clipboard is left alone for.
#[derive(Debug)]
pub struct Launch {
    program: OsString,
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    backoff: Duration,
    script: Option<Script>,
}

impl Launch {
    /// The launch of the PowerShell helper that serves the Windows clipboard, whose script is laid
    /// down in a directory of this filesystem.
    ///
    /// # Returns
    ///
    /// The launch, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Script::lay_down`]'s return values on failure.
    /// * Forwards [`Script::windows_path`]'s return values on failure.
    pub fn windows_clipboard(directory: &Path) -> Result<Self, Error> {
        let script = Script::lay_down(directory)?;
        let mut arguments: Vec<OsString> =
            POWERSHELL_ARGUMENTS.iter().map(OsString::from).collect();
        arguments.push("-File".into());
        arguments.push(script.windows_path()?);

        Ok(Self {
            script: Some(script),
            ..Self::of(POWERSHELL.into(), arguments)
        })
    }

    /// # Returns
    ///
    /// The launch of a program that speaks the clipboard protocol over its own pipes.
    pub fn of(program: OsString, arguments: Vec<OsString>) -> Self {
        Self {
            program,
            arguments,
            environment: Vec::new(),
            backoff: BACKOFF_FLOOR,
            script: None,
        }
    }

    /// # Returns
    ///
    /// This launch, with an environment variable set on the helper.
    #[must_use]
    pub fn with_environment(mut self, key: OsString, value: OsString) -> Self {
        self.environment.push((key, value));

        self
    }

    /// # Returns
    ///
    /// This launch, with the backoff a degraded clipboard is left alone for, which is never
    /// shorter than [`BACKOFF_FLOOR`] however short a one is asked for.
    #[must_use]
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff.max(BACKOFF_FLOOR);

        self
    }

    /// # Returns
    ///
    /// The backoff a degraded clipboard is left alone for.
    pub fn backoff(&self) -> Duration {
        self.backoff
    }

    /// # Returns
    ///
    /// The program the helper is.
    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// # Returns
    ///
    /// The arguments it is started with.
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// # Returns
    ///
    /// The script it runs, if it runs one.
    pub fn script(&self) -> Option<&Script> {
        self.script.as_ref()
    }

    /// # Returns
    ///
    /// The command this launch is, with the pipes the protocol is spoken over.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, value) in &self.environment {
            command.env(key, value);
        }

        command
    }
}

/// The clipboard, as one helper process kept for the life of a session.
#[derive(Debug)]
pub struct Helper {
    launch: Launch,
    running: Option<Running>,
    degraded_until: Option<Instant>,
    launches: u64,
}

impl Helper {
    /// Starts the helper and pays its startup cost, so that the first request the editor makes of
    /// it does not.
    ///
    /// # Returns
    ///
    /// The helper, on success. A helper whose clipboard refused the request it was primed with is
    /// returned degraded rather than not returned: the process is up, and the clipboard is what is
    /// unavailable.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Helper::start`]'s return values on failure.
    pub fn launch(launch: Launch) -> Result<Self, Error> {
        let mut helper = Self {
            launch,
            running: None,
            degraded_until: None,
            launches: 0,
        };
        helper.start()?;

        // Priming is for what the exchange costs, not for what it answers.
        let _ = helper.request(&Request::Read);

        Ok(helper)
    }

    /// Asks the helper what the clipboard holds.
    ///
    /// # Returns
    ///
    /// What the clipboard holds, on success, which includes [`Response::Failed`] for a clipboard
    /// that could not be read.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Helper::request`]'s return values on failure.
    pub fn read(&mut self) -> Result<Response, Error> {
        self.request(&Request::Read)
    }

    /// Asks the helper to put text on the clipboard.
    ///
    /// # Returns
    ///
    /// [`Response::Stored`] on success, or [`Response::Failed`] for a clipboard that could not be
    /// written.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Helper::request`]'s return values on failure.
    pub fn write(&mut self, text: String) -> Result<Response, Error> {
        self.request(&Request::Write(text))
    }

    /// Ends the helper's input, which is how it is asked to exit, and waits for it to. A helper
    /// that has not gone by [`SHUTDOWN_GRACE`] is killed, so that no session leaves one behind.
    pub fn shutdown(&mut self) {
        let Some(mut running) = self.running.take() else {
            return;
        };

        drop(running.input);
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            match running.child.try_wait() {
                Ok(None) => thread::sleep(SHUTDOWN_POLL),
                _ => return,
            }
        }

        let _ = running.child.kill();
        let _ = running.child.wait();
    }

    /// # Returns
    ///
    /// The number of helper processes this session has started, which is one for a session whose
    /// helper never died.
    pub fn launches(&self) -> u64 {
        self.launches
    }

    /// # Returns
    ///
    /// When the clipboard will be tried again, if it is degraded.
    pub fn degraded_until(&self) -> Option<Instant> {
        self.degraded_until
    }

    /// # Returns
    ///
    /// The process the helper is running as, if one is running.
    pub fn process_id(&self) -> Option<u32> {
        self.running.as_ref().map(|running| running.child.id())
    }

    /// Puts one request to the helper, restarting a helper that has died and degrading the
    /// clipboard after a failure.
    ///
    /// # Returns
    ///
    /// What the helper answered, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Degraded`] if the clipboard is degraded and the backoff has not run out, in
    ///   which case the helper is not spoken to at all.
    /// * Forwards [`Helper::attempt`]'s return values on failure.
    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        if let Some(until) = self.degraded_until {
            let now = Instant::now();
            if now < until {
                return Err(Error::Degraded {
                    retry_in: until - now,
                });
            }
            self.degraded_until = None;
        }

        let attempted = self.attempt(request);
        match &attempted {
            Ok(Response::Failed(_)) | Err(_) => self.degrade(),
            Ok(_) => {}
        }

        attempted
    }

    /// Puts one request to a running helper, starting one that is not running and replacing one
    /// that stopped answering part way through.
    ///
    /// # Returns
    ///
    /// What the helper answered, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Helper::start`]'s return values on failure.
    /// * Forwards [`Helper::exchange`]'s return values on failure.
    fn attempt(&mut self, request: &Request) -> Result<Response, Error> {
        if self.exited() {
            self.running = None;
        }
        if self.running.is_none() {
            self.start()?;
        }

        match self.exchange(request) {
            Ok(response) => Ok(response),
            Err(_) => {
                self.discard();
                self.start()?;
                self.exchange(request)
            }
        }
    }

    /// Writes a request to the helper and reads its answer.
    ///
    /// # Returns
    ///
    /// What the helper answered, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Broken`] if no helper is running.
    /// * Forwards [`Request::write_to`]'s return values on failure.
    /// * Forwards [`Response::read_from`]'s return values on failure.
    fn exchange(&mut self, request: &Request) -> Result<Response, Error> {
        let running = self.running.as_mut().ok_or_else(|| {
            Error::Broken(protocol::Error::Io {
                kind: ErrorKind::BrokenPipe,
                message: "no helper is running".to_owned(),
            })
        })?;

        request.write_to(&mut running.input)?;
        let response = Response::read_from(&mut running.output)?;

        Ok(response)
    }

    /// Starts a helper process.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Spawn`] if the helper could not be started.
    fn start(&mut self) -> Result<(), Error> {
        let mut child = self
            .launch
            .command()
            .spawn()
            .map_err(|error| Error::Spawn {
                program: self.launch.program.to_string_lossy().into_owned(),
                kind: error.kind(),
                message: error.to_string(),
            })?;

        let taken = child.stdin.take().zip(child.stdout.take());
        let Some((input, output)) = taken else {
            let _ = child.kill();
            let _ = child.wait();

            return Err(Error::Spawn {
                program: self.launch.program.to_string_lossy().into_owned(),
                kind: ErrorKind::BrokenPipe,
                message: "the helper was started without its pipes".to_owned(),
            });
        };

        self.launches += 1;
        self.running = Some(Running {
            child,
            input,
            output,
        });

        Ok(())
    }

    /// Kills the running helper and waits for it, so that a replaced helper is not left behind.
    fn discard(&mut self) {
        let Some(mut running) = self.running.take() else {
            return;
        };

        let _ = running.child.kill();
        let _ = running.child.wait();
    }

    /// Holds the clipboard off for the backoff this helper was launched with.
    fn degrade(&mut self) {
        self.degraded_until = Some(Instant::now() + self.launch.backoff);
    }

    /// # Returns
    ///
    /// Whether the helper process is gone, waiting for it if it is.
    fn exited(&mut self) -> bool {
        match self.running.as_mut() {
            None => false,
            Some(running) => matches!(running.child.try_wait(), Ok(Some(_))),
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A helper process and the pipes it is spoken to over.
#[derive(Debug)]
struct Running {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
}
