//! The keys a sequence written in vim's own notation stands for.
//!
//! A case cross-checked against vim is written once and typed at both engines, so the keys reach
//! vim as the string vim's `feedkeys()` reads and reach this editor as the events a terminal
//! reports for the same string. A file that spelled its own keys twice would be a file whose two
//! sides could drift apart, and a file that could only spell the characters a terminal reports
//! with no modifier held could not type `CTRL-V` at all.

use crossterm::event::{KeyCode, KeyModifiers};
use vbc_editor::event::KeyEvent;

/// The keys vim's notation names by something other than the character they type, each with the
/// event a terminal reports for it. `CTRL-` keys are spelled by the character they hold rather
/// than named here, since every one of them is spelled the same way.
const NAMED: [(&str, KeyCode); 4] = [
    ("Esc", KeyCode::Esc),
    ("CR", KeyCode::Enter),
    ("Tab", KeyCode::Tab),
    ("BS", KeyCode::Backspace),
];

/// # Returns
///
/// The key events `sequence` names, in which `<Esc>` and its like name a key, `<C-v>` names a
/// character typed with the control key held, and every other character stands for itself.
///
/// # Panics
///
/// Panics if the sequence names a key this notation does not hold, so that a case whose keys were
/// mistyped fails rather than being replayed as the characters they were written from.
pub fn keys(sequence: &str) -> Vec<KeyEvent> {
    let mut keys = Vec::new();
    let mut rest = sequence;
    while let Some(index) = rest.find('<') {
        keys.extend(rest[..index].chars().map(typed));
        let named = &rest[index..];
        let end = named
            .find('>')
            .unwrap_or_else(|| panic!("`{sequence}` leaves a named key unclosed"));
        keys.push(key(&named[1..end], sequence));
        rest = &named[end + 1..];
    }
    keys.extend(rest.chars().map(typed));

    keys
}

/// # Returns
///
/// The event a terminal reports for the key `name` names.
///
/// # Panics
///
/// Panics if `name` names no key this notation holds.
fn key(name: &str, sequence: &str) -> KeyEvent {
    if let Some(held) = name.strip_prefix("C-") {
        let mut characters = held.chars();
        let character = characters
            .next()
            .filter(|_| characters.next().is_none())
            .unwrap_or_else(|| panic!("`<{name}>` of `{sequence}` holds one character"));

        return KeyEvent::new(
            KeyCode::Char(character.to_ascii_lowercase()),
            KeyModifiers::CONTROL,
        );
    }
    let code = NAMED
        .into_iter()
        .find(|(named, _code)| named.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("`<{name}>` of `{sequence}` is not a key this notation holds"))
        .1;

    KeyEvent::new(code, KeyModifiers::NONE)
}

/// # Returns
///
/// The event a terminal reports when `character` is typed with no modifier held.
fn typed(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}
