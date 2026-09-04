//! The keys a transcript is read with, and the text the keys are read over.
//!
//! Everything a transcript can be asked for was built before anything could ask: the objects a
//! motion takes, the yanks that carry a block out whole, the folds that collapse what a subagent
//! did. Each of them arrived with a reader of its own that turned a string of typed keys into the
//! command it named, and a reader beside the keybinding table is a second dispatcher: the two
//! disagree about what a key is bound to the moment either is rebound, and neither of them was
//! ever reached by a keystroke because nothing routed keys to it.
//!
//! So there is one table. `iac`, `yac` and `za` are entries of [`crate::keys::Bindings`] beside
//! `dw` and `ciw`, read by the same machine, in the mode each belongs to: a text object in visual
//! mode, where the range it names is the selection it leaves behind; a structural yank under the
//! yank operator that spells it; and a fold command in normal mode. An object is not read in
//! operator-pending, because the operators that would take one are the ones a transcript refuses,
//! and the one that would not spells its own sequences already.
//!
//! What the table cannot do with any of them is emit one of modalkit's editing actions, because a
//! block, a fold and a patch are not addressed in a text's coordinates at all, so the entry names
//! a [`Command`] and the panel above runs it.
//!
//! What the panel runs it over is the transcript flattened. A reader moves through a transcript
//! with vim's own motions, and vim's motions are over a text, so the folded transcript is written
//! out as one -- a closed fold as the one row of its summary, every other block as its own source
//! -- and [`Flattened`] is what carries a position back the other way. A place in that text is a
//! block and a byte of that block's source, which is the coordinate every object, yank and fold
//! is already addressed in, so nothing here reads a rendered row and no answer depends on where
//! the rows broke.

use modalkit::env::vim::VimMode;
use vbc_layout::buffer::LINE_SEPARATOR;

use crate::chat::fold::{Command as Fold, Entry, View};
use crate::chat::object::{Kind as ObjectKind, Object, Position, Scope};
use crate::chat::transcript::Transcript;
use crate::chat::yank::Structure;
use crate::engine::Position as Caret;
use crate::keys::{Bindings, Step};

/// The keys each text object of a transcript is spelled by, which are vim's own `i` and `a`
/// before the two characters naming what is addressed.
const OBJECTS: [(&str, Scope, ObjectKind); 6] = [
    ("iac", Scope::Inner, ObjectKind::Code),
    ("aac", Scope::Around, ObjectKind::Code),
    ("iam", Scope::Inner, ObjectKind::Message),
    ("aam", Scope::Around, ObjectKind::Message),
    ("iat", Scope::Inner, ObjectKind::ToolResult),
    ("aat", Scope::Around, ObjectKind::ToolResult),
];

/// The operator a structural yank is typed under, which is vim's own yank.
const YANK_OPERATOR: &str = "y";

/// The keys each structural yank is spelled by once the yank operator is held, so that the whole
/// sequence is `yac`, `yam`, `yat` or `yad`.
const YANKS: [(&str, Structure); 4] = [
    ("ac", Structure::Code),
    ("am", Structure::Message),
    ("at", Structure::ToolResult),
    ("ad", Structure::Diff),
];

/// The keys each fold command is spelled by, which are vim's own.
const FOLDS: [(&str, Fold); 5] = [
    ("za", Fold::Toggle),
    ("zo", Fold::Open),
    ("zc", Fold::Close),
    ("zR", Fold::OpenAll),
    ("zM", Fold::CloseAll),
];

/// What a key sequence asks of a transcript's own structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    /// Take the object the cursor is in as a selection, which is what `iac` and its fellows are.
    Object(Object),

    /// Take what the cursor is in out of the transcript whole, which is what `yac` and its
    /// fellows are.
    Yank(Structure),

    /// Open or close the folds at the cursor, which is what `za` and its fellows are.
    Fold(Fold),
}

/// # Returns
///
/// The editor's own vim table with the transcript's own sequences bound in it: the text objects
/// in visual mode, the structural yanks under the yank operator, and the fold commands in normal
/// mode.
///
/// # Panics
///
/// Panics if a sequence of the table names a key that cannot be parsed, which is a fault in the
/// table rather than in what a caller asked for.
#[must_use]
pub fn bindings() -> Bindings {
    let mut bindings = Bindings::vim();
    for (keys, scope, kind) in OBJECTS {
        bindings.bind(
            VimMode::Visual,
            keys,
            Step::Chat(Command::Object(Object::new(scope, kind))),
        );
    }
    for (keys, structure) in YANKS {
        bindings.bind_under(YANK_OPERATOR, keys, Step::Chat(Command::Yank(structure)));
    }
    for (keys, command) in FOLDS {
        bindings.bind(VimMode::Normal, keys, Step::Chat(Command::Fold(command)));
    }

    bindings
}

/// One entry of a folded transcript as it was written into the flattened text: which entry it is,
/// which block it draws, and the bytes of that text it takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Placed {
    entry: usize,
    block: usize,
    start: usize,
    end: usize,
    summary: bool,
}

/// A folded transcript written out as one text, and the map back from a place in that text to the
/// block and the byte of its source the place stands for.
///
/// A closed fold is written as the one row its summary is drawn in, which is what makes a motion
/// over the flattened text a motion over the folded transcript: `j` crosses a fold in one step
/// however many blocks it covers, because there is one line there to cross.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Flattened {
    text: String,
    placed: Vec<Placed>,
}

impl Flattened {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The entries `view` draws of `transcript`, written one after another and separated by the
    /// one line break that separates them on the screen.
    #[must_use]
    pub fn of(view: &View<'_>, transcript: &Transcript) -> Self {
        let mut text = String::new();
        let mut placed = Vec::new();
        for (entry, drawn) in view.entries().iter().enumerate() {
            if 0 < entry {
                text.push(LINE_SEPARATOR);
            }
            let start = text.len();
            let summary = matches!(drawn, Entry::Summary(_));
            match drawn {
                Entry::Summary(held) => text.push_str(held.text()),
                Entry::Body(block) => {
                    if let Some(held) = transcript.block(*block) {
                        text.push_str(held.source());
                    }
                }
            }
            placed.push(Placed {
                entry,
                block: drawn.block(),
                start,
                end: text.len(),
                summary,
            });
        }

        Self { text, placed }
    }

    /// # Returns
    ///
    /// The transcript as one text, which is what the engine is laid out over.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// # Returns
    ///
    /// The block and the byte of its source the byte `offset` of the flattened text stands for,
    /// or [`None`] where the text is empty. A byte of a summary row stands for the start of the
    /// block the fold it belongs to heads, since the row itself is drawn from no byte of one.
    #[must_use]
    pub fn at(&self, offset: usize) -> Option<Position> {
        let placed = self.placed_at(offset)?;
        let within = if placed.summary {
            0
        } else {
            offset.min(placed.end) - placed.start
        };

        Some(Position::new(placed.block, within))
    }

    /// # Returns
    ///
    /// The byte of the flattened text the byte `within` of the block `block`'s source is written
    /// at, or [`None`] where the block is folded away or the flattened text holds no such block.
    #[must_use]
    pub fn offset_of(&self, block: usize, within: usize) -> Option<usize> {
        let placed = self
            .placed
            .iter()
            .find(|placed| block == placed.block && !placed.summary)?;

        Some((placed.start + within).min(placed.end))
    }

    /// # Returns
    ///
    /// The byte the entry drawing the block `block` starts at, or [`None`] where no entry draws
    /// it, which is what a block a closed fold covers has.
    #[must_use]
    pub fn start_of(&self, block: usize) -> Option<usize> {
        self.placed
            .iter()
            .find(|placed| block == placed.block)
            .map(|placed| placed.start)
    }

    /// # Returns
    ///
    /// The entry of the folded transcript the byte `offset` falls in, or [`None`] where the text
    /// is empty.
    #[must_use]
    pub fn entry_at(&self, offset: usize) -> Option<usize> {
        self.placed_at(offset).map(|placed| placed.entry)
    }

    /// # Returns
    ///
    /// The byte of the flattened text the cursor rests on where it rests at `at`, which is the
    /// end of the text where `at` names a line the text does not hold.
    #[must_use]
    pub fn caret_at(&self, at: Caret) -> usize {
        let mut offset = 0;
        for line in self.text.split(LINE_SEPARATOR).take(at.line) {
            offset += line.len() + LINE_SEPARATOR.len_utf8();
        }

        (offset + at.column).min(self.text.len())
    }

    /// # Returns
    ///
    /// Where the cursor rests when it rests on the byte `offset` of the flattened text.
    #[must_use]
    pub fn caret_of(&self, offset: usize) -> Caret {
        let offset = offset.min(self.text.len());
        let above = self.text[..offset].matches(LINE_SEPARATOR).count();
        let start = self.text[..offset]
            .rfind(LINE_SEPARATOR)
            .map_or(0, |break_at| break_at + LINE_SEPARATOR.len_utf8());

        Caret {
            line: above,
            column: offset - start,
        }
    }

    /// # Returns
    ///
    /// The entry the byte `offset` falls in, which is the entry above wherever the byte is the
    /// separator between two of them.
    fn placed_at(&self, offset: usize) -> Option<&Placed> {
        let above = self
            .placed
            .partition_point(|placed| placed.start <= offset)
            .checked_sub(1)?;

        self.placed.get(above)
    }
}

#[cfg(test)]
mod tests {
    use modalkit::env::vim::VimMode;

    use crate::chat::block::{Block, Kind, Role};
    use crate::chat::fold::{Folds, Tag, View};
    use crate::chat::object::Position;
    use crate::chat::transcript::Transcript;
    use crate::engine::Position as Caret;
    use crate::keys::{Bindings, Edge, Step};

    use super::{bindings, Command, Flattened, FOLDS, OBJECTS, YANKS, YANK_OPERATOR};

    #[test]
    fn every_sequence_a_transcript_answers_is_an_entry_of_the_one_table() {
        let table = bindings();
        let mut bound = 0;
        for entry in table.entries() {
            if let Step::Chat(_) = entry.step {
                bound += 1;
            }
        }

        assert_eq!(OBJECTS.len() + YANKS.len() + FOLDS.len(), bound);
    }

    #[test]
    fn the_editors_own_table_answers_none_of_them() {
        let table = Bindings::vim();
        for entry in table.entries() {
            assert!(
                !matches!(entry.step, Step::Chat(_)),
                "the editor's own table names a command of a transcript"
            );
        }
    }

    #[test]
    fn a_text_object_is_read_in_the_mode_a_range_is_a_selection_in() {
        let table = bindings();
        for (keys, scope, kind) in OBJECTS {
            let step = step(&table, VimMode::Visual, keys, None);

            assert!(
                matches!(step, Some(Step::Chat(Command::Object(object)))
                    if scope == object.scope() && kind == object.kind()),
                "`{keys}` names no object of a transcript"
            );
        }
    }

    #[test]
    fn a_structural_yank_is_read_under_the_operator_that_spells_it() {
        let table = bindings();
        for (keys, structure) in YANKS {
            let step = step(&table, VimMode::OperationPending, keys, Some(YANK_OPERATOR));

            assert!(
                matches!(step, Some(Step::Chat(Command::Yank(read))) if structure == read),
                "`{YANK_OPERATOR}{keys}` names no structural yank"
            );
        }
    }

    #[test]
    fn a_fold_command_is_read_in_normal_mode() {
        let table = bindings();
        for (keys, command) in FOLDS {
            let step = step(&table, VimMode::Normal, keys, None);

            assert!(
                matches!(step, Some(Step::Chat(Command::Fold(read))) if command == read),
                "`{keys}` names no fold command"
            );
        }
    }

    #[test]
    fn a_flattened_transcript_writes_every_open_block_and_a_closed_fold_as_one_row() {
        let transcript = said();
        let folds = Folds::of(&transcript, &tags());
        let flattened = flatten(&transcript, &folds);
        let rows: Vec<&str> = flattened.text().split('\n').collect();

        assert_eq!("a question", rows[0]);
        assert_eq!("an answer", rows[1]);
        assert!(rows[2].contains("2 lines"), "{:?} is no summary", rows[2]);
        assert_eq!(3, rows.len());
    }

    #[test]
    fn a_place_in_the_flattened_text_names_the_block_and_the_byte_it_was_written_from() {
        let transcript = said();
        let mut folds = Folds::of(&transcript, &tags());
        folds.apply(crate::chat::fold::Command::OpenAll, 2);
        let flattened = flatten(&transcript, &folds);

        assert_eq!(Some(Position::new(0, 0)), flattened.at(0));
        assert_eq!(Some(Position::new(0, 3)), flattened.at(3));
        assert_eq!(Some(Position::new(1, 0)), flattened.at(11));
        assert_eq!(Some(Position::new(2, 5)), flattened.at(26));
        assert_eq!(Some(11), flattened.offset_of(1, 0));
        assert_eq!(None, flattened.offset_of(9, 0));
    }

    #[test]
    fn a_byte_of_a_closed_fold_names_the_block_the_fold_heads() {
        let transcript = said();
        let folds = Folds::of(&transcript, &tags());
        let flattened = flatten(&transcript, &folds);
        let summary = flattened.start_of(2).expect("the fold is drawn");

        assert_eq!(Some(Position::new(2, 0)), flattened.at(summary + 4));
        assert_eq!(None, flattened.offset_of(2, 0));
    }

    #[test]
    fn a_place_is_the_same_place_read_as_a_byte_and_as_a_line_and_a_column() {
        let transcript = said();
        let mut folds = Folds::of(&transcript, &tags());
        folds.apply(crate::chat::fold::Command::OpenAll, 2);
        let flattened = flatten(&transcript, &folds);
        for offset in 0..=flattened.text().len() {
            let caret = flattened.caret_of(offset);

            assert_eq!(offset, flattened.caret_at(caret), "the byte {offset} moved");
        }
        assert_eq!(Caret { line: 1, column: 2 }, flattened.caret_of(13));
    }

    /// # Returns
    ///
    /// What `keys` are bound to in `mode`, under `operator` where they are read under one, and
    /// [`None`] where the table binds them to nothing.
    fn step(
        bindings: &Bindings,
        mode: VimMode,
        keys: &str,
        operator: Option<&str>,
    ) -> Option<Step> {
        bindings
            .entries()
            .iter()
            .find(|entry| {
                mode == entry.mode
                    && operator.is_some() == entry.operator.is_some()
                    && keys == spelled(&entry.keys)
            })
            .map(|entry| entry.step.clone())
    }

    /// # Returns
    ///
    /// How the keys of a bound sequence are spelled.
    fn spelled(edges: &[Edge]) -> String {
        edges
            .iter()
            .map(|edge| match edge {
                Edge::Key(key) => key.to_string(),
                Edge::Any => "{any}".to_owned(),
            })
            .collect()
    }

    /// # Returns
    ///
    /// The transcript `transcript` flattened as `folds` leave it.
    fn flatten(transcript: &Transcript, folds: &Folds) -> Flattened {
        let view = View::of(folds, transcript);

        Flattened::of(&view, transcript)
    }

    /// # Returns
    ///
    /// A short exchange whose last block is one that folds.
    fn said() -> Transcript {
        [
            Block::new(Kind::Message(Role::User), "a question".to_owned()),
            Block::new(Kind::Message(Role::Assistant), "an answer".to_owned()),
            Block::new(Kind::ToolResult, "what it\nanswered".to_owned()),
        ]
        .into_iter()
        .collect()
    }

    /// # Returns
    ///
    /// The tags the blocks of [`said`] arrived with, which say nothing about any of them.
    fn tags() -> Vec<Tag> {
        vec![Tag::untagged(), Tag::untagged(), Tag::untagged()]
    }
}
