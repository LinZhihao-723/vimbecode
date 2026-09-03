//! The ordered sequence of blocks a transcript is.
//!
//! A transcript is read-only and grows only at its end: Claude says another thing and another
//! block is appended, and nothing rewrites what was said before. The blocks are held in the order
//! they were said, and a block is named by its index in that order, which is the coordinate a
//! motion over blocks will move in and the half of a selection's position that says which block it
//! fell in.

use crate::chat::block::Block;

/// The blocks of a conversation, in the order they were said.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Transcript {
    blocks: Vec<Block>,
}

impl Transcript {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created transcript holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    /// Appends `block` to the end of the transcript.
    pub fn push(&mut self, block: Block) {
        self.blocks.push(block);
    }

    /// # Returns
    ///
    /// The blocks of the transcript, in the order they were said.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// # Returns
    ///
    /// The block at `index`, or `None` if the transcript holds no such block.
    #[must_use]
    pub fn block(&self, index: usize) -> Option<&Block> {
        self.blocks.get(index)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

impl FromIterator<Block> for Transcript {
    fn from_iter<BlocksType: IntoIterator<Item = Block>>(blocks: BlocksType) -> Self {
        Self {
            blocks: blocks.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::chat::block::{Block, Kind, Role};

    use super::Transcript;

    #[test]
    fn a_new_transcript_holds_nothing() {
        let transcript = Transcript::new();

        assert_eq!(&[] as &[Block], transcript.blocks());
        assert_eq!(0, transcript.len());
        assert!(transcript.is_empty());
        assert_eq!(None, transcript.block(0));
    }

    #[test]
    fn blocks_are_held_in_the_order_they_were_said() {
        let mut transcript = Transcript::new();
        for block in &said() {
            transcript.push(block.clone());
        }

        assert_eq!(said(), transcript.blocks());
        assert_eq!(3, transcript.len());
        assert!(!transcript.is_empty());
        for (index, block) in said().into_iter().enumerate() {
            assert_eq!(Some(&block), transcript.block(index));
        }
        assert_eq!(None, transcript.block(3));
    }

    #[test]
    fn a_transcript_collected_from_blocks_holds_them_in_the_same_order() {
        let transcript: Transcript = said().into_iter().collect();

        assert_eq!(said(), transcript.blocks());
    }

    /// # Returns
    ///
    /// A short exchange: a question, an answer, and the diff the answer wrote.
    fn said() -> Vec<Block> {
        vec![
            Block::new(Kind::Message(Role::User), "make it compile".to_owned()),
            Block::new(Kind::Message(Role::Assistant), "one line to add".to_owned()),
            Block::diff("fn main() {}\n", "fn main() {\n    todo!();\n}\n"),
        ]
    }
}
