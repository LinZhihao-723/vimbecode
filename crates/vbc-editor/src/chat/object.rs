//! Addressing what a transcript said as objects a motion can take.
//!
//! A terminal holds cells, so the most a reader of a chat panel can ordinarily ask for is a
//! rectangle of them. A transcript here is not cells: it is the blocks of [`crate::chat::block`],
//! each of which knows what it is and holds the source it was built from. That is what lets a
//! motion ask for the code block, the message or the tool result the cursor is in and be given
//! exactly the bytes that thing is made of.
//!
//! Three objects are addressed, each by the keys vim would spell them with: `iac` and `aac` for a
//! code block, `iam` and `aam` for a message, `iat` and `aat` for a tool result. What the cursor
//! is in decides which of them resolves and to what, and a block of another kind -- a call to a
//! tool, a thinking block, a diff -- is no object of its own, so a position in one resolves to
//! nothing rather than to whatever happened to lie nearby. What such a block fences is another
//! matter: a fenced region is read out of the bytes it is written among rather than out of the
//! kind of the block holding them, so `iac` from inside a fence written in a thinking block names
//! that fence's code, and only the block as a whole is what a block of no kind offers nothing of.
//!
//! An object is either a block or a fenced region written inside one, and the two differ in what
//! the `a` form adds to the `i` form. A fenced region's delimiters are bytes of the source, so
//! `aac` takes the fence lines and `iac` takes the lines between them. A block's delimiter is the
//! block itself and occupies no bytes at all, so `aam` takes the whole source the block holds and
//! `iam` takes that source with the blank lines at either end left off, the way `ap` and `ip`
//! differ over a paragraph.
//!
//! Fenced regions nest, and a region opened inside another is an object in its own right, so a
//! cursor inside a fence written inside a longer one resolves to the inner of them. The rule is
//! the innermost object of the kind asked for that the cursor falls in, which is also what lets
//! `iam` from inside a fenced block still name the message that block was written in.
//!
//! Resolving an object costs the block it is resolved in, because the fence that opened a region
//! is found by reading the lines of that block. Measured in release, `iac` in a fenced block costs
//! 19 µs at a thousand lines, 180 µs at ten thousand and 1.8 ms at a hundred thousand: linear in
//! the bytes of the one block, and untouched by the transcript around it. That is a keystroke's
//! work rather than a frame's, and it is why this is not what a render does: drawing a window of a
//! block costs the window, and only a key asking for an object pays for the block.

use std::ops::Range;

use vbc_layout::buffer::LINE_SEPARATOR;

use crate::chat::block::{self, Block};
use crate::chat::transcript::Transcript;

/// The shortest run of fence characters that opens or closes a fenced region.
const FENCE: usize = 3;

/// The deepest a fence line may be indented and still be read as a fence.
const INDENT: usize = 3;

/// The characters a fence may be written with.
const FENCES: [char; 2] = ['`', '~'];

/// What an object addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// A fenced region, or a block the transcript holds the code of on its own.
    Code,

    /// Prose said by one side of the conversation.
    Message,

    /// What a tool answered.
    ToolResult,
}

/// How much of an object is taken: the thing itself, or the thing with what delimits it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// The object with its delimiters: the fence lines of a fenced region, or the blank lines at
    /// either end of a block.
    Around,

    /// The object without them.
    Inner,
}

/// A text object: what it addresses, and how much of it it takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Object {
    scope: Scope,
    kind: Kind,
}

impl Object {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The object taking `scope` of a thing of `kind`.
    #[must_use]
    pub fn new(scope: Scope, kind: Kind) -> Self {
        Self { scope, kind }
    }

    #[must_use]
    pub fn scope(&self) -> Scope {
        self.scope
    }

    #[must_use]
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Resolves the object `at` falls in.
    ///
    /// Where the position falls in more than one object of the kind, which is what a fenced region
    /// written inside another leaves it in, the innermost of them is the one resolved.
    ///
    /// # Returns
    ///
    /// The region of the transcript the object names, or `None` where `at` names no block, falls
    /// past the last byte of the block it names, or falls in no object of the kind.
    #[must_use]
    pub fn resolve(&self, transcript: &Transcript, at: Position) -> Option<Region> {
        let block = transcript.block(at.block)?;
        if !addressable(block.source(), at.offset) {
            return None;
        }

        let found = candidates(block)
            .into_iter()
            .filter(|candidate| self.kind == candidate.kind && candidate.holds(at.offset))
            .min_by_key(|candidate| candidate.around.end - candidate.around.start)?;

        Some(Region {
            block: at.block,
            range: match self.scope {
                Scope::Around => found.around,
                Scope::Inner => found.inner,
            },
        })
    }
}

/// A position in a transcript: the block it falls in, and the byte of that block's source it falls
/// on.
///
/// The offset is a byte rather than a cluster, and every byte of a cluster addresses the same
/// objects, so a cursor drawn on a wide or a joined character resolves what that character is
/// written in wherever inside it the offset happens to fall.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    block: usize,
    offset: usize,
}

impl Position {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The position at byte `offset` of the source of the block at `block`.
    #[must_use]
    pub fn new(block: usize, offset: usize) -> Self {
        Self { block, offset }
    }

    #[must_use]
    pub fn block(&self) -> usize {
        self.block
    }

    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }
}

/// What an object resolved to: the block it was found in, and the byte range of that block's
/// source it names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Region {
    block: usize,
    range: Range<usize>,
}

impl Region {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The region naming `range` of the source of the block at `block`.
    #[must_use]
    pub fn new(block: usize, range: Range<usize>) -> Self {
        Self { block, range }
    }

    #[must_use]
    pub fn block(&self) -> usize {
        self.block
    }

    #[must_use]
    pub fn range(&self) -> &Range<usize> {
        &self.range
    }

    /// # Returns
    ///
    /// The source the region names, or `None` where `transcript` holds no such block or the region
    /// is not a range of that block's source.
    #[must_use]
    pub fn text<'transcript>(
        &self,
        transcript: &'transcript Transcript,
    ) -> Option<&'transcript str> {
        transcript.block(self.block)?.slice(self.range.clone())
    }
}

/// One object a block offers, held as both of the ranges it can be taken as.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Candidate {
    kind: Kind,
    around: Range<usize>,
    inner: Range<usize>,
}

impl Candidate {
    /// # Returns
    ///
    /// Whether a cursor at `offset` falls in the object, which the first byte of one taking no
    /// bytes at all does.
    fn holds(&self, offset: usize) -> bool {
        self.around.start <= offset && offset < self.around.end.max(self.around.start + 1)
    }
}

/// A fenced region that has been opened and not yet closed.
struct Open {
    character: char,
    length: usize,
    start: usize,
    content: usize,
}

impl Open {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The region `fence` opens, whose fence line covers `line` of a source of `length` bytes.
    fn new(fence: &Fence, line: Range<usize>, length: usize) -> Self {
        Self {
            character: fence.character,
            length: fence.length,
            start: line.start,
            content: (line.end + LINE_SEPARATOR.len_utf8()).min(length),
        }
    }

    /// # Returns
    ///
    /// The object the region is, closed by the fence line covering `line`.
    fn closed_by(self, line: Range<usize>) -> Candidate {
        let end = if self.content < line.start {
            line.start - LINE_SEPARATOR.len_utf8()
        } else {
            self.content
        };

        Candidate {
            kind: Kind::Code,
            around: self.start..line.end,
            inner: self.content..end,
        }
    }

    /// # Returns
    ///
    /// The object the region is, ended at `end` by whatever closed the region around it or by the
    /// end of the block rather than by a fence line of its own.
    fn ended_at(self, end: usize) -> Candidate {
        Candidate {
            kind: Kind::Code,
            around: self.start..end,
            inner: self.content.min(end)..end,
        }
    }
}

/// A fence line, read as what it does to the regions open around it.
struct Fence {
    character: char,
    length: usize,
    named: bool,
}

/// # Returns
///
/// Every object `block` offers, in no particular order.
fn candidates(block: &Block) -> Vec<Candidate> {
    let source = block.source();
    let mut found = fenced(source);
    let Some(kind) = whole(block.kind()) else {
        return found;
    };

    let around = 0..source.len();
    if found
        .iter()
        .any(|candidate| kind == candidate.kind && around == candidate.around)
    {
        return found;
    }
    found.push(Candidate {
        kind,
        inner: body(source),
        around,
    });

    found
}

/// # Returns
///
/// The kind of object a block of `kind` is as a whole, or `None` where a block of it is no object
/// of a transcript.
fn whole(kind: &block::Kind) -> Option<Kind> {
    match kind {
        block::Kind::Message(_) => Some(Kind::Message),
        block::Kind::Code { .. } => Some(Kind::Code),
        block::Kind::ToolResult => Some(Kind::ToolResult),
        block::Kind::ToolCall { .. } | block::Kind::Thinking | block::Kind::Diff { .. } => None,
    }
}

/// Reads `source` for the fenced regions written in it.
///
/// A fence line naming a language opens a region. A bare fence line closes the innermost region
/// opened with the same character by a fence no longer than its own, and opens one where there is
/// no such region. A region left open by the end of the source, or by the fence that closed the
/// region around it, ends there.
///
/// # Returns
///
/// The object every fenced region of `source` is, in the order they close, which is a region
/// written inside another before the one written around it.
fn fenced(source: &str) -> Vec<Candidate> {
    let mut found = Vec::new();
    let mut open: Vec<Open> = Vec::new();

    for (start, text) in lines(source) {
        let Some(fence) = fence_of(text) else {
            continue;
        };
        let line = start..start + text.len();
        let closing = if fence.named {
            None
        } else {
            open.iter().rposition(|region| {
                fence.character == region.character && region.length <= fence.length
            })
        };
        let Some(index) = closing else {
            open.push(Open::new(&fence, line, source.len()));
            continue;
        };

        while index < open.len() {
            let region = open
                .pop()
                .expect("the regions above the one being closed are open");
            found.push(if index == open.len() {
                region.closed_by(line.clone())
            } else {
                region.ended_at(line.start.saturating_sub(LINE_SEPARATOR.len_utf8()))
            });
        }
    }

    let end = source
        .strip_suffix(LINE_SEPARATOR)
        .map_or(source.len(), str::len);
    while let Some(region) = open.pop() {
        found.push(region.ended_at(end));
    }

    found
}

/// A fence line is a run of at least [`FENCE`] of one of [`FENCES`], under no more than [`INDENT`]
/// spaces, and what follows the run names the language where it is not blank. A run of backticks
/// naming a language that itself holds a backtick is no fence, because what is written there is a
/// span of inline code rather than the opening of a region.
///
/// # Returns
///
/// What `text` does to the regions open around it, or `None` where it is no fence line.
fn fence_of(text: &str) -> Option<Fence> {
    let indent = text.len() - text.trim_start_matches(' ').len();
    if INDENT < indent {
        return None;
    }

    let rest = &text[indent..];
    let character = rest.chars().next().filter(|first| FENCES.contains(first))?;
    let after = rest.trim_start_matches(character);
    let length = rest.len() - after.len();
    if length < FENCE {
        return None;
    }

    let named = after.trim();
    if '`' == character && named.contains('`') {
        return None;
    }

    Some(Fence {
        character,
        length,
        named: !named.is_empty(),
    })
}

/// # Returns
///
/// What is left of `source` when the blank lines at either end of it are left off, which is a
/// range of no bytes where every line of it is blank.
fn body(source: &str) -> Range<usize> {
    let mut start = None;
    let mut end = 0;

    for (at, text) in lines(source) {
        if text.trim().is_empty() {
            continue;
        }
        start.get_or_insert(at);
        end = at + text.len();
    }

    start.map_or(0..0, |start| start..end)
}

/// # Returns
///
/// Every logical line of `source`, each with the byte offset it starts at and its separator left
/// off.
fn lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;

    std::iter::from_fn(move || {
        if source.len() < offset {
            return None;
        }

        let rest = &source[offset..];
        let text = rest.find(LINE_SEPARATOR).map_or(rest, |at| &rest[..at]);
        let start = offset;
        offset += text.len() + LINE_SEPARATOR.len_utf8();

        Some((start, text))
    })
}

/// # Returns
///
/// Whether a cursor may sit at byte `offset` of `source`, which the first byte of a source of
/// nothing is and the byte past the last of any other source is not.
fn addressable(source: &str, offset: usize) -> bool {
    offset < source.len() || (source.is_empty() && 0 == offset)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ops::Range;

    use vbc_layout::width::grapheme_indices;

    use crate::chat::block::{Block, Kind as BlockKind, Role};
    use crate::chat::transcript::Transcript;

    use super::{Kind, Object, Position, Region, Scope};

    /// The blocks of the fixture transcript, named by where they sit in it.
    const QUESTION: usize = 0;
    const ANSWER: usize = 1;
    const CALL: usize = 2;
    const RESULT: usize = 3;
    const CODE: usize = 4;
    const NESTING: usize = 5;
    const WIDE: usize = 6;
    const THOUGHT: usize = 7;
    const EDIT: usize = 8;

    /// The keys every object of a transcript is spelled with.
    const KEYS: [&str; 6] = ["iac", "aac", "iam", "aam", "iat", "aat"];

    /// The question the fixture opens with, which is prose and nothing else.
    const ASKED: &str = "make the panel wrap\n";

    /// The answer, which fences a block of code in the middle of its prose.
    const ANSWERED: &str = concat!(
        "here is the fix\n",
        "\n",
        "```rust\n",
        "fn main() {\n",
        "    todo!();\n",
        "}\n",
        "```\n",
        "\n",
        "and that is all\n",
    );

    /// The code that answer fenced, and the same code with the fence around it.
    const FENCED: &str = "fn main() {\n    todo!();\n}";
    const FENCE: &str = "```rust\nfn main() {\n    todo!();\n}\n```";

    /// What the tool answered, ending in the separator its last line was written with.
    const RAN: &str = "running 1 test\ntest layout ... ok\n";

    /// A block of code the transcript holds on its own, whose fence is not among its bytes.
    const HELPER: &str = "fn helper() -> usize {\n    1\n}\n";

    /// A message showing markdown, which writes a fence inside a longer one.
    const NESTED: &str = concat!(
        "look:\n",
        "\n",
        "````markdown\n",
        "text\n",
        "```rust\n",
        "let x = 1;\n",
        "```\n",
        "more\n",
        "````\n",
    );

    /// A line of clusters that are neither one code point nor one column: CJK, a letter carrying a
    /// combining mark, and a joined emoji.
    const WIDE_LINE: &str = concat!(
        "\u{4e2d}\u{6587} e\u{301} ",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} \u{884c}",
    );

    #[test]
    fn every_object_names_the_same_bytes_from_the_first_of_them_the_last_and_the_middle() {
        let transcript = transcript();

        for (keys, block, source, wanted) in objects() {
            let expected = region(block, source, wanted);
            let cursors = cursors(source, expected.range());
            assert!(
                2 < cursors.len(),
                "`{keys}` names too few bytes for a cursor to be put in the middle of them"
            );

            for offset in cursors {
                assert_eq!(
                    Some(expected.clone()),
                    resolve(&transcript, keys, block, offset),
                    "`{keys}` from byte {offset} named other bytes"
                );
            }
        }
    }

    #[test]
    fn the_inner_code_object_leaves_the_fence_out_and_the_around_object_takes_it() {
        let transcript = transcript();
        let inside = at(ANSWERED, "todo");
        let opening = at(ANSWERED, "```rust");
        let closing = ANSWERED.rfind("```").expect("the fixture closes its fence") + "``".len();

        let inner = resolve(&transcript, "iac", ANSWER, inside);
        let around = resolve(&transcript, "aac", ANSWER, inside);
        assert_eq!(Some(FENCED), text(&transcript, &inner));
        assert_eq!(Some(FENCE), text(&transcript, &around));
        assert!(
            !FENCED.contains("```"),
            "the bytes the inner object is expected to name hold a fence"
        );

        for offset in [opening, closing] {
            assert_eq!(
                inner,
                resolve(&transcript, "iac", ANSWER, offset),
                "a cursor on the fence at byte {offset} did not resolve what it fences"
            );
            assert_eq!(around, resolve(&transcript, "aac", ANSWER, offset));
        }
    }

    #[test]
    fn the_inner_message_leaves_the_blank_lines_of_a_block_out_and_the_around_message_takes_them() {
        let transcript = transcript();

        let inner = resolve(&transcript, "iam", ANSWER, 0);
        let around = resolve(&transcript, "aam", ANSWER, 0);
        assert_eq!(
            Some(ANSWERED.trim_end_matches('\n')),
            text(&transcript, &inner)
        );
        assert_eq!(Some(ANSWERED), text(&transcript, &around));
        assert_ne!(inner, around);

        let padded = "\n\nhere is the fix\n\nand that is all\n\n";
        let spaced = said(padded);
        assert_eq!(
            Some("here is the fix\n\nand that is all"),
            text(&spaced, &resolve(&spaced, "iam", 0, 0)),
            "the blank lines at the start of a block were taken by the inner object"
        );
        assert_eq!(Some(padded), text(&spaced, &resolve(&spaced, "aam", 0, 0)));

        let whitespace = "  \nhere is the fix\n   \n";
        let blank = said(whitespace);
        assert_eq!(
            Some("here is the fix"),
            text(&blank, &resolve(&blank, "iam", 0, 0)),
            "a line holding nothing but spaces was not read as a blank line"
        );
    }

    #[test]
    fn an_object_the_position_falls_in_none_of_resolves_to_nothing() {
        let transcript = transcript();
        let prose = at(ANSWERED, "here is the fix");

        for (keys, block, offset) in [
            ("iat", ANSWER, prose),
            ("aat", ANSWER, prose),
            ("iac", ANSWER, prose),
            ("aac", ANSWER, prose),
            ("iac", QUESTION, 0),
            ("aac", QUESTION, 0),
            ("iam", RESULT, 0),
            ("aam", RESULT, 0),
            ("iat", QUESTION, 0),
            ("aat", QUESTION, 0),
            ("aam", transcript.len(), 0),
            ("aam", ANSWER, ANSWERED.len()),
        ] {
            assert_eq!(
                None,
                resolve(&transcript, keys, block, offset),
                "`{keys}` at byte {offset} of block {block} named bytes of its own"
            );
        }

        for block in [CALL, THOUGHT, EDIT] {
            for keys in KEYS {
                assert_eq!(
                    None,
                    resolve(&transcript, keys, block, 0),
                    "`{keys}` named bytes of block {block}, which is no object of a transcript"
                );
            }
        }
    }

    #[test]
    fn a_fence_written_inside_another_resolves_to_the_innermost_the_cursor_falls_in() {
        let transcript = transcript();
        let outer_body = "text\n```rust\nlet x = 1;\n```\nmore";
        let outer_fence = "````markdown\ntext\n```rust\nlet x = 1;\n```\nmore\n````";
        let inside = at(NESTED, "let x = 1;");

        assert_eq!(
            Some("let x = 1;"),
            text(&transcript, &resolve(&transcript, "iac", NESTING, inside))
        );
        assert_eq!(
            Some("```rust\nlet x = 1;\n```"),
            text(&transcript, &resolve(&transcript, "aac", NESTING, inside))
        );
        assert_eq!(
            Some("let x = 1;"),
            text(
                &transcript,
                &resolve(&transcript, "iac", NESTING, at(NESTED, "```rust"))
            ),
            "a cursor on the inner fence resolved the fence around it"
        );

        for offset in [at(NESTED, "text"), at(NESTED, "more")] {
            assert_eq!(
                Some(outer_body),
                text(&transcript, &resolve(&transcript, "iac", NESTING, offset))
            );
            assert_eq!(
                Some(outer_fence),
                text(&transcript, &resolve(&transcript, "aac", NESTING, offset))
            );
        }

        assert_eq!(
            Some(NESTED),
            text(&transcript, &resolve(&transcript, "aam", NESTING, inside)),
            "the message the fences were written in was lost from inside the innermost of them"
        );
    }

    #[test]
    fn a_fence_naming_a_language_opens_a_region_inside_the_one_around_it_rather_than_closing_it() {
        let source = "```text\nbefore\n```rust\nlet y = 2;\n```\nafter\n";
        let transcript = said(source);

        assert_eq!(
            Some("let y = 2;"),
            text(
                &transcript,
                &resolve(&transcript, "iac", 0, at(source, "let y = 2;"))
            )
        );
        assert_eq!(
            Some("before\n```rust\nlet y = 2;\n```\nafter"),
            text(
                &transcript,
                &resolve(&transcript, "iac", 0, at(source, "before"))
            ),
            "a fence naming a language closed the region it was written inside"
        );
    }

    #[test]
    fn an_object_over_wide_and_joined_clusters_names_whole_clusters_from_anywhere_inside_one() {
        let transcript = transcript();
        let source = clustered();
        let inner = region(WIDE, &source, WIDE_LINE);
        let around = region(WIDE, &source, &format!("```text\n{WIDE_LINE}\n```"));
        let boundaries = clusters(&source);

        let swept: Vec<usize> = around.range().clone().collect();
        assert!(
            swept.iter().any(|offset| !boundaries.contains(offset)),
            "the fixture holds no cluster of more than one byte to put a cursor inside of"
        );
        for offset in swept {
            assert_eq!(
                Some(inner.clone()),
                resolve(&transcript, "iac", WIDE, offset),
                "`iac` from byte {offset} named other bytes"
            );
            assert_eq!(
                Some(around.clone()),
                resolve(&transcript, "aac", WIDE, offset),
                "`aac` from byte {offset} named other bytes"
            );
        }

        for edge in [
            inner.range().start,
            inner.range().end,
            around.range().start,
            around.range().end,
        ] {
            assert!(
                boundaries.contains(&edge),
                "an object was cut at byte {edge}, which is inside a cluster of {source:?}"
            );
        }
        assert_eq!(Some(WIDE_LINE), text(&transcript, &Some(inner)));
    }

    #[test]
    fn a_block_of_code_the_transcript_holds_on_its_own_has_no_fence_among_its_bytes() {
        let transcript = transcript();

        assert!(!HELPER.contains("```"));
        assert_eq!(
            Some("fn helper() -> usize {\n    1\n}"),
            text(&transcript, &resolve(&transcript, "iac", CODE, 0))
        );
        assert_eq!(
            Some(HELPER),
            text(&transcript, &resolve(&transcript, "aac", CODE, 0))
        );
    }

    #[test]
    fn a_block_of_code_that_holds_its_own_fence_is_one_object_rather_than_two() {
        let source = "```rust\nfn a() {}\n```";
        let transcript: Transcript = std::iter::once(Block::new(
            BlockKind::Code {
                language: Some("rust".to_owned()),
            },
            source.to_owned(),
        ))
        .collect();

        assert_eq!(
            Some("fn a() {}"),
            text(
                &transcript,
                &resolve(&transcript, "iac", 0, at(source, "fn a"))
            )
        );
        assert_eq!(
            Some(source),
            text(&transcript, &resolve(&transcript, "aac", 0, 0))
        );
    }

    #[test]
    fn a_fence_closes_only_a_region_of_its_own_character_that_is_no_longer_than_it_is() {
        let mixed = "~~~text\nabove\n```\ncode\n```\nbelow\n~~~\n";
        let longer = "````markdown\nabove\n```\ncode\n```\nbelow\n````\n";

        for source in [mixed, longer] {
            let transcript = said(source);

            assert_eq!(
                Some("above\n```\ncode\n```\nbelow"),
                text(
                    &transcript,
                    &resolve(&transcript, "iac", 0, at(source, "above"))
                ),
                "a fence of another character or another length closed {source:?}"
            );
            assert_eq!(
                Some("code"),
                text(
                    &transcript,
                    &resolve(&transcript, "iac", 0, at(source, "code"))
                )
            );
        }
    }

    #[test]
    fn a_fence_is_read_where_it_is_written_with_tildes_or_indented_a_little_but_not_a_lot() {
        let with_tildes = "~~~python\nprint(1)\n~~~\n";
        let indented = "  ```rust\n  let a = 1;\n  ```\n";
        let buried = "    ```rust\n    let a = 1;\n    ```\n";

        let tilde = said(with_tildes);
        assert_eq!(
            Some("print(1)"),
            text(
                &tilde,
                &resolve(&tilde, "iac", 0, at(with_tildes, "print(1)"))
            )
        );
        assert_eq!(
            Some("~~~python\nprint(1)\n~~~"),
            text(&tilde, &resolve(&tilde, "aac", 0, 0))
        );

        let little = said(indented);
        assert_eq!(
            Some("  let a = 1;"),
            text(
                &little,
                &resolve(&little, "iac", 0, at(indented, "let a = 1;"))
            )
        );

        let lot = said(buried);
        assert_eq!(
            None,
            resolve(&lot, "iac", 0, at(buried, "let a = 1;")),
            "a fence buried under four spaces of indent was read as a fence"
        );

        let trailed = "```rust\nlet a = 1;\n```  \nafter\n";
        let spaced = said(trailed);
        assert_eq!(
            Some("let a = 1;"),
            text(
                &spaced,
                &resolve(&spaced, "iac", 0, at(trailed, "let a = 1;"))
            ),
            "the spaces written after a closing fence were read as the language it names"
        );
        assert_eq!(
            None,
            resolve(&spaced, "iac", 0, at(trailed, "after")),
            "the region ran on past the fence that closed it"
        );
    }

    #[test]
    fn a_fence_left_open_runs_to_the_end_of_the_block() {
        let source = "start\n```rust\nfn a() {}\n";
        let transcript = said(source);
        let inside = at(source, "fn a");

        assert_eq!(
            Some("fn a() {}"),
            text(&transcript, &resolve(&transcript, "iac", 0, inside))
        );
        assert_eq!(
            Some("```rust\nfn a() {}"),
            text(&transcript, &resolve(&transcript, "aac", 0, inside))
        );

        let both = "~~~outer\n```rust\nfn a() {}\n";
        let open = said(both);
        assert_eq!(
            Some("```rust\nfn a() {}"),
            text(&open, &resolve(&open, "aac", 0, at(both, "fn a")))
        );
        assert_eq!(
            Some("~~~outer\n```rust\nfn a() {}"),
            text(&open, &resolve(&open, "aac", 0, at(both, "outer"))),
            "a region left open around another was lost at the end of the block"
        );
    }

    #[test]
    fn a_message_of_nothing_is_an_object_of_no_bytes_rather_than_no_object() {
        let transcript: Transcript =
            std::iter::once(Block::new(BlockKind::Message(Role::User), String::new())).collect();

        assert_eq!(
            Some(Region::new(0, 0..0)),
            resolve(&transcript, "iam", 0, 0)
        );
        assert_eq!(
            Some(Region::new(0, 0..0)),
            resolve(&transcript, "aam", 0, 0)
        );
        assert_eq!(None, resolve(&transcript, "iac", 0, 0));
    }

    #[test]
    fn a_run_of_fewer_fence_characters_than_a_fence_takes_is_no_fence_at_all() {
        let short = "``text\nnot code\n``\n";
        let shortest = "~text\nnot code\n~\n";

        for source in [short, shortest] {
            let transcript = said(source);

            assert_eq!(
                None,
                resolve(&transcript, "iac", 0, at(source, "not code")),
                "a run of fewer than three characters opened a region of {source:?}"
            );
            assert_eq!(
                Some(source.trim_end_matches('\n')),
                text(&transcript, &resolve(&transcript, "iam", 0, 0)),
                "the lines that are no fence were left out of the message holding them"
            );
        }
    }

    #[test]
    fn a_fence_of_backticks_naming_a_language_that_holds_one_is_no_fence_either() {
        let inline = "```a`b\ncode\n```\n";
        let named = "```ab\ncode\n```\n";

        let spanned = said(inline);
        assert_eq!(
            None,
            resolve(&spanned, "iac", 0, at(inline, "code")),
            "a line of inline code was read as the fence opening a region"
        );

        let opened = said(named);
        assert_eq!(
            Some("code"),
            text(&opened, &resolve(&opened, "iac", 0, at(named, "code"))),
            "the same line without the backtick in its language opened no region"
        );

        let tilde = "~~~a`b\ncode\n~~~\n";
        let fenced = said(tilde);
        assert_eq!(
            Some("code"),
            text(&fenced, &resolve(&fenced, "iac", 0, at(tilde, "code"))),
            "a backtick in the language of a fence of tildes stopped it opening a region"
        );
    }

    #[test]
    fn a_region_the_fence_around_it_closed_over_ends_above_that_fence_line() {
        let source = "~~~outer\n```inner\ncode\n~~~\nafter\n";
        let transcript = said(source);
        let inside = at(source, "code");

        assert_eq!(
            Some("```inner\ncode"),
            text(&transcript, &resolve(&transcript, "aac", 0, inside)),
            "a region left open took the fence line that closed the region around it"
        );
        assert_eq!(
            Some("code"),
            text(&transcript, &resolve(&transcript, "iac", 0, inside))
        );
        assert_eq!(
            Some("~~~outer\n```inner\ncode\n~~~"),
            text(
                &transcript,
                &resolve(&transcript, "aac", 0, at(source, "outer"))
            ),
            "the region written around the one left open was not closed by its own fence"
        );
    }

    #[test]
    fn a_fence_written_in_a_block_that_is_no_object_still_fences_a_region_of_code() {
        let thought = "reasoning\n```rust\nlet x = 1;\n```\ndone\n";
        let transcript: Transcript =
            std::iter::once(Block::new(BlockKind::Thinking, thought.to_owned())).collect();
        let inside = at(thought, "let x = 1;");

        assert_eq!(
            Some("let x = 1;"),
            text(&transcript, &resolve(&transcript, "iac", 0, inside))
        );
        assert_eq!(
            Some("```rust\nlet x = 1;\n```"),
            text(&transcript, &resolve(&transcript, "aac", 0, inside))
        );
        for keys in ["iam", "aam", "iat", "aat"] {
            assert_eq!(
                None,
                resolve(&transcript, keys, 0, inside),
                "`{keys}` named bytes of a thinking block, which is no object of a transcript"
            );
        }
        assert_eq!(
            None,
            resolve(&transcript, "iac", 0, at(thought, "reasoning")),
            "the block itself was an object of code outside the region it fenced"
        );
    }

    /// # Returns
    ///
    /// Every object of the fixture transcript, as the keys spelling it, the block it is in, the
    /// source of that block, and the bytes of that source it is expected to name.
    fn objects() -> Vec<(&'static str, usize, &'static str, &'static str)> {
        vec![
            ("iac", ANSWER, ANSWERED, FENCED),
            ("aac", ANSWER, ANSWERED, FENCE),
            ("iam", ANSWER, ANSWERED, ANSWERED.trim_end_matches('\n')),
            ("aam", ANSWER, ANSWERED, ANSWERED),
            ("iam", QUESTION, ASKED, ASKED.trim_end_matches('\n')),
            ("aam", QUESTION, ASKED, ASKED),
            ("iat", RESULT, RAN, RAN.trim_end_matches('\n')),
            ("aat", RESULT, RAN, RAN),
        ]
    }

    /// # Returns
    ///
    /// A transcript holding a block of every kind an object may be resolved in, and of every kind
    /// none may.
    fn transcript() -> Transcript {
        vec![
            Block::new(BlockKind::Message(Role::User), ASKED.to_owned()),
            Block::new(BlockKind::Message(Role::Assistant), ANSWERED.to_owned()),
            Block::new(
                BlockKind::ToolCall {
                    name: "Bash".to_owned(),
                },
                "cargo test -p vbc-editor".to_owned(),
            ),
            Block::new(BlockKind::ToolResult, RAN.to_owned()),
            Block::new(
                BlockKind::Code {
                    language: Some("rust".to_owned()),
                },
                HELPER.to_owned(),
            ),
            Block::new(BlockKind::Message(Role::Assistant), NESTED.to_owned()),
            Block::new(BlockKind::Message(Role::Assistant), clustered()),
            Block::new(BlockKind::Thinking, "a fence is bytes\n".to_owned()),
            Block::diff(
                "src/main.rs".to_owned(),
                "fn main() {}\n",
                "fn main() {\n    todo!();\n}\n",
            ),
        ]
        .into_iter()
        .collect()
    }

    /// # Returns
    ///
    /// A transcript of one message of `source`.
    fn said(source: &str) -> Transcript {
        std::iter::once(Block::new(
            BlockKind::Message(Role::Assistant),
            source.to_owned(),
        ))
        .collect()
    }

    /// # Returns
    ///
    /// A message fencing a line of clusters that are neither one code point nor one column.
    fn clustered() -> String {
        format!("\u{770b}\u{8fd9}\u{91cc}\n```text\n{WIDE_LINE}\n```\n")
    }

    /// # Returns
    ///
    /// The region the object `keys` spells resolves to with the cursor at byte `offset` of the
    /// block at `block`.
    ///
    /// # Panics
    ///
    /// Panics if `keys` spell no object of a transcript, which is a fault in a fixture.
    fn resolve(transcript: &Transcript, keys: &str, block: usize, offset: usize) -> Option<Region> {
        let object = match keys {
            "iac" => Object::new(Scope::Inner, Kind::Code),
            "aac" => Object::new(Scope::Around, Kind::Code),
            "iam" => Object::new(Scope::Inner, Kind::Message),
            "aam" => Object::new(Scope::Around, Kind::Message),
            "iat" => Object::new(Scope::Inner, Kind::ToolResult),
            "aat" => Object::new(Scope::Around, Kind::ToolResult),
            other => panic!("`{other}` spells no object of a transcript"),
        };

        object.resolve(transcript, Position::new(block, offset))
    }

    /// # Returns
    ///
    /// The source `region` names, or `None` where it named nothing at all.
    fn text<'transcript>(
        transcript: &'transcript Transcript,
        region: &Option<Region>,
    ) -> Option<&'transcript str> {
        region.as_ref().map(|region| {
            region
                .text(transcript)
                .expect("a region names a range of its own block")
        })
    }

    /// # Returns
    ///
    /// The region of the block at `block` naming the first `wanted` of `source`.
    fn region(block: usize, source: &str, wanted: &str) -> Region {
        let start = at(source, wanted);

        Region::new(block, start..start + wanted.len())
    }

    /// # Returns
    ///
    /// The byte of `source` the first `wanted` of it starts at.
    fn at(source: &str, wanted: &str) -> usize {
        source
            .find(wanted)
            .expect("a fixture holds the bytes an object is expected to name")
    }

    /// # Returns
    ///
    /// The first byte of `range` a cursor may sit on, the last, and the one halfway through it,
    /// each of them the first byte of the cluster it falls in.
    fn cursors(source: &str, range: &Range<usize>) -> Vec<usize> {
        let starts: Vec<usize> = grapheme_indices(source)
            .map(|(at, _)| at)
            .filter(|at| range.contains(at))
            .collect();
        let Some(first) = starts.first() else {
            return vec![range.start];
        };

        vec![
            *first,
            starts[starts.len() / 2],
            *starts.last().expect("the range holds a cluster"),
        ]
    }

    /// # Returns
    ///
    /// Every byte of `source` a grapheme cluster begins at, the end of the source included, which
    /// are the only bytes an object may be cut at.
    fn clusters(source: &str) -> BTreeSet<usize> {
        grapheme_indices(source)
            .map(|(offset, _)| offset)
            .chain(std::iter::once(source.len()))
            .collect()
    }
}
