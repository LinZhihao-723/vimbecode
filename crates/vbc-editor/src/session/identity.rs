//! Which conversation a spawn joins, and the identifier it is joined by.
//!
//! A session is named by a uuid, and there are three ways to start one: with an identifier we
//! chose, from an identifier the child already knows, or from one it knows with the history copied
//! aside first. The third is what a rewind respawns through, and the identifier it ends up under
//! is not the one it was given -- the child mints a new one and announces it in the init frame --
//! so the identifier a session is *running* as is always read from the stream rather than assumed
//! from the spawn.
//!
//! The uuid is generated here rather than taken from a crate. What a session identifier has to be
//! is unique on this machine, not unpredictable to anyone: it names a local conversation, is
//! written to a local transcript, and is never a secret or a capability. Uniqueness is what the
//! generation below buys, from the clock, the process and the hash seeds the standard library
//! already draws from the operating system, and it buys it without widening the dependency graph
//! this workspace holds itself to.

use std::collections::hash_map::RandomState;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::hash::{BuildHasher, Hasher};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

/// The flag naming the identifier a new session is to run under.
pub const SESSION_ID_FLAG: &str = "--session-id";

/// The flag naming the session a spawn continues.
pub const RESUME_FLAG: &str = "--resume";

/// The flag that copies the resumed history aside instead of continuing it in place.
pub const FORK_SESSION_FLAG: &str = "--fork-session";

/// The identifier a session is known by, machine-wide and across processes.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(String);

impl SessionId {
    /// # Returns
    ///
    /// A newly created identifier no session has been started under before.
    #[must_use]
    pub fn generated() -> Self {
        Self(hyphenated(bits()))
    }

    /// # Returns
    ///
    /// The identifier a session is already known by, which is one the child minted rather than one
    /// we chose.
    #[must_use]
    pub fn known(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SessionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str(&self.0)
    }
}

/// Which conversation a spawn joins.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Identity {
    /// A conversation of its own, under an identifier we chose.
    Fresh(SessionId),

    /// The conversation an identifier already names, continued in place.
    Resumed(SessionId),

    /// The conversation an identifier already names, copied aside first. The session that results
    /// runs under an identifier the child mints, which the init frame is the only account of.
    Forked(SessionId),
}

impl Identity {
    /// # Returns
    ///
    /// The identifier the spawn is given, which is the one the session ends up running under for
    /// every identity but [`Identity::Forked`].
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::Fresh(id) | Self::Resumed(id) | Self::Forked(id) => id,
        }
    }

    /// # Returns
    ///
    /// The arguments that join the spawn to this conversation.
    #[must_use]
    pub fn arguments(&self) -> Vec<String> {
        match self {
            Self::Fresh(id) => vec![SESSION_ID_FLAG.to_owned(), id.0.clone()],
            Self::Resumed(id) => vec![RESUME_FLAG.to_owned(), id.0.clone()],
            Self::Forked(id) => vec![
                RESUME_FLAG.to_owned(),
                id.0.clone(),
                FORK_SESSION_FLAG.to_owned(),
            ],
        }
    }
}

impl Default for Identity {
    fn default() -> Self {
        Self::Fresh(SessionId::generated())
    }
}

/// # Returns
///
/// Sixteen bytes distinct from any this machine has handed out before, laid out as a version 4
/// uuid: the four bits naming the version and the two naming the variant are what a reader of the
/// identifier checks, and everything else is drawn from the clock, the process and two hash seeds
/// the standard library takes from the operating system.
fn bits() -> [u8; 16] {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(now);
    hasher.write_u32(process::id());
    let high = hasher.finish();

    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(high);
    hasher.write_u128(now);
    let low = hasher.finish();

    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    bytes
}

/// # Returns
///
/// Sixteen bytes written the way a uuid is: lowercase hexadecimal, in groups of 4, 2, 2, 2 and 6
/// bytes.
fn hyphenated(bytes: [u8; 16]) -> String {
    let mut text = String::with_capacity(36);
    for (at, byte) in bytes.iter().enumerate() {
        if matches!(at, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        text.push_str(&format!("{byte:02x}"));
    }

    text
}
