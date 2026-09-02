//! Reading a Rust source for the shapes the workspace's guards are about rather than for the names
//! it happens to use.
//!
//! A guard that matches a name is defeated by a rename, and a renderer that lays every line of a
//! document out is one whatever its author called it. What separates that renderer from an honest
//! one is where the call sits rather than what it is called: a layout reached for once, for the
//! line a caller asked about, costs that line, while the same call inside a repetition over the
//! whole text costs the document. So a source is read into its words, and every word is told what
//! the code around it makes of it -- whether it is called, whether it sits inside a repetition
//! over a whole text, and whether it sits in a test.
//!
//! Comments and the text of literals are left out, which is what keeps a guard from firing on a
//! word that was only ever written about.

/// The keywords that open a repetition.
const REPETITIONS: [&str; 3] = ["for", "loop", "while"];

/// The iterator adapters that run what they are given once for every element they are handed.
const ADAPTERS: [&str; 10] = [
    "all",
    "any",
    "filter_map",
    "find_map",
    "flat_map",
    "fold",
    "for_each",
    "map",
    "retain",
    "scan",
];

/// The words that name the whole of a text rather than a bounded part of one, which is what tells
/// a repetition over a document from a walk of the rows around an anchor.
const WHOLE_TEXT: [&str; 8] = [
    "buffer",
    "document",
    "enumerate",
    "into_iter",
    "iter",
    "iter_mut",
    "len",
    "lines",
];

/// A word of code, together with what the code around it makes of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Word {
    text: String,
    line: usize,
    called: bool,
    repeated: bool,
    tested: bool,
}

impl Word {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }

    /// # Returns
    ///
    /// Whether the word names something the source calls here, which a word declaring a function
    /// of that name does not.
    #[must_use]
    pub fn called(&self) -> bool {
        self.called
    }

    /// # Returns
    ///
    /// Whether the word sits inside a repetition over a whole text.
    #[must_use]
    pub fn repeated(&self) -> bool {
        self.repeated
    }

    /// # Returns
    ///
    /// Whether the word sits inside a module that is compiled only for the tests.
    #[must_use]
    pub fn tested(&self) -> bool {
        self.tested
    }
}

/// Reads a Rust source into the words it is written from.
///
/// # Returns
///
/// Every word of code the source holds, in the order it holds them, with its comments and the text
/// of its literals left out.
#[must_use]
pub fn words(source: &str) -> Vec<Word> {
    let characters: Vec<char> = source.chars().collect();
    let mut words: Vec<Word> = Vec::new();
    let mut frames = vec![Frame::default()];
    let mut spoken: Option<usize> = None;
    let mut test = false;
    let mut line = 1;
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];
        let start = index;
        match character {
            '/' if Some(&'/') == characters.get(index + 1) => {
                index = line_comment(&characters, index);
                spoken = None;
            }
            '/' if Some(&'*') == characters.get(index + 1) => {
                index = block_comment(&characters, index);
                spoken = None;
            }
            '"' => {
                index = string(&characters, index);
                spoken = None;
            }
            '\'' => {
                index = character_literal(&characters, index);
                spoken = None;
            }
            '#' => {
                let (end, attribute) = attribute(&characters, index);
                index = end;
                spoken = None;
                test |= "cfg(test)" == attribute;
            }
            '(' | '[' | '{' => {
                let frame = frames.last().expect("the outermost frame is never closed");
                let repetition = opens_a_repetition(character, frame);
                if '(' == character {
                    if let Some(spoken) = spoken {
                        words[spoken].called = true;
                    }
                }

                let opens_a_test = test && '{' == character;
                test &= !opens_a_test;
                frames.push(Frame {
                    header: Vec::new(),
                    repetition,
                    test: opens_a_test,
                });
                spoken = None;
                index += 1;
            }
            ')' | ']' | '}' => {
                if 1 < frames.len() {
                    let closed = frames.pop().expect("a frame is open");
                    let frame = frames
                        .last_mut()
                        .expect("the outermost frame is never closed");
                    if '}' == character {
                        frame.header.clear();
                    } else {
                        frame.header.extend(closed.header);
                    }
                }
                spoken = None;
                index += 1;
            }
            ';' => {
                let frame = frames
                    .last_mut()
                    .expect("the outermost frame is never closed");
                frame.header.clear();
                test = false;
                spoken = None;
                index += 1;
            }
            _ if starts_a_word(character) => {
                let end = word_end(&characters, index);
                let text: String = characters[index..end].iter().collect();
                if let Some(literal) = raw_string(&characters, &text, end) {
                    index = literal;
                    spoken = None;
                } else {
                    let repeated = frames.iter().any(|frame| frame.repetition);
                    let tested = frames.iter().any(|frame| frame.test);
                    let frame = frames
                        .last_mut()
                        .expect("the outermost frame is never closed");
                    frame.header.push(text.clone());
                    words.push(Word {
                        text,
                        line,
                        called: false,
                        repeated,
                        tested,
                    });
                    spoken = Some(words.len() - 1);
                    index = end;
                }
            }
            _ => {
                if !character.is_whitespace() {
                    spoken = None;
                }
                index += 1;
            }
        }
        line += characters[start..index]
            .iter()
            .filter(|character| '\n' == **character)
            .count();
    }

    words
}

/// What a delimiter opened, which is what the words inside it are read against.
#[derive(Debug, Default)]
struct Frame {
    header: Vec<String>,
    repetition: bool,
    test: bool,
}

/// # Returns
///
/// Whether a delimiter opens a repetition over a whole text, which is either a loop walking one or
/// an adapter run over every element of one.
fn opens_a_repetition(delimiter: char, frame: &Frame) -> bool {
    let over_a_whole_text = frame
        .header
        .iter()
        .any(|word| WHOLE_TEXT.contains(&word.as_str()));
    match delimiter {
        '{' => {
            over_a_whole_text
                && frame
                    .header
                    .iter()
                    .any(|word| REPETITIONS.contains(&word.as_str()))
        }
        '(' => {
            over_a_whole_text
                && frame
                    .header
                    .last()
                    .is_some_and(|word| ADAPTERS.contains(&word.as_str()))
        }
        _ => false,
    }
}

/// # Returns
///
/// Whether a character can start a word.
fn starts_a_word(character: char) -> bool {
    character.is_alphabetic() || '_' == character
}

/// # Returns
///
/// The index one past the word starting at `index`.
fn word_end(characters: &[char], index: usize) -> usize {
    let mut cursor = index;
    while characters
        .get(cursor)
        .is_some_and(|character| character.is_alphanumeric() || '_' == *character)
    {
        cursor += 1;
    }

    cursor
}

/// # Returns
///
/// The index of the newline ending the line comment starting at `index`, which is the end of the
/// source for a comment on its last line.
fn line_comment(characters: &[char], index: usize) -> usize {
    let mut cursor = index;
    while characters
        .get(cursor)
        .is_some_and(|character| '\n' != *character)
    {
        cursor += 1;
    }

    cursor
}

/// # Returns
///
/// The index one past the block comment starting at `index`, which is the end of the source where
/// the comment is never closed.
fn block_comment(characters: &[char], index: usize) -> usize {
    let mut cursor = index + 2;
    let mut depth = 1;
    while cursor < characters.len() {
        if '/' == characters[cursor] && Some(&'*') == characters.get(cursor + 1) {
            depth += 1;
            cursor += 2;
        } else if '*' == characters[cursor] && Some(&'/') == characters.get(cursor + 1) {
            depth -= 1;
            cursor += 2;
            if 0 == depth {
                return cursor;
            }
        } else {
            cursor += 1;
        }
    }

    characters.len()
}

/// # Returns
///
/// The index one past the string literal starting at `index`, which is the end of the source where
/// the literal is never closed.
fn string(characters: &[char], index: usize) -> usize {
    let mut cursor = index + 1;
    while cursor < characters.len() {
        match characters[cursor] {
            '\\' => cursor += 2,
            '"' => return cursor + 1,
            _ => cursor += 1,
        }
    }

    characters.len()
}

/// # Returns
///
/// The index one past the raw string literal a `b`, `br` or `r` prefix ending at `index`
/// introduces, or `None` where the prefix introduces no literal at all.
fn raw_string(characters: &[char], prefix: &str, index: usize) -> Option<usize> {
    if !["b", "br", "r"].contains(&prefix) {
        return None;
    }

    let mut cursor = index;
    while Some(&'#') == characters.get(cursor) {
        cursor += 1;
    }
    let hashes = cursor - index;
    if Some(&'"') != characters.get(cursor) {
        return None;
    }

    cursor += 1;
    while cursor < characters.len() {
        if '"' == characters[cursor] {
            let closed = characters[cursor + 1..]
                .iter()
                .take_while(|character| '#' == **character)
                .count();
            if hashes <= closed {
                return Some(cursor + 1 + hashes);
            }
        }
        cursor += 1;
    }

    Some(characters.len())
}

/// # Returns
///
/// The index one past the character literal starting at `index`, which is the index after the
/// quote itself where the quote opens a lifetime rather than a literal.
fn character_literal(characters: &[char], index: usize) -> usize {
    if Some(&'\\') != characters.get(index + 1) && Some(&'\'') != characters.get(index + 2) {
        return index + 1;
    }

    let mut cursor = index + 1;
    while cursor < characters.len() {
        match characters[cursor] {
            '\\' => cursor += 2,
            '\'' => return cursor + 1,
            _ => cursor += 1,
        }
    }

    characters.len()
}

/// # Returns
///
/// The index one past the attribute starting at `index` together with the attribute's own text,
/// with its spaces left out, or the index after `index` and an empty text where no attribute
/// starts there.
fn attribute(characters: &[char], index: usize) -> (usize, String) {
    let mut cursor = index + 1;
    if Some(&'!') == characters.get(cursor) {
        cursor += 1;
    }
    if Some(&'[') != characters.get(cursor) {
        return (index + 1, String::new());
    }

    let start = cursor + 1;
    let mut depth = 0;
    while cursor < characters.len() {
        match characters[cursor] {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if 0 == depth {
                    let text = characters[start..cursor]
                        .iter()
                        .filter(|character| !character.is_whitespace())
                        .collect();
                    return (cursor + 1, text);
                }
            }
            _ => {}
        }
        cursor += 1;
    }

    (characters.len(), String::new())
}
