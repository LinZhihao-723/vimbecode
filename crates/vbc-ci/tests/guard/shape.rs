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
//! A loop reaches the whole of a text through the words naming one, and it reaches the whole of a
//! text just as surely through a local the extent was hoisted into: `while offset <= end` walks a
//! document whenever `end` was bound from one, and a guard that reads only the loop's own header
//! is defeated by moving the call that measures the text one line up. So a name bound from the
//! whole of a text stands for one wherever it is read, for as long as the block that bound it, and
//! so does a name bound from that name, since passing an extent along says nothing about the walk.
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
pub const WHOLE_TEXT: [&str; 8] = [
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
                let repetition = opens_a_repetition(character, &frames);
                if '(' == character {
                    if let Some(spoken) = spoken {
                        words[spoken].called = true;
                    }
                }

                let opens_a_test = test && '{' == character;
                test &= !opens_a_test;
                frames.push(Frame {
                    header: Vec::new(),
                    bound: Vec::new(),
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
                let bound = hoisted(&frames).cloned();
                let frame = frames
                    .last_mut()
                    .expect("the outermost frame is never closed");
                if let Some(name) = bound {
                    frame.bound.push(name);
                }
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

/// A `::`-joined path a source names, together with what the code around it makes of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Path {
    segments: Vec<String>,
    tested: bool,
}

impl Path {
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// # Returns
    ///
    /// Whether the path is written where the tests alone are compiled, which is what separates a
    /// module the application reaches from one only its own tests name. A module the attribute
    /// stands above is one such place, and so is a single import the attribute stands above.
    #[must_use]
    pub fn tested(&self) -> bool {
        self.tested
    }
}

/// Reads a Rust source into the paths it names.
///
/// # Returns
///
/// Every `::`-joined path the source names, in the order it names them, with the branches a `use`
/// tree brackets written out one path each so that `use a::{b, c}` names `a::b` and `a::c` as
/// plainly as two statements would. The modules the source declares are left out, because a
/// declaration hands a module to the compiler without any code reaching for it.
#[must_use]
pub fn paths(source: &str) -> Vec<Path> {
    let characters: Vec<char> = source.chars().collect();
    let mut paths = Vec::new();
    let mut walk = Walk::default();
    let mut index = 0;

    while index < characters.len() {
        match characters[index] {
            '/' if Some(&'/') == characters.get(index + 1) => {
                index = line_comment(&characters, index);
                walk.flush(&mut paths);
            }
            '/' if Some(&'*') == characters.get(index + 1) => {
                index = block_comment(&characters, index);
                walk.flush(&mut paths);
            }
            '"' => {
                index = string(&characters, index);
                walk.flush(&mut paths);
            }
            '\'' => {
                index = character_literal(&characters, index);
                walk.flush(&mut paths);
            }
            '#' => {
                let (end, attribute) = attribute(&characters, index);
                index = end;
                walk.flush(&mut paths);
                walk.attribute(&attribute);
            }
            ':' if Some(&':') == characters.get(index + 1) => {
                walk.join();
                index += 2;
            }
            '{' => {
                walk.open(&mut paths);
                index += 1;
            }
            '}' => {
                walk.close(&mut paths);
                index += 1;
            }
            ';' => {
                walk.flush(&mut paths);
                walk.plain();
                index += 1;
            }
            character if starts_a_word(character) => {
                let end = word_end(&characters, index);
                let text: String = characters[index..end].iter().collect();
                if let Some(literal) = raw_string(&characters, &text, end) {
                    index = literal;
                    walk.flush(&mut paths);
                } else {
                    walk.segment(text, &mut paths);
                    index = end;
                }
            }
            character => {
                if !character.is_whitespace() {
                    walk.flush(&mut paths);
                }
                index += 1;
            }
        }
    }
    walk.flush(&mut paths);

    paths
}

/// What an open brace bracketed, which decides what closing it does.
#[derive(Debug)]
enum Brace {
    /// The branches of a `use` tree, which share the path written before the brace.
    Group,

    /// A block of code, which is a test's block where the attribute above it said so.
    Block { test: bool },
}

/// Where a walk over the paths of a source has reached.
#[derive(Debug, Default)]
struct Walk {
    groups: Vec<Vec<String>>,
    braces: Vec<Brace>,
    segments: Vec<String>,
    joined: bool,
    declaration: bool,
    declaring: bool,
    attributed: bool,
}

impl Walk {
    /// Reads a `::`, which joins whatever comes next to the path being walked.
    fn join(&mut self) {
        self.joined = true;
    }

    /// Reads an attribute, which says the item it is written above is compiled only for the tests
    /// where it is the one saying so. That item is a module as often as it is a single import, and
    /// an import the tests alone are given is no less test-only for having no module around it.
    fn attribute(&mut self, attribute: &str) {
        self.attributed |= "cfg(test)" == attribute;
    }

    /// Reads the end of an item, which is as far as an attribute above it reaches.
    fn plain(&mut self) {
        self.attributed = false;
    }

    /// Reads a word, which either continues the path being walked or starts one of its own.
    fn segment(&mut self, text: String, paths: &mut Vec<Path>) {
        if self.joined {
            self.joined = false;
            self.segments.push(text);
            return;
        }

        self.flush(paths);
        self.declaration = self.declaring;
        self.declaring = "mod" == text;
        self.segments.push(text);
    }

    /// Reads an open brace, which brackets the branches of a `use` tree where a `::` is waiting on
    /// it and a block of code otherwise.
    fn open(&mut self, paths: &mut Vec<Path>) {
        if self.joined {
            self.joined = false;
            let mut prefix = self.groups.concat();
            prefix.append(&mut self.segments);
            self.groups.push(prefix);
            self.braces.push(Brace::Group);
            self.declaration = false;
            return;
        }

        self.flush(paths);
        let test = self.attributed;
        self.plain();
        self.braces.push(Brace::Block { test });
    }

    /// Reads a closing brace, which ends the last branch of a `use` tree it closes one of.
    fn close(&mut self, paths: &mut Vec<Path>) {
        self.flush(paths);
        if let Some(Brace::Group) = self.braces.pop() {
            self.groups.pop();
        } else {
            self.plain();
        }
    }

    /// Writes down the path the walk has reached the end of, if it reached one that is named
    /// rather than declared.
    fn flush(&mut self, paths: &mut Vec<Path>) {
        let declaration = self.declaration;
        let branch = std::mem::take(&mut self.segments);
        self.declaration = false;
        self.joined = false;
        if declaration || branch.is_empty() {
            return;
        }

        let mut segments = self.groups.concat();
        segments.extend(branch);
        let tested = self.attributed
            || self
                .braces
                .iter()
                .any(|brace| matches!(brace, Brace::Block { test: true }));
        paths.push(Path { segments, tested });
    }
}

/// What a delimiter opened, which is what the words inside it are read against.
#[derive(Debug, Default)]
struct Frame {
    header: Vec<String>,
    bound: Vec<String>,
    repetition: bool,
    test: bool,
}

/// # Returns
///
/// Whether a delimiter opens a repetition over a whole text, which is either a loop walking one or
/// an adapter run over every element of one. A header naming a local the extent of a text was
/// bound to names the text, so a bound that was hoisted out of the loop is read as one written
/// into it.
fn opens_a_repetition(delimiter: char, frames: &[Frame]) -> bool {
    let Some(frame) = frames.last() else {
        return false;
    };
    let over_a_whole_text = frame
        .header
        .iter()
        .any(|word| names_a_whole_text(word, frames));
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
/// The name the statement a frame has reached the end of binds the whole of a text to, or [`None`]
/// where the statement binds no such name. `let end = source.len();` binds one, and a loop reading
/// `end` afterwards walks the text `source` holds however far from the loop the measurement was
/// written. A statement binding one such name to another binds one too, so passing the extent
/// along a chain of locals says no less about the loop that walks to the last of them.
fn hoisted(frames: &[Frame]) -> Option<&String> {
    let mut words = frames.last()?.header.iter();
    if Some("let") != words.next().map(String::as_str) {
        return None;
    }

    let mut name = words.next()?;
    if "mut" == name {
        name = words.next()?;
    }

    words
        .any(|word| names_a_whole_text(word, frames))
        .then_some(name)
}

/// # Returns
///
/// Whether a word names the whole of a text, which it does either by being one of the words that
/// name one or by being a local one was bound to in a block that is still open, so that a chain of
/// such locals names a text as plainly as the measurement at the head of it does.
fn names_a_whole_text(word: &str, frames: &[Frame]) -> bool {
    WHOLE_TEXT.contains(&word)
        || frames
            .iter()
            .any(|frame| frame.bound.iter().any(|bound| bound == word))
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
