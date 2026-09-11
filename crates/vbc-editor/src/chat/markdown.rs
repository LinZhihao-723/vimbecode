//! The markdown a reply is written in, read as the styles it asks for.
//!
//! What is read here is the spans that draw a reply as it was meant: emphasis in the weight or the
//! slant it names, inline code in a colour of its own, a heading bold, and a list item's bullet
//! and a quote's mark in the colour of their own.
//!
//! The marks themselves stay where they were written, dimmed. A block's source is what was said
//! and a span only says how part of it is drawn, so a yank of a reply hands back the markdown it
//! was written in, which is design §11's rule that what is yanked is the source and never the
//! rendering.
//!
//! The reading is deliberately small: no emphasis nested inside another, no links, no tables. A
//! mark it cannot pair is left as the character it is.

use std::ops::Range;

use ratatui::style::{Modifier, Style};

use crate::chat::palette::{Palette, Rgb, BLUE, CYAN, ORANGE};
use crate::style::Span;

/// How a mark is drawn: dimmed, so that what it marks is what the eye lands on.
pub const MARK: Style = Style::new().add_modifier(Modifier::DIM);

/// How what `**` and `*` mark is drawn.
pub const STRONG: Style = Style::new().add_modifier(Modifier::BOLD);
pub const EMPHASIS: Style = Style::new().add_modifier(Modifier::ITALIC);

/// How a quoted line is drawn.
pub const QUOTED: Style = Style::new().add_modifier(Modifier::ITALIC);

/// The character a heading is written with, and the most of them a heading is written under.
const HEADING_MARK: u8 = b'#';
const HEADING_DEPTH: usize = 6;

/// The characters a list item is bulleted with, and the ones a numbered item's number ends in.
const BULLETS: [u8; 3] = [b'-', b'*', b'+'];
const NUMBERED: [u8; 2] = [b'.', b')'];

/// The character a quoted line is marked with.
const QUOTE_MARK: u8 = b'>';

/// The character inline code is fenced by, and the ones emphasis is written with.
const TICK: u8 = b'`';
const EMPHASES: [u8; 2] = [b'*', b'_'];

/// The character that takes the one after it as the character it is.
const ESCAPE: u8 = b'\\';

/// The deepest a line's marks may be indented and still be read as a list item's or a quote's.
const INDENT: usize = 3;

/// The colours inline code, a heading and a bullet are drawn in.
const CODE_COLOUR: Rgb = ORANGE;
const HEADING_COLOUR: Rgb = BLUE;
const BULLET_COLOUR: Rgb = CYAN;

/// # Returns
///
/// The spans the markdown `source` is written in asks for, in `palette`, each naming a byte range
/// of `source`, in the order they are painted in.
#[must_use]
pub fn spans(source: &str, palette: Palette) -> Vec<Span> {
    let mut read = Read {
        palette,
        spans: Vec::new(),
    };
    let mut offset = 0;
    for written in source.split_inclusive('\n') {
        let line = written.trim_end_matches(['\n', '\r']);
        if !read.heading(line, offset) {
            let content = read.marks(line, offset);
            read.inline(&line[content..], offset + content);
        }
        offset += written.len();
    }

    read.spans
}

/// The spans read out of a source so far, and the palette they are painted in.
struct Read {
    palette: Palette,
    spans: Vec<Span>,
}

impl Read {
    /// Reads `line`, which starts at the byte `offset` of the source, as a heading where it is one.
    ///
    /// # Returns
    ///
    /// Whether the line is a heading.
    fn heading(&mut self, line: &str, offset: usize) -> bool {
        let bytes = line.as_bytes();
        let depth = bytes
            .iter()
            .take_while(|byte| HEADING_MARK == **byte)
            .count();
        if !(1..=HEADING_DEPTH).contains(&depth)
            || bytes.get(depth).is_some_and(|byte| b' ' != *byte)
        {
            return false;
        }

        let heading = self.colour(HEADING_COLOUR).add_modifier(Modifier::BOLD);
        self.paint(offset..offset + line.len(), heading);
        self.paint(offset..offset + depth, heading.add_modifier(Modifier::DIM));

        true
    }

    /// Reads the mark `line`, which starts at the byte `offset` of the source, opens with where it
    /// is a list item or a quote.
    ///
    /// # Returns
    ///
    /// The byte of `line` its content starts at, past the mark and the blank after it, which is
    /// the start of the line where it opens with no mark.
    fn marks(&mut self, line: &str, offset: usize) -> usize {
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|byte| b' ' == **byte).count();
        if INDENT < indent {
            return 0;
        }
        let spaced = |at: usize| bytes.get(at).is_some_and(|byte| b' ' == *byte);
        let bullet = self.colour(BULLET_COLOUR);

        if bytes.get(indent).is_some_and(|byte| BULLETS.contains(byte)) && spaced(indent + 1) {
            self.paint(offset + indent..offset + indent + 1, bullet);

            return indent + 2;
        }

        let digits = bytes[indent..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        let number = indent + digits;
        if 0 < digits
            && bytes
                .get(number)
                .is_some_and(|byte| NUMBERED.contains(byte))
            && spaced(number + 1)
        {
            self.paint(offset + indent..offset + number + 1, bullet);

            return number + 2;
        }

        if bytes.get(indent).is_some_and(|byte| QUOTE_MARK == *byte) {
            let content = if spaced(indent + 1) {
                indent + 2
            } else {
                indent + 1
            };
            self.paint(offset + content..offset + line.len(), QUOTED);
            self.paint(offset + indent..offset + indent + 1, MARK);

            return content;
        }

        0
    }

    /// Reads the inline code and emphasis of `text`, which starts at the byte `offset` of the
    /// source.
    fn inline(&mut self, text: &str, offset: usize) {
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            at = match bytes[at] {
                ESCAPE => at + 2,
                TICK => self.code(bytes, at, offset),
                byte if EMPHASES.contains(&byte) => self.emphasis(bytes, at, offset),
                _ => at + 1,
            };
        }
    }

    /// Reads the inline code a run of ticks at `at` opens, where a run as long closes it.
    ///
    /// # Returns
    ///
    /// The byte past what was read: past the closing run where there is one, and past the opening
    /// run where there is none.
    fn code(&mut self, bytes: &[u8], at: usize, offset: usize) -> usize {
        let run = run_of(bytes, at);
        let mut from = at + run;
        while from < bytes.len() {
            if TICK != bytes[from] {
                from += 1;
                continue;
            }
            let closing = run_of(bytes, from);
            if closing == run {
                let code = self.colour(CODE_COLOUR);
                self.paint(offset + at..offset + at + run, MARK);
                self.paint(offset + at + run..offset + from, code);
                self.paint(offset + from..offset + from + run, MARK);

                return from + run;
            }
            from += closing;
        }

        at + run
    }

    /// Reads the emphasis a mark at `at` opens: strong where the mark is doubled, and emphasis
    /// where it is not, where a mark as long closes it.
    ///
    /// A mark opens only where what follows it is not blank and closes only where what precedes it
    /// is not, so `2 * 3 * 4` is arithmetic. An underscore opens and closes only at the edge of a
    /// word, so `snake_case_name` is a name.
    ///
    /// # Returns
    ///
    /// The byte past what was read.
    fn emphasis(&mut self, bytes: &[u8], at: usize, offset: usize) -> usize {
        let mark = bytes[at];
        let width = if bytes.get(at + 1).is_some_and(|byte| mark == *byte) {
            2
        } else {
            1
        };
        let opened = at + width;
        let word = |byte: Option<&u8>| byte.is_some_and(u8::is_ascii_alphanumeric);
        let blank = |byte: Option<&u8>| byte.is_none_or(u8::is_ascii_whitespace);
        if blank(bytes.get(opened))
            || (b'_' == mark && word(at.checked_sub(1).and_then(|before| bytes.get(before))))
        {
            return opened;
        }

        let mut from = opened + 1;
        while from + width <= bytes.len() {
            let closes = bytes[from..from + width].iter().all(|byte| mark == *byte)
                && bytes.get(from + width).is_none_or(|byte| mark != *byte)
                && !blank(bytes.get(from - 1))
                && !(b'_' == mark && word(bytes.get(from + width)));
            if closes {
                let style = if 2 == width { STRONG } else { EMPHASIS };
                self.paint(offset + at..offset + opened, MARK);
                self.paint(offset + opened..offset + from, style);
                self.paint(offset + from..offset + from + width, MARK);

                return from + width;
            }
            from += 1;
        }

        opened
    }

    /// Paints `range` of the source in `style`.
    fn paint(&mut self, range: Range<usize>, style: Style) {
        self.spans.push(Span::new(range, style));
    }

    /// # Returns
    ///
    /// A style drawing its text in `rgb`, at the depth of the palette the spans are painted in.
    fn colour(&self, rgb: Rgb) -> Style {
        Style::new().fg(self.palette.color(rgb))
    }
}

/// # Returns
///
/// How many ticks the run starting at `at` of `bytes` holds.
fn run_of(bytes: &[u8], at: usize) -> usize {
    bytes[at..].iter().take_while(|byte| TICK == **byte).count()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use crate::chat::palette::{Palette, Rgb};
    use crate::style::{Span, Style};

    use super::{
        spans, BULLET_COLOUR, CODE_COLOUR, EMPHASIS, HEADING_COLOUR, MARK, QUOTED, STRONG,
    };

    #[test]
    fn strong_and_emphasis_are_drawn_in_their_weight_and_their_marks_are_dimmed() {
        let source = "a **bold** and an *em* word";

        assert_eq!(
            vec![
                ("**", MARK),
                ("bold", STRONG),
                ("**", MARK),
                ("*", MARK),
                ("em", EMPHASIS),
                ("*", MARK),
            ],
            painted(source)
        );
    }

    #[test]
    fn inline_code_is_drawn_in_its_colour_and_holds_no_emphasis() {
        let source = "run `cargo *test*` now";

        assert_eq!(
            vec![
                ("`", MARK),
                ("cargo *test*", coloured(CODE_COLOUR)),
                ("`", MARK)
            ],
            painted(source)
        );
    }

    #[test]
    fn a_heading_is_bold_with_its_marks_dimmed() {
        let heading = coloured(HEADING_COLOUR).add_modifier(Modifier::BOLD);

        assert_eq!(
            vec![
                ("## Plan", heading),
                ("##", heading.add_modifier(Modifier::DIM))
            ],
            painted("## Plan")
        );
        assert_eq!(Vec::<(&str, Style)>::new(), painted("#hashtag"));
    }

    #[test]
    fn a_bullet_a_number_and_a_quote_mark_are_drawn_as_marks() {
        let source = "- one\n2. two\n> said";
        let bullet = coloured(BULLET_COLOUR);

        assert_eq!(
            vec![("-", bullet), ("2.", bullet), ("said", QUOTED), (">", MARK)],
            painted(source)
        );
    }

    #[test]
    fn arithmetic_names_and_unpaired_marks_are_left_as_they_are() {
        for source in [
            "2 * 3 * 4",
            "snake_case_name",
            "a ** b",
            "one `tick",
            "*open",
        ] {
            assert_eq!(
                Vec::<(&str, Style)>::new(),
                painted(source),
                "{source:?} was read as markdown"
            );
        }
    }

    #[test]
    fn a_span_names_the_bytes_of_the_line_it_was_read_on() {
        let source = "first line\nthen **this**";
        let read = spans(source, Palette::Indexed);

        assert_eq!(
            Some(&Span::new(18..22, STRONG)),
            read.iter().find(|span| STRONG == span.style())
        );
    }

    /// # Returns
    ///
    /// What each span read out of `source` covers, and the style it paints it in.
    fn painted(source: &str) -> Vec<(&str, Style)> {
        spans(source, Palette::Indexed)
            .iter()
            .map(|span| (&source[span.range().clone()], span.style()))
            .collect()
    }

    /// # Returns
    ///
    /// A style drawing its text in `rgb` in 256 colours.
    fn coloured(rgb: Rgb) -> Style {
        Style::new().fg(Palette::Indexed.color(rgb))
    }
}
