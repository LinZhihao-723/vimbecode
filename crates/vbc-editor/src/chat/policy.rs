//! What a reader may do to a transcript, and what becomes of the keys that ask for more.
//!
//! The chat panel is the editor's own machinery pointed at what was said, and it shares all of it:
//! the same keybinding table, the same engine, the same motions, registers and selections. What it
//! does not share is the permission to write. A transcript is the record of an exchange that
//! already happened, and an editor that let `x` take a character out of it would be an editor
//! whose transcript no longer says what was said.
//!
//! The refusal is decided over the actions a keystroke asks for rather than over the keys
//! themselves, because the keys do not say what they do. `d` on its own asks for nothing at all,
//! `dgj` asks for a delete over a motion counted in screen lines, and a table that has been
//! rebound spells either of them with other keys entirely. So a keystroke is read the whole way
//! through the keybinding machine before any of it is run, and only one whose every action leaves
//! the text as it stands is typed at the engine. One that would write never reaches the engine at
//! all, which is what leaves the transcript byte-identical rather than merely restored to what it
//! held.
//!
//! Reading a keystroke before running it costs a second copy of the keybinding machine, kept in
//! step with the engine's by feeding it the very same keys. The keys of a sequence still being
//! typed are held back until the sequence completes, and a completed sequence either reaches the
//! engine whole or is dropped whole and the machine wound back to where the engine still stands.
//!
//! Two things are refused rather than one. An action that writes is the obvious one. The other is
//! the mode: `i` asks for nothing but a cursor split and a mode to stand in, so a panel reading
//! actions alone would grant it and then sit in insert mode refusing every character typed into
//! it. vim refuses `i` outright on a buffer that is `'nomodifiable'`, and so does this.
//!
//! What is left over is everything a reader came for. Every motion, the ones counted in screen
//! lines among them, every character search, every yank into every register and every visual
//! selection reaches the engine exactly as it would in a buffer that could be written to, because
//! none of them writes.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::mem;

use modalkit::actions::Action;
use modalkit::editing::context::EditContext;
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;

use crate::engine::{Engine, Error, Held, Position};
use crate::event::KeyEvent;
use crate::keys::Keys;
use crate::screen::Geometry;

/// What the status line says about a keystroke the panel would not run.
pub const REFUSAL: &str = "the transcript is read-only";

/// What a panel lets the keystrokes typed at it do.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Policy {
    /// Nothing runs that could leave the transcript different from the way it arrived, and nothing
    /// puts the panel in a mode whose purpose is to write.
    #[default]
    ReadOnly,

    /// Everything a keystroke asks for runs. This is not a policy the chat panel is used under: it
    /// is here so that a refusal can be shown to be [`Policy::ReadOnly`]'s doing rather than a
    /// keystroke that happened to do nothing, by typing the same keys under it and watching the
    /// transcript change.
    Unrestricted,
}

impl Policy {
    /// # Returns
    ///
    /// Whether a keystroke asking for `actions` and leaving the keys standing in `mode` may reach
    /// the text.
    #[must_use]
    pub fn allows(self, actions: &[(Action, EditContext)], mode: VimMode) -> bool {
        if Self::Unrestricted == self {
            return true;
        }

        VimMode::Insert != mode
            && actions
                .iter()
                .all(|(action, context)| readable(action, context))
    }
}

/// A keystroke a policy would not let reach the text, spelled the way it was typed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    keys: String,
}

impl Refusal {
    /// # Returns
    ///
    /// The keys that were refused, spelled as a vim manual spells them.
    #[must_use]
    pub fn keys(&self) -> &str {
        &self.keys
    }
}

impl Display for Refusal {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(
            formatter,
            "{REFUSAL}: `{}` would change what was said",
            self.keys
        )
    }
}

/// A transcript panel: a vim engine over what was said, and the policy deciding which of the
/// keystrokes typed at it reach it.
///
/// Keys go in one at a time as they do at an engine, and a key that only carries a sequence
/// further -- a count, a register, an operator waiting for its target -- is held back until the
/// sequence it belongs to completes, because what a sequence does is not known until then.
pub struct Panel {
    engine: Engine,
    policy: Policy,
    keys: Keys,
    agreed: Keys,
    held: Vec<KeyEvent>,
    refusal: Option<Refusal>,
}

impl Panel {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created read-only panel showing `transcript`, with the cursor on its first
    /// character, measuring the screen motions typed at it in the window a vim manual draws its
    /// examples in.
    #[must_use]
    pub fn new(transcript: &str) -> Self {
        Self::over(Engine::new(transcript))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created panel like [`Panel::new`]'s, measuring the screen motions typed at it in
    /// `geometry`.
    #[must_use]
    pub fn laid_out_in(transcript: &str, geometry: Geometry) -> Self {
        Self::over(Engine::laid_out_in(transcript, geometry))
    }

    /// # Returns
    ///
    /// The same panel governed by `policy` rather than by [`Policy::ReadOnly`].
    #[must_use]
    pub fn governed_by(mut self, policy: Policy) -> Self {
        self.policy = policy;

        self
    }

    /// # Returns
    ///
    /// The policy the panel refuses keystrokes by.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Types one key at the panel, running everything it asks for that the policy allows.
    ///
    /// A keystroke the policy refuses reaches neither the engine nor the text, and what the status
    /// line says about it is what [`Panel::refusal`] answers with until the next key is typed.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::press_all`]'s return values on failure.
    pub fn press(&mut self, key: KeyEvent) -> Result<(), Error> {
        self.refusal = None;
        self.held.push(key);
        self.keys.input_key(key.into());
        let mut asked = Vec::new();
        while let Some(popped) = self.keys.pop() {
            asked.push(popped);
        }
        if !self.policy.allows(&asked, self.keys.mode()) {
            self.keys = self.agreed.clone();
            self.refusal = Some(Refusal {
                keys: spelled(&mem::take(&mut self.held)),
            });

            return Ok(());
        }
        if asked.is_empty() {
            return Ok(());
        }
        let typed = mem::take(&mut self.held);
        let ran = self.engine.press_all(typed);
        self.agreed = self.keys.clone();

        ran
    }

    /// Types a sequence of keys at the panel, one key at a time.
    ///
    /// # Type Parameters
    ///
    /// * `KeysType` - The keys to type, in the order they are typed.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Panel::press`]'s return values on failure.
    pub fn press_all<KeysType: IntoIterator<Item = KeyEvent>>(
        &mut self,
        keys: KeysType,
    ) -> Result<(), Error> {
        for key in keys {
            self.press(key)?;
        }

        Ok(())
    }

    /// # Returns
    ///
    /// The transcript being read, which ends in a newline as vim's own text does.
    #[must_use]
    pub fn text(&self) -> String {
        self.engine.text()
    }

    /// # Returns
    ///
    /// Where the cursor rests in the transcript.
    pub fn cursor(&mut self) -> Position {
        self.engine.cursor()
    }

    /// # Returns
    ///
    /// The mode the panel is in, which is the mode the keys typed at it left the keybinding
    /// machine in rather than the mode a refused keystroke asked for.
    #[must_use]
    pub fn mode(&self) -> VimMode {
        self.keys.mode()
    }

    /// # Returns
    ///
    /// What every register holding text holds, keyed by the name it is addressed by.
    #[must_use]
    pub fn registers(&self) -> BTreeMap<char, Held> {
        self.engine.registers()
    }

    /// # Returns
    ///
    /// The keystroke the policy last refused, whose [`Display`] is what the status line says, and
    /// [`None`] where the last key typed was one the panel ran.
    #[must_use]
    pub fn refusal(&self) -> Option<&Refusal> {
        self.refusal.as_ref()
    }

    /// # Returns
    ///
    /// A newly created read-only panel reading the keys typed at `engine` a second time, so that
    /// what a keystroke asks for is known before any of it is run.
    fn over(engine: Engine) -> Self {
        let keys = Keys::vim();

        Self {
            engine,
            policy: Policy::default(),
            agreed: keys.clone(),
            keys,
            held: Vec::new(),
            refusal: None,
        }
    }
}

/// # Returns
///
/// Whether `action`, run under `context`, leaves the text as it stands. Moving a reader about the
/// text, searching it, scrolling it, redrawing it and saying something about it are all as
/// harmless as a motion, and an editing action is harmless where modalkit says it is. Everything
/// else is not, this module's failure to recognize an action included: a panel that guesses is a
/// panel that eventually guesses wrong, and never writing is the whole of what it promises.
fn readable(action: &Action, context: &EditContext) -> bool {
    match action {
        Action::NoOp
        | Action::Jump(..)
        | Action::KeywordLookup(_)
        | Action::RedrawScreen
        | Action::Scroll(_)
        | Action::Search(..)
        | Action::ShowInfoMessage(_)
        | Action::Suspend => true,
        Action::Editor(editor) => editor.is_readonly(context),
        _ => false,
    }
}

/// # Returns
///
/// `keys` spelled the way a vim manual spells a sequence of keystrokes.
fn spelled(keys: &[KeyEvent]) -> String {
    keys.iter()
        .map(|key| TerminalKey::from(*key).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use editor_types::prelude::{
        Count, EditTarget, MoveDir1D, MoveDir2D, MoveDirMod, PasteStyle, PositionList, RepeatType,
        ScrollSize, ScrollStyle, Specifier,
    };
    use modalkit::actions::{
        Action, EditAction, EditorAction, HistoryAction, InsertTextAction, MacroAction,
    };
    use modalkit::editing::context::EditContext;
    use modalkit::env::vim::VimMode;

    use super::Policy;

    #[test]
    fn a_read_only_policy_allows_the_actions_that_leave_the_text_as_it_stands() {
        for action in readable() {
            assert!(
                Policy::ReadOnly
                    .allows(&[(action.clone(), EditContext::default())], VimMode::Normal),
                "`{action:?}` writes nothing and is refused all the same"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_the_actions_that_would_write() {
        for action in writing() {
            assert!(
                !Policy::ReadOnly
                    .allows(&[(action.clone(), EditContext::default())], VimMode::Normal),
                "`{action:?}` would change the text and is allowed all the same"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_an_action_it_cannot_read_as_harmless() {
        for action in unread() {
            assert!(
                !Policy::ReadOnly
                    .allows(&[(action.clone(), EditContext::default())], VimMode::Normal),
                "`{action:?}` is an action nothing here reads, and is allowed all the same"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_a_keystroke_that_would_leave_the_panel_writing() {
        assert!(!Policy::ReadOnly.allows(&[], VimMode::Insert));
        for mode in [
            VimMode::Normal,
            VimMode::Visual,
            VimMode::Select,
            VimMode::OperationPending,
        ] {
            assert!(
                Policy::ReadOnly.allows(&[], mode),
                "a reader cannot stand in `{mode:?}`"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_a_keystroke_where_any_one_of_its_actions_would_write() {
        let mixed = [
            (readable().remove(0), EditContext::default()),
            (writing().remove(0), EditContext::default()),
        ];

        assert!(!Policy::ReadOnly.allows(&mixed, VimMode::Normal));
    }

    #[test]
    fn an_unrestricted_policy_allows_everything_a_read_only_one_refuses() {
        for action in writing().into_iter().chain(unread()) {
            assert!(
                Policy::Unrestricted.allows(&[(action, EditContext::default())], VimMode::Insert)
            );
        }
    }

    /// # Returns
    ///
    /// Actions that leave the text exactly as it stands.
    fn readable() -> Vec<Action> {
        vec![
            Action::NoOp,
            EditorAction::Edit(
                Specifier::Exact(EditAction::Motion),
                EditTarget::CurrentPosition,
            )
            .into(),
            EditorAction::Edit(
                Specifier::Exact(EditAction::Yank),
                EditTarget::CurrentPosition,
            )
            .into(),
            EditorAction::History(HistoryAction::Checkpoint).into(),
            Action::Jump(PositionList::JumpList, MoveDir1D::Next, Count::Exact(1)),
            Action::RedrawScreen,
            Action::Scroll(ScrollStyle::Direction2D(
                MoveDir2D::Up,
                ScrollSize::Cell,
                Count::Exact(1),
            )),
            Action::Search(MoveDirMod::Same, Count::Exact(1)),
        ]
    }

    /// # Returns
    ///
    /// Actions that would leave the text different from the way it arrived.
    fn writing() -> Vec<Action> {
        vec![
            EditorAction::Edit(
                Specifier::Exact(EditAction::Delete),
                EditTarget::CurrentPosition,
            )
            .into(),
            EditorAction::InsertText(InsertTextAction::Paste(
                PasteStyle::Side(MoveDir1D::Next),
                Count::Exact(1),
            ))
            .into(),
            EditorAction::History(HistoryAction::Undo(Count::Exact(1))).into(),
            EditorAction::History(HistoryAction::Redo(Count::Exact(1))).into(),
        ]
    }

    /// # Returns
    ///
    /// Actions this module reads as neither, which a panel that promises never to write cannot
    /// let through on the grounds that it does not recognize them.
    fn unread() -> Vec<Action> {
        vec![
            Action::Macro(MacroAction::Execute(Count::Exact(1))),
            Action::Repeat(RepeatType::EditSequence),
            Action::Repeat(RepeatType::LastAction),
        ]
    }
}
