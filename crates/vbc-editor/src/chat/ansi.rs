//! Reading the ANSI escapes tool output arrives with.
//!
//! `cargo`, `git` and `ls --color` all colour their output with escape sequences. A transcript
//! that passed them through would draw them as text and a transcript that stripped them would
//! throw the colour away, so they are read instead: the escapes are consumed and what they said
//! becomes the spans styling the text they surrounded. The source a block carries is therefore
//! the text a reader sees, which is what makes an exact yank of tool output yield the output
//! rather than the escapes that coloured it.
//!
//! Only a select-graphic-rendition sequence carries style. Every other escape -- a cursor move, an
//! erase, a window title -- is consumed and dropped, because a transcript is not a terminal and
//! has nothing for one to move about in. A sequence the output ends inside of is dropped whole.

use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};

use crate::style::{Block, Span};

/// The escape every sequence starts with.
const ESCAPE: char = '\u{1b}';

/// The introducer of a control sequence, which is the only kind of sequence that carries style.
const CONTROL: char = '[';

/// The final character of the control sequence selecting a graphic rendition.
const RENDITION: char = 'm';

/// The character separating one parameter of a control sequence from the next.
const PARAMETER: char = ';';

/// One of the two terminators a string sequence may end at.
const BELL: char = '\u{7}';

/// The character that, after an escape, terminates a string sequence.
const STRING_TERMINATOR: char = '\\';

/// The distance between the code naming a colour in the foreground and the one naming it in the
/// background.
const BACKGROUND_OFFSET: u32 = 10;

/// Reads the escapes of `raw` as the styles they name.
///
/// # Returns
///
/// A block of the text of `raw` with its escapes consumed, styled by the renditions they selected.
#[must_use]
pub fn parse(raw: &str) -> Block {
    let mut text = String::with_capacity(raw.len());
    let mut spans = Vec::new();
    let mut rendition = Rendition::default();
    let mut start = 0;
    let mut rest = raw;

    while let Some(offset) = rest.find(ESCAPE) {
        text.push_str(&rest[..offset]);
        let (sequence, remainder) = split(&rest[offset..]);
        rest = remainder;

        let Some(parameters) = rendition_parameters(sequence) else {
            continue;
        };
        let selected = rendition.applied(parameters);
        if selected == rendition {
            continue;
        }

        push(&mut spans, start..text.len(), rendition);
        start = text.len();
        rendition = selected;
    }
    text.push_str(rest);
    push(&mut spans, start..text.len(), rendition);

    Block::with_spans(text, spans)
}

/// The graphic rendition a run of text is drawn under, which is what a rendition sequence sets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Rendition {
    foreground: Option<Color>,
    background: Option<Color>,
    modifier: Modifier,
}

impl Rendition {
    /// # Returns
    ///
    /// This rendition with `parameters` applied, every parameter it does not know left out.
    fn applied(mut self, parameters: &str) -> Self {
        let mut codes = parameters.split(PARAMETER).map(|parameter| {
            if parameter.is_empty() {
                Some(0)
            } else {
                parameter.parse::<u32>().ok()
            }
        });

        while let Some(code) = codes.next() {
            let Some(code) = code else {
                continue;
            };

            match code {
                0 => self = Self::default(),
                1 => self.modifier.insert(Modifier::BOLD),
                2 => self.modifier.insert(Modifier::DIM),
                3 => self.modifier.insert(Modifier::ITALIC),
                4 => self.modifier.insert(Modifier::UNDERLINED),
                5 => self.modifier.insert(Modifier::SLOW_BLINK),
                6 => self.modifier.insert(Modifier::RAPID_BLINK),
                7 => self.modifier.insert(Modifier::REVERSED),
                8 => self.modifier.insert(Modifier::HIDDEN),
                9 => self.modifier.insert(Modifier::CROSSED_OUT),
                22 => self.modifier.remove(Modifier::BOLD | Modifier::DIM),
                23 => self.modifier.remove(Modifier::ITALIC),
                24 => self.modifier.remove(Modifier::UNDERLINED),
                25 => self
                    .modifier
                    .remove(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
                27 => self.modifier.remove(Modifier::REVERSED),
                28 => self.modifier.remove(Modifier::HIDDEN),
                29 => self.modifier.remove(Modifier::CROSSED_OUT),
                30..=37 | 90..=97 => self.foreground = Some(basic(code)),
                38 => self.foreground = extended(&mut codes).or(self.foreground),
                39 => self.foreground = None,
                40..=47 | 100..=107 => self.background = Some(basic(code - BACKGROUND_OFFSET)),
                48 => self.background = extended(&mut codes).or(self.background),
                49 => self.background = None,
                _ => {}
            }
        }

        self
    }

    /// # Returns
    ///
    /// The style a run of text under this rendition is drawn in.
    fn style(self) -> Style {
        let style = Style::new().add_modifier(self.modifier);
        let style = self.foreground.map_or(style, |colour| style.fg(colour));

        self.background.map_or(style, |colour| style.bg(colour))
    }
}

/// Records `range` as styled by `rendition`, dropping a run that is empty or carries no style of
/// its own.
fn push(spans: &mut Vec<Span>, range: Range<usize>, rendition: Rendition) {
    if range.is_empty() || Rendition::default() == rendition {
        return;
    }

    spans.push(Span::new(range, rendition.style()));
}

/// Splits the escape sequence `raw` starts with from the text after it.
///
/// # Returns
///
/// The sequence and the text following it, which is empty for a sequence `raw` ends inside of.
fn split(raw: &str) -> (&str, &str) {
    let mut characters = raw.char_indices();
    characters.next();

    let end = match characters.next() {
        None => None,
        Some((_, CONTROL)) => characters
            .find(|&(_, character)| !is_parameter(character) && !is_intermediate(character))
            .map(|(index, character)| index + character.len_utf8()),
        Some((_, introducer)) if is_string(introducer) => {
            let mut previous = introducer;
            characters.find_map(|(index, character)| {
                let terminated =
                    BELL == character || (STRING_TERMINATOR == character && ESCAPE == previous);
                previous = character;
                terminated.then(|| index + character.len_utf8())
            })
        }
        Some((_, introducer)) if is_intermediate(introducer) => characters
            .find(|&(_, character)| !is_intermediate(character))
            .map(|(index, character)| index + character.len_utf8()),
        Some((index, introducer)) => Some(index + introducer.len_utf8()),
    };
    let end = end.unwrap_or(raw.len());

    (&raw[..end], &raw[end..])
}

/// # Returns
///
/// The parameters of `sequence` if it selects a graphic rendition, or `None` if it is any other
/// escape.
fn rendition_parameters(sequence: &str) -> Option<&str> {
    sequence
        .strip_prefix(ESCAPE)?
        .strip_prefix(CONTROL)?
        .strip_suffix(RENDITION)
}

/// # Returns
///
/// Whether `character` is one a control sequence carries its parameters in.
fn is_parameter(character: char) -> bool {
    matches!(character, '\u{30}'..='\u{3f}')
}

/// # Returns
///
/// Whether `character` is one a control sequence carries between its parameters and its end.
fn is_intermediate(character: char) -> bool {
    matches!(character, '\u{20}'..='\u{2f}')
}

/// # Returns
///
/// Whether `character` introduces a sequence carrying a string, which runs to a terminator of its
/// own rather than to a final character.
fn is_string(character: char) -> bool {
    matches!(character, ']' | 'P' | 'X' | '^' | '_')
}

/// # Returns
///
/// The colour the foreground rendition code `code` names.
///
/// # Panics
///
/// Panics if `code` names no basic colour.
fn basic(code: u32) -> Color {
    match code {
        30 => Color::Black,
        31 => Color::Red,
        32 => Color::Green,
        33 => Color::Yellow,
        34 => Color::Blue,
        35 => Color::Magenta,
        36 => Color::Cyan,
        37 => Color::Gray,
        90 => Color::DarkGray,
        91 => Color::LightRed,
        92 => Color::LightGreen,
        93 => Color::LightYellow,
        94 => Color::LightBlue,
        95 => Color::LightMagenta,
        96 => Color::LightCyan,
        97 => Color::White,
        _ => panic!("a basic colour is named by one of the sixteen foreground codes"),
    }
}

/// Reads the colour an extended rendition code selects from the parameters after it.
///
/// # Returns
///
/// The colour selected, or `None` if the parameters name none.
fn extended(codes: &mut impl Iterator<Item = Option<u32>>) -> Option<Color> {
    match codes.next()? {
        Some(5) => Some(Color::Indexed(channel(codes)?)),
        Some(2) => {
            let red = channel(codes)?;
            let green = channel(codes)?;
            let blue = channel(codes)?;

            Some(Color::Rgb(red, green, blue))
        }
        _ => None,
    }
}

/// # Returns
///
/// The next parameter of `codes` as a colour channel, or `None` if there is none or it names no
/// channel.
fn channel(codes: &mut impl Iterator<Item = Option<u32>>) -> Option<u8> {
    codes
        .next()
        .flatten()
        .and_then(|code| u8::try_from(code).ok())
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier, Style};

    use crate::style::Span;

    use super::parse;

    #[test]
    fn a_rendition_styles_the_text_it_wrapped() {
        let block = parse("plain \u{1b}[31mred\u{1b}[0m plain");

        assert_eq!("plain red plain", block.source());
        assert_eq!(&[Span::new(6..9, red())], block.spans());
    }

    #[test]
    fn escapes_are_absent_from_the_source() {
        let block = parse("\u{1b}[1;32mok\u{1b}[m\n\u{1b}[31mbad\u{1b}[0m");

        assert_eq!("ok\nbad", block.source());
        assert!(
            !block.source().contains('\u{1b}'),
            "the source kept an escape: {:?}",
            block.source()
        );
    }

    #[test]
    fn a_rendition_left_open_styles_the_rest_of_the_output() {
        let block = parse("cold \u{1b}[34mblue to the end");

        assert_eq!("cold blue to the end", block.source());
        assert_eq!(
            &[Span::new(5..20, Style::new().fg(Color::Blue))],
            block.spans()
        );
    }

    #[test]
    fn renditions_accumulate_until_they_are_turned_off() {
        let block = parse("\u{1b}[1m\u{1b}[31mboth\u{1b}[22mred\u{1b}[39mplain");

        assert_eq!("bothredplain", block.source());
        assert_eq!(
            &[
                Span::new(0..4, red().add_modifier(Modifier::BOLD)),
                Span::new(4..7, red()),
            ],
            block.spans()
        );
    }

    #[test]
    fn an_extended_rendition_names_an_indexed_or_true_colour() {
        let block = parse("\u{1b}[38;5;208mo\u{1b}[48;2;1;2;3mb");

        assert_eq!("ob", block.source());
        assert_eq!(
            &[
                Span::new(0..1, Style::new().fg(Color::Indexed(208))),
                Span::new(
                    1..2,
                    Style::new().fg(Color::Indexed(208)).bg(Color::Rgb(1, 2, 3))
                ),
            ],
            block.spans()
        );
    }

    #[test]
    fn an_escape_that_selects_no_rendition_is_dropped_without_styling_anything() {
        for raw in [
            "a\u{1b}[2Jb",
            "a\u{1b}[Kb",
            "a\u{1b}[?25lb",
            "a\u{1b}]0;a title\u{7}b",
            "a\u{1b}]8;;https://example.com\u{1b}\\b",
            "a\u{1b}(Bb",
            "a\u{1b}7b",
        ] {
            let block = parse(raw);
            assert_eq!(
                "ab",
                block.source(),
                "the escape of {raw:?} was not dropped"
            );
            assert_eq!(&[] as &[Span], block.spans());
        }
    }

    #[test]
    fn a_sequence_the_output_ends_inside_of_is_dropped_whole() {
        for raw in [
            "done\u{1b}",
            "done\u{1b}[",
            "done\u{1b}[31",
            "done\u{1b}]0;t",
        ] {
            let block = parse(raw);
            assert_eq!(
                "done",
                block.source(),
                "the tail of {raw:?} was not dropped"
            );
            assert_eq!(&[] as &[Span], block.spans());
        }
    }

    #[test]
    fn a_rendition_selecting_what_is_already_selected_leaves_one_span() {
        let block = parse("\u{1b}[31mred\u{1b}[31mstill red");

        assert_eq!("redstill red", block.source());
        assert_eq!(&[Span::new(0..12, red())], block.spans());
    }

    #[test]
    fn a_rendition_around_no_text_styles_nothing() {
        let block = parse("\u{1b}[31m\u{1b}[0mplain");

        assert_eq!("plain", block.source());
        assert_eq!(&[] as &[Span], block.spans());
    }

    #[test]
    fn output_carrying_no_escape_is_left_as_it_was() {
        let block = parse("nothing to read here");

        assert_eq!("nothing to read here", block.source());
        assert_eq!(&[] as &[Span], block.spans());
    }

    /// # Returns
    ///
    /// The style the fixtures name most often.
    fn red() -> Style {
        Style::new().fg(Color::Red)
    }
}
