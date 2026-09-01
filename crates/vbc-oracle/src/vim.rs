//! The driver that replays key sequences against a real vim.
//!
//! A run launches vim with no user configuration and no plugins, feeds it a starting buffer and a
//! key sequence, and reports the [`EditorState`] vim ends in. vim never touches a terminal, so a
//! run works the same on a developer's machine and in CI.

use std::{
    collections::BTreeMap,
    env, fmt,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::state::{Cursor, EditorState, Mode, Register, RegisterName, RegisterType};

/// The oldest vim the driver accepts. Patch 8.2.1978 introduced the `<Cmd>` key, with which the
/// driver snapshots vim's state without disturbing the mode the key sequence ended in.
pub const MINIMUM_VERSION: VimVersion = VimVersion {
    major: 8,
    minor: 2,
    patch: 1978,
};

/// The version of a vim binary, as reported by `vim --version`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct VimVersion {
    /// The major version.
    pub major: u32,

    /// The minor version.
    pub minor: u32,

    /// The highest patch number included in the build, zero if the build reports no patches.
    pub patch: u32,
}

impl fmt::Display for VimVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A harness that replays key sequences against a real vim.
///
/// The driver is pinned to the binary it was created with, and reports the version of that binary
/// so a differential run can record which vim it was compared against.
#[derive(Clone, Debug)]
pub struct VimDriver {
    binary: PathBuf,
    version: VimVersion,
    timeout: Duration,
}

impl VimDriver {
    /// Factory function.
    ///
    /// Creates a driver bound to the `vim` binary found on `PATH`.
    ///
    /// # Returns
    ///
    /// A newly created driver on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`VimDriver::with_binary`]'s return values on failure.
    pub fn new() -> Result<Self, Error> {
        Self::with_binary("vim")
    }

    /// Factory function.
    ///
    /// Creates a driver bound to the given vim binary, rejecting a binary that is missing, is not
    /// vim, or is too old or too small a build to be driven.
    ///
    /// # Returns
    ///
    /// A newly created driver on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Launch`] if the binary could not be executed.
    /// * [`Error::UnrecognizedBinary`] if the binary did not report a vim version.
    /// * [`Error::UnsupportedVersion`] if the binary is older than [`MINIMUM_VERSION`].
    /// * [`Error::MissingFeature`] if the binary was built without a feature the driver needs.
    pub fn with_binary(binary: impl Into<PathBuf>) -> Result<Self, Error> {
        let binary = binary.into();
        let output = Command::new(&binary)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|source| Error::Launch {
                binary: binary.clone(),
                source,
            })?;
        let report = String::from_utf8_lossy(&output.stdout);

        let Some(version) = parse_version(&report) else {
            return Err(Error::UnrecognizedBinary {
                binary,
                version_output: report.lines().next().unwrap_or_default().to_owned(),
            });
        };
        if version < MINIMUM_VERSION {
            return Err(Error::UnsupportedVersion { binary, version });
        }
        for feature in REQUIRED_FEATURES {
            if !report.contains(&format!("+{feature}")) {
                return Err(Error::MissingFeature { binary, feature });
            }
        }

        Ok(Self {
            binary,
            version,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// # Returns
    ///
    /// The vim binary the driver runs.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// # Returns
    ///
    /// The version of the vim binary the driver runs.
    #[must_use]
    pub fn version(&self) -> VimVersion {
        self.version
    }

    /// Sets how long a single replay may take before vim is killed.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Replays a key sequence against a starting buffer.
    ///
    /// The starting buffer is opened as a file, so it is reported back with a trailing newline
    /// exactly when vim considers its last line newline-terminated. The keys are written in vim's
    /// key notation: `<Esc>`, `<C-r>` and `<Down>` name keys, `<lt>` names a literal `<`, and
    /// every other character stands for itself.
    ///
    /// Only the registers a differential run compares are reported: the unnamed and small-delete
    /// registers, the numbered registers, and the named registers `a` to `z`.
    ///
    /// # Returns
    ///
    /// The state vim ends in on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Io`] if the driver's temporary workspace could not be written or read.
    /// * [`Error::Launch`] if vim could not be started.
    /// * [`Error::Timeout`] if vim did not finish in time, which a key sequence that leaves vim
    ///   waiting for an argument (an unfinished `f` or `q`, for example) causes.
    /// * [`Error::Wait`] if vim could not be waited on.
    /// * [`Error::NoState`] if vim exited without reporting a state.
    /// * [`Error::MalformedState`] if the reported state could not be decoded.
    /// * [`Error::UnsupportedMode`] if vim ended in a mode [`Mode`] cannot represent.
    pub fn run(&self, buffer: &str, keys: &str) -> Result<EditorState, Error> {
        let workspace = Workspace::new()?;
        let buffer_path = workspace.path().join("buffer");
        let script_path = workspace.path().join("capture.vim");
        let state_path = workspace.path().join("state.json");
        let stderr_path = workspace.path().join("stderr");
        write_file(&buffer_path, buffer.as_bytes())?;
        write_file(&script_path, build_script(&state_path, keys).as_bytes())?;

        let stderr = File::create(&stderr_path).map_err(|source| Error::Io {
            path: stderr_path.clone(),
            source,
        })?;
        let child = Command::new(&self.binary)
            .args(VIM_ARGUMENTS)
            .args(["--cmd", PRE_COMMANDS])
            .arg("-S")
            .arg(&script_path)
            .arg(&buffer_path)
            .env_remove("VIMINIT")
            .env_remove("EXINIT")
            .env_remove("MYVIMRC")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|source| Error::Launch {
                binary: self.binary.clone(),
                source,
            })?;
        let status = wait_for_exit(child, self.timeout)?;

        let Ok(report) = fs::read_to_string(&state_path) else {
            return Err(Error::NoState {
                status,
                stderr: fs::read_to_string(&stderr_path).unwrap_or_default(),
            });
        };
        serde_json::from_str::<Report>(&report)
            .map_err(|source| Error::MalformedState {
                detail: source.to_string(),
            })?
            .into_state()
    }
}

/// The reason a driver could not be created, or a key sequence could not be replayed.
#[derive(Debug)]
pub enum Error {
    /// A file the driver reads or writes could not be accessed.
    Io {
        /// The file the driver failed on.
        path: PathBuf,

        /// The underlying failure.
        source: io::Error,
    },

    /// The vim binary could not be executed.
    Launch {
        /// The binary the driver tried to run.
        binary: PathBuf,

        /// The underlying failure.
        source: io::Error,
    },

    /// The binary ran but did not report a vim version.
    UnrecognizedBinary {
        /// The binary the driver tried to run.
        binary: PathBuf,

        /// The first line the binary printed for `--version`.
        version_output: String,
    },

    /// The binary is an older vim than [`MINIMUM_VERSION`].
    UnsupportedVersion {
        /// The binary the driver tried to run.
        binary: PathBuf,

        /// The version the binary reported.
        version: VimVersion,
    },

    /// The binary is a vim built without a feature the driver needs.
    MissingFeature {
        /// The binary the driver tried to run.
        binary: PathBuf,

        /// The name of the missing feature, without its `+` sign.
        feature: &'static str,
    },

    /// vim could not be waited on.
    Wait {
        /// The underlying failure.
        source: io::Error,
    },

    /// vim did not exit before the timeout elapsed and was killed.
    Timeout {
        /// The time vim was given.
        timeout: Duration,
    },

    /// vim exited without reporting a state.
    NoState {
        /// The status vim exited with.
        status: ExitStatus,

        /// Everything vim wrote to its standard error.
        stderr: String,
    },

    /// The state vim reported could not be decoded.
    MalformedState {
        /// What could not be decoded.
        detail: String,
    },

    /// vim ended in a mode the state schema cannot represent.
    UnsupportedMode {
        /// The mode vim reported, as returned by its `mode(1)`.
        mode: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
            Self::Launch { binary, source } => write!(
                formatter,
                "failed to run vim binary `{}`: {source}; install vim {MINIMUM_VERSION} or newer, \
                 or point the driver at an existing binary",
                binary.display()
            ),
            Self::UnrecognizedBinary {
                binary,
                version_output,
            } => write!(
                formatter,
                "`{}` is not vim: its `--version` printed `{version_output}`; the driver needs a \
                 real vim {MINIMUM_VERSION} or newer",
                binary.display()
            ),
            Self::UnsupportedVersion { binary, version } => write!(
                formatter,
                "vim {version} at `{}` is older than the required {MINIMUM_VERSION}; upgrade vim",
                binary.display()
            ),
            Self::MissingFeature { binary, feature } => write!(
                formatter,
                "vim at `{}` was built without `+{feature}`; install a full vim build, for example \
                 the `vim` package instead of `vim-tiny`",
                binary.display()
            ),
            Self::Wait { source } => write!(formatter, "failed to wait for vim: {source}"),
            Self::Timeout { timeout } => write!(
                formatter,
                "vim did not finish within {timeout:?} and was killed; the key sequence may leave \
                 vim waiting for an argument, as an unfinished `f` or `q` does"
            ),
            Self::NoState { status, stderr } => write!(
                formatter,
                "vim exited with {status} without reporting a state; its standard error held \
                 `{}`",
                stderr.trim()
            ),
            Self::MalformedState { detail } => {
                write!(
                    formatter,
                    "vim reported a state that is not readable: {detail}"
                )
            }
            Self::UnsupportedMode { mode } => write!(
                formatter,
                "vim ended in mode `{mode}`, which the state schema cannot represent"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Launch { source, .. } | Self::Wait { source } => {
                Some(source)
            }
            _ => None,
        }
    }
}

/// The features a vim build must have for the driver to drive it.
const REQUIRED_FEATURES: [&str; 2] = ["eval", "timers"];

/// The time a single replay is given before vim is killed.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a running vim is checked for having exited.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// The arguments that keep vim reproducible: no user configuration, no plugins, no viminfo, no
/// swap file, and no terminal.
const VIM_ARGUMENTS: [&str; 8] = [
    "-u",
    "NONE",
    "-N",
    "--noplugin",
    "-n",
    "-i",
    "NONE",
    "--not-a-term",
];

/// The options that must be set before the starting buffer is read, so that vim neither rewrites
/// its bytes nor takes instructions from them.
const PRE_COMMANDS: &str = "set encoding=utf-8 fileencodings= fileformats=unix nofixendofline \
                            nomodeline nomore noswapfile nobackup nowritebackup noundofile \
                            ttimeout ttimeoutlen=0 shortmess+=A";

/// The registers a differential run compares.
const CAPTURED_REGISTERS: &str = "\"-0123456789abcdefghijklmnopqrstuvwxyz";

/// Distinguishes the workspaces of concurrent runs in the same process.
static WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary directory holding one run's files, removed when the run ends.
struct Workspace {
    path: PathBuf,
}

impl Workspace {
    /// Factory function.
    ///
    /// Creates an empty directory for a single run.
    ///
    /// # Returns
    ///
    /// A newly created workspace on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Io`] if the directory could not be created.
    fn new() -> Result<Self, Error> {
        let path = env::temp_dir().join(format!(
            "vbc-vim-{}-{}",
            std::process::id(),
            WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The state a run's vim writes out, in the shape vim's `json_encode` produces.
#[derive(Deserialize)]
struct Report {
    buffer: String,
    line: u64,
    column: u64,
    mode: String,
    registers: BTreeMap<String, (String, String)>,
}

impl Report {
    /// Translates vim's report into the schema a differential run compares.
    ///
    /// # Returns
    ///
    /// The reported state on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::UnsupportedMode`] if vim reported a mode [`Mode`] cannot represent.
    /// * [`Error::MalformedState`] if vim reported a register the schema cannot hold.
    fn into_state(self) -> Result<EditorState, Error> {
        let mode = parse_mode(&self.mode)?;
        let mut registers: BTreeMap<RegisterName, Register> = BTreeMap::new();
        for (name, (text, register_type)) in self.registers {
            let mut characters = name.chars();
            let (Some(name), None) = (characters.next(), characters.next()) else {
                return Err(Error::MalformedState {
                    detail: format!("`{name}` is not a register name"),
                });
            };
            registers.insert(
                name,
                Register {
                    text,
                    register_type: parse_register_type(&register_type)?,
                },
            );
        }

        Ok(EditorState {
            buffer: self.buffer,
            cursor: Cursor {
                line: self.line,
                column: self.column,
            },
            mode,
            registers,
        })
    }
}

/// Writes a file the driver hands to vim.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Io`] if the file could not be written.
fn write_file(path: &Path, contents: &[u8]) -> Result<(), Error> {
    fs::write(path, contents).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}

/// Reads a vim version out of the binary's `--version` report.
///
/// # Returns
///
/// * The reported version.
/// * `None` if the report is not a vim version report.
fn parse_version(report: &str) -> Option<VimVersion> {
    let banner = report.lines().next()?;
    if !banner.contains("Vi IMproved") {
        return None;
    }

    let (major, minor) = banner.split_whitespace().find_map(|token| {
        let (major, minor) = token.split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    })?;
    let patch = report.lines().find_map(parse_patch_level).unwrap_or(0);

    Some(VimVersion {
        major,
        minor,
        patch,
    })
}

/// Reads the highest patch number out of a `--version` report's patch line.
///
/// # Returns
///
/// * The highest patch number the line lists.
/// * `None` if the line does not list patches.
fn parse_patch_level(line: &str) -> Option<u32> {
    line.strip_prefix("Included patches: ")?
        .split([',', '-', ' '])
        .filter_map(|patch| patch.parse().ok())
        .max()
}

/// Builds the script that replays the keys and writes the resulting state out.
///
/// # Returns
///
/// The script vim sources after it has opened the starting buffer.
fn build_script(state_path: &Path, keys: &str) -> String {
    format!(
        r#"
function! g:VbcCapture() abort
  let l:registers = {{}}
  for l:name in split('{CAPTURED_REGISTERS}', '\zs')
    let l:text = getreg(l:name)
    if l:text !=# ''
      let l:registers[l:name] = [l:text, getregtype(l:name)]
    endif
  endfor
  let l:state = {{
        \ 'buffer': join(getline(1, '$'), "\n") . (&endofline ? "\n" : ''),
        \ 'line': line('.') - 1,
        \ 'column': col('.') - 1,
        \ 'mode': mode(1),
        \ 'registers': l:registers,
        \ }}
  call writefile([json_encode(l:state)], '{state_path}', 'b')
  call timer_start(0, function('g:VbcQuit'))
endfunction

function! g:VbcQuit(...) abort
  qall!
endfunction

call feedkeys("{keys}\<Cmd>call g:VbcCapture()\<CR>", 'nt')
"#,
        state_path = state_path.display().to_string().replace('\'', "''"),
        keys = render_keys(keys),
    )
}

/// Renders a key sequence into the body of a vim double-quoted string.
///
/// # Returns
///
/// The rendered keys, in which `<Esc>` and its like become the key codes vim gives a typed key.
fn render_keys(keys: &str) -> String {
    let mut rendered = String::with_capacity(keys.len());
    let mut rest = keys;
    while !rest.is_empty() {
        if let Some(remainder) = rest.strip_prefix('<') {
            if let Some(end) = remainder.find('>') {
                let name = &remainder[..end];
                if is_key_name(name) {
                    rendered.push_str("\\<");
                    rendered.push_str(name);
                    rendered.push('>');
                    rest = &remainder[end + 1..];
                    continue;
                }
            }
        }

        let character = rest.chars().next().expect("the rest is not empty");
        match character {
            '"' | '\\' => {
                rendered.push('\\');
                rendered.push(character);
            }
            control if control.is_ascii_control() => {
                rendered.push_str(&format!("\\x{:02x}", control as u32));
            }
            literal => rendered.push(literal),
        }
        rest = &rest[character.len_utf8()..];
    }

    rendered
}

/// # Returns
///
/// Whether the text names a key, as the `Esc` of `<Esc>` does.
fn is_key_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// Translates the mode vim's `mode(1)` reports.
///
/// # Returns
///
/// The reported mode on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::UnsupportedMode`] if the mode has no [`Mode`] counterpart, as vim's select and
///   terminal modes do not.
fn parse_mode(mode: &str) -> Result<Mode, Error> {
    Ok(match mode {
        pending if pending.starts_with("no") => Mode::OperatorPending,
        normal if normal.starts_with('n') => Mode::Normal,
        insert if insert.starts_with('i') => Mode::Insert,
        replace if replace.starts_with('R') => Mode::Replace,
        visual if visual.starts_with('v') => Mode::Visual,
        visual_line if visual_line.starts_with('V') => Mode::VisualLine,
        visual_block if visual_block.starts_with('\u{16}') => Mode::VisualBlock,
        command_line if command_line.starts_with('c') => Mode::CommandLine,
        unsupported => {
            return Err(Error::UnsupportedMode {
                mode: unsupported.to_owned(),
            })
        }
    })
}

/// Translates the layout vim's `getregtype()` reports.
///
/// # Returns
///
/// The reported layout on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::MalformedState`] if the layout is none of vim's three.
fn parse_register_type(register_type: &str) -> Result<RegisterType, Error> {
    Ok(match register_type {
        "v" => RegisterType::Charwise,
        "V" => RegisterType::Linewise,
        blockwise if blockwise.starts_with('\u{16}') => RegisterType::Blockwise,
        unknown => {
            return Err(Error::MalformedState {
                detail: format!("`{unknown}` is not a register layout"),
            })
        }
    })
}

/// Waits for vim to exit, killing it once it has run out of time.
///
/// # Returns
///
/// The status vim exited with on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Timeout`] if vim had to be killed.
/// * [`Error::Wait`] if vim could not be waited on.
fn wait_for_exit(mut child: Child, timeout: Duration) -> Result<ExitStatus, Error> {
    let deadline = Instant::now() + timeout;
    loop {
        let exited = child.try_wait().map_err(|source| Error::Wait { source })?;
        if let Some(status) = exited {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Timeout { timeout });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUFFER: &str = "alpha beta gamma\nsecond line\n";

    /// # Returns
    ///
    /// A driver bound to the vim on `PATH` on success.
    fn driver() -> anyhow::Result<VimDriver> {
        Ok(VimDriver::new()?)
    }

    /// # Returns
    ///
    /// The register a charwise delete or yank of the given text leaves behind.
    fn charwise(text: &str) -> Register {
        Register {
            text: text.to_owned(),
            register_type: RegisterType::Charwise,
        }
    }

    /// Writes an executable that answers `--version` with the given report.
    ///
    /// # Returns
    ///
    /// The workspace holding the executable, which removes it when dropped, and its path.
    #[cfg(unix)]
    fn fake_vim(report: &str) -> anyhow::Result<(Workspace, PathBuf)> {
        use std::os::unix::fs::PermissionsExt;

        let workspace = Workspace::new()?;
        let path = workspace.path().join("vim");
        write_file(
            &path,
            format!("#!/bin/sh\ncat <<'EOF'\n{report}\nEOF\n").as_bytes(),
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;

        Ok((workspace, path))
    }

    #[test]
    fn dw_deletes_the_first_word() -> anyhow::Result<()> {
        let state = driver()?.run(BUFFER, "dw")?;

        assert_eq!(
            state,
            EditorState {
                buffer: "beta gamma\nsecond line\n".to_owned(),
                cursor: Cursor { line: 0, column: 0 },
                mode: Mode::Normal,
                registers: BTreeMap::from([('"', charwise("alpha ")), ('-', charwise("alpha ")),]),
            }
        );
        Ok(())
    }

    #[test]
    fn repeated_runs_report_identical_state() -> anyhow::Result<()> {
        const RUNS: usize = 100;

        let driver = driver()?;
        let expected = driver.run(BUFFER, "wdwyiwj$a!<Esc>0")?;

        for run in 1..RUNS {
            assert_eq!(
                driver.run(BUFFER, "wdwyiwj$a!<Esc>0")?,
                expected,
                "run {run} diverged from the first run"
            );
        }
        Ok(())
    }

    #[test]
    fn a_missing_binary_reports_an_actionable_error() {
        let error = VimDriver::with_binary("/nonexistent/directory/vim")
            .expect_err("a missing binary cannot be driven");

        assert!(
            matches!(error, Error::Launch { .. }),
            "expected a launch failure, got {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("/nonexistent/directory/vim") && message.contains("install vim"),
            "the message names neither the binary nor the fix: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_outdated_vim_is_rejected() -> anyhow::Result<()> {
        let (_workspace, path) = fake_vim(
            "VIM - Vi IMproved 8.2 (2019 Dec 12, compiled Jan 01 2020 00:00:00)\n\
             Included patches: 1-100\n\
             Huge version without GUI.  Features included (+) or not (-):\n\
             +eval +timers",
        )?;

        let error = VimDriver::with_binary(&path).expect_err("an outdated vim cannot be driven");

        assert!(
            matches!(error, Error::UnsupportedVersion { version, .. } if version
                == VimVersion { major: 8, minor: 2, patch: 100 }),
            "expected an unsupported version, got {error:?}"
        );
        assert!(
            error
                .to_string()
                .contains("older than the required 8.2.1978"),
            "the message does not name the required version: {error}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_vim_without_the_features_the_driver_needs_is_rejected() -> anyhow::Result<()> {
        let (_workspace, path) = fake_vim(
            "VIM - Vi IMproved 9.1 (2024 Jan 02, compiled Jan 01 2025 00:00:00)\n\
             Included patches: 1-1000\n\
             Small version without GUI.  Features included (+) or not (-):\n\
             -eval -timers",
        )?;

        let error = VimDriver::with_binary(&path).expect_err("a small vim cannot be driven");

        assert!(
            matches!(
                error,
                Error::MissingFeature {
                    feature: "eval",
                    ..
                }
            ),
            "expected a missing feature, got {error:?}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_binary_that_is_not_vim_is_rejected() -> anyhow::Result<()> {
        let (_workspace, path) = fake_vim("NVIM v0.9.5")?;

        let error = VimDriver::with_binary(&path).expect_err("only vim can be driven");

        assert!(
            matches!(error, Error::UnrecognizedBinary { .. }),
            "expected an unrecognized binary, got {error:?}"
        );
        Ok(())
    }

    #[test]
    fn the_driver_reports_the_vim_it_ran_against() -> anyhow::Result<()> {
        let driver = driver()?;

        assert!(
            driver.version() >= MINIMUM_VERSION,
            "the driver accepted vim {}",
            driver.version()
        );
        Ok(())
    }

    #[test]
    fn escape_leaves_insert_mode() -> anyhow::Result<()> {
        let driver = driver()?;

        let inserting = driver.run(BUFFER, "ihello")?;
        assert_eq!(inserting.mode, Mode::Insert);
        assert_eq!(inserting.cursor, Cursor { line: 0, column: 5 });

        let inserted = driver.run(BUFFER, "ihello<Esc>")?;
        assert_eq!(inserted.mode, Mode::Normal);
        assert_eq!(inserted.cursor, Cursor { line: 0, column: 4 });
        assert_eq!(inserted.buffer, "helloalpha beta gamma\nsecond line\n");
        Ok(())
    }

    #[test]
    fn control_keys_are_transmitted() -> anyhow::Result<()> {
        let driver = driver()?;

        assert_eq!(
            driver.run(BUFFER, "yiwA <C-r>\"<Esc>")?.buffer,
            "alpha beta gamma alpha\nsecond line\n"
        );
        assert_eq!(
            driver.run(BUFFER, "i<C-o>A!")?,
            EditorState {
                buffer: "alpha beta gamma!\nsecond line\n".to_owned(),
                cursor: Cursor {
                    line: 0,
                    column: 17,
                },
                mode: Mode::Insert,
                registers: BTreeMap::new(),
            }
        );
        Ok(())
    }

    #[test]
    fn arrow_keys_move_the_cursor() -> anyhow::Result<()> {
        let state = driver()?.run(BUFFER, "<Down><Right><Right>x")?;

        assert_eq!(
            state,
            EditorState {
                buffer: "alpha beta gamma\nseond line\n".to_owned(),
                cursor: Cursor { line: 1, column: 2 },
                mode: Mode::Normal,
                registers: BTreeMap::from([('"', charwise("c")), ('-', charwise("c"))]),
            }
        );
        Ok(())
    }

    #[test]
    fn a_lone_operator_ends_in_operator_pending_mode() -> anyhow::Result<()> {
        assert_eq!(driver()?.run(BUFFER, "d")?.mode, Mode::OperatorPending);
        Ok(())
    }

    #[test]
    fn linewise_and_blockwise_yanks_report_their_layout() -> anyhow::Result<()> {
        let driver = driver()?;

        assert_eq!(
            driver.run(BUFFER, "yy")?.registers[&'"'],
            Register {
                text: "alpha beta gamma\n".to_owned(),
                register_type: RegisterType::Linewise,
            }
        );
        assert_eq!(
            driver.run(BUFFER, "<C-v>jly")?.registers[&'"'],
            Register {
                text: "al\nse".to_owned(),
                register_type: RegisterType::Blockwise,
            }
        );
        Ok(())
    }

    #[test]
    fn a_buffer_without_a_trailing_newline_keeps_its_shape() -> anyhow::Result<()> {
        assert_eq!(
            driver()?.run("no newline", "A!<Esc>")?.buffer,
            "no newline!"
        );
        Ok(())
    }

    #[test]
    fn a_run_that_outlasts_its_timeout_is_killed() -> anyhow::Result<()> {
        let mut driver = driver()?;
        driver.set_timeout(Duration::from_millis(300));

        let error = driver
            .run(BUFFER, ":sleep 30<CR>")
            .expect_err("a sleeping vim outlasts its timeout");

        assert!(
            matches!(error, Error::Timeout { .. }),
            "expected a timeout, got {error:?}"
        );
        Ok(())
    }

    #[test]
    fn a_key_sequence_that_never_settles_reports_an_error() -> anyhow::Result<()> {
        let mut driver = driver()?;
        driver.set_timeout(Duration::from_millis(500));

        let error = driver
            .run(BUFFER, "f")
            .expect_err("an unfinished `f` never lets vim report a state");

        assert!(
            matches!(error, Error::Timeout { .. } | Error::NoState { .. }),
            "expected a timeout or a missing state, got {error:?}"
        );
        Ok(())
    }

    #[test]
    fn key_notation_renders_literals_and_named_keys() {
        assert_eq!(render_keys("dw"), "dw");
        assert_eq!(render_keys("i\"quoted\\\""), r#"i\"quoted\\\""#);
        assert_eq!(render_keys("<Esc><C-r><Down>"), r"\<Esc>\<C-r>\<Down>");
        assert_eq!(render_keys("a < b"), "a < b");
        assert_eq!(render_keys("<lt>"), r"\<lt>");
        assert_eq!(render_keys("\u{1b}"), r"\x1b");
    }

    #[test]
    fn version_reports_are_parsed() {
        assert_eq!(
            parse_version(
                "VIM - Vi IMproved 9.1 (2024 Jan 02, compiled Jan 01 2025 00:00:00)\n\
                 Included patches: 1-16, 647, 17-579\n"
            ),
            Some(VimVersion {
                major: 9,
                minor: 1,
                patch: 647,
            })
        );
        assert_eq!(
            parse_version("VIM - Vi IMproved 8.2 (2019 Dec 12)\n"),
            Some(VimVersion {
                major: 8,
                minor: 2,
                patch: 0,
            })
        );
        assert_eq!(parse_version("NVIM v0.9.5\n"), None);
        assert_eq!(parse_version(""), None);
    }
}
