//! How the `claude` child is started, and the hygiene that has to be applied before it is.
//!
//! Three of the decisions here are not preferences. The child's standard input must be a pipe: on
//! a terminal it exits in about a second saying input must come through stdin or as a prompt
//! argument, so a session hosted in a pty is not a session at all. `--permission-prompt-tool
//! stdio` must be passed or every tool call needing approval is auto-denied by a notification
//! nothing can answer. And the invocation is pinned to the print lane, because the flags that
//! resume a session partway through are print-lane only and an interactive resume ignores them
//! without a word. A fourth is smaller and is the same kind of decision: a subagent's own prose is
//! forwarded only under `--forward-subagent-text`, and a panel that folds a subagent's work away
//! wants that work to be there to fold.
//!
//! The environment is the last of them. vimbecode is often started from inside a Claude Code
//! session, and the variables such a session exports are read by the child as a claim to be part
//! of it: inheriting them silently disables transcript persistence, and a completed multi-turn
//! session leaves nothing on disk at all. They are stripped from the [`Command`] rather than from
//! this process, so what a spawn does is decided by the spawn instead of by how vimbecode was
//! started. One variable goes the other way -- file checkpointing has no flag, is irreversible
//! once the child is up, and is what a rewind depends on -- so it is set after the strip that
//! would otherwise remove it.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::identity::Identity;
use super::probe::{PERMISSION_PROMPT_TOOL, PERMISSION_PROMPT_TOOL_VALUE};

/// The binary a session is spoken to, as it is found on the path.
pub const BINARY: &str = "claude";

/// The flags that put the binary in the print lane and make both directions NDJSON. `--verbose` is
/// not a logging preference: without it the print lane emits one summary frame instead of the
/// stream.
pub const PRINT_FLAG: &str = "-p";
pub const INPUT_FORMAT_FLAG: &str = "--input-format";
pub const OUTPUT_FORMAT_FLAG: &str = "--output-format";
pub const STREAM_FORMAT: &str = "stream-json";
pub const VERBOSE_FLAG: &str = "--verbose";

/// The flag that forwards what a subagent said as well as what it called. Without it a subagent's
/// tool calls arrive tagged with the call that started it and its own prose does not arrive at
/// all, so the nested fold over a subagent covers what it did and not what it reported.
pub const FORWARD_SUBAGENT_TEXT_FLAG: &str = "--forward-subagent-text";

/// The flag naming the model a turn runs on.
pub const MODEL_FLAG: &str = "--model";

/// The variable that turns file checkpointing on, which has no flag and cannot be changed once the
/// child is up.
pub const CHECKPOINTING: &str = "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING";
pub const CHECKPOINTING_VALUE: &str = "true";

/// The variables a Claude Code session exports, which a child of one must not inherit.
pub const INHERITED_PREFIX: &str = "CLAUDE_CODE_";
pub const INHERITED_NAMES: [&str; 3] = ["CLAUDECODE", "CLAUDE_PID", "CLAUDE_EFFORT"];

/// How a session's child is to be started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spawn {
    binary: OsString,
    directory: Option<PathBuf>,
    model: Option<String>,
    identity: Identity,
}

impl Spawn {
    /// # Returns
    ///
    /// A newly created spawn of a conversation, in the directory this process is already in and on
    /// the model the binary would choose for itself.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self {
            binary: OsString::from(BINARY),
            directory: None,
            model: None,
            identity,
        }
    }

    /// # Returns
    ///
    /// The spawn, of a binary other than the one on the path.
    #[must_use]
    pub fn with_binary(mut self, binary: impl AsRef<OsStr>) -> Self {
        self.binary = binary.as_ref().to_owned();
        self
    }

    /// # Returns
    ///
    /// The spawn, in a directory other than this process's own.
    #[must_use]
    pub fn with_directory(mut self, directory: impl AsRef<Path>) -> Self {
        self.directory = Some(directory.as_ref().to_owned());
        self
    }

    /// # Returns
    ///
    /// The spawn, on a named model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    #[must_use]
    pub fn binary(&self) -> &OsStr {
        &self.binary
    }

    /// # Returns
    ///
    /// Every argument the child is started with, in order.
    #[must_use]
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            PRINT_FLAG.to_owned(),
            INPUT_FORMAT_FLAG.to_owned(),
            STREAM_FORMAT.to_owned(),
            OUTPUT_FORMAT_FLAG.to_owned(),
            STREAM_FORMAT.to_owned(),
            VERBOSE_FLAG.to_owned(),
            FORWARD_SUBAGENT_TEXT_FLAG.to_owned(),
            PERMISSION_PROMPT_TOOL.to_owned(),
            PERMISSION_PROMPT_TOOL_VALUE.to_owned(),
        ];
        if let Some(model) = &self.model {
            arguments.push(MODEL_FLAG.to_owned());
            arguments.push(model.clone());
        }
        arguments.extend(self.identity.arguments());

        arguments
    }

    /// # Returns
    ///
    /// The command that starts the child: the arguments above, all three streams on pipes, and an
    /// environment with what a Claude Code session exports taken out and file checkpointing put
    /// in.
    #[must_use]
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command.args(self.arguments());
        if let Some(directory) = &self.directory {
            command.current_dir(directory);
        }

        for (key, _) in std::env::vars_os() {
            if stripped(&key.to_string_lossy()) {
                command.env_remove(&key);
            }
        }
        command.env(CHECKPOINTING, CHECKPOINTING_VALUE);

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        command
    }
}

/// # Returns
///
/// Whether a variable is one a Claude Code session exports, and therefore one the child must not
/// be handed. [`CHECKPOINTING`] is one of them, and is put back after the strip rather than
/// excepted from it, so this stays the whole rule.
#[must_use]
pub fn stripped(key: &str) -> bool {
    key.starts_with(INHERITED_PREFIX) || INHERITED_NAMES.contains(&key)
}
