//! Colouring the code in a reply in the language it was written in.
//!
//! A code block is coloured by spans like any other block, so the colour is chrome: it says how
//! the source is drawn and never what the source is, and a yank of a highlighted block is the code
//! that was fenced, byte for byte.
//!
//! A language is found by the tag its fence named -- a name such as `rust` or an extension such as
//! `rs` -- or by the extension of a file, where what is highlighted knows the file it holds. The
//! grammars are Sublime Text's, as `syntect` bundles them, and a tag naming no language they know
//! leaves the block plain. The colours are the [`palette`]'s, laid over the grammars' scopes the
//! way base16's own scheme lays them, with a macro coloured as a call is and the quotes around a
//! string as the string; what the scheme leaves in its foreground is left in the terminal's own.
//!
//! Highlighting costs what the source holds, so a block is highlighted once, when it is made, and
//! never when it is drawn: a block is what was said and does not change afterwards, so the spans
//! made then are the spans every panel built over it draws. A source longer than [`MAX_SOURCE`] is
//! left plain rather than costing an arrival that much. Measured in release, a mebibyte of Rust
//! takes 1.6 s to highlight and a mebibyte written on one line 0.63 s, so the bound holds a block
//! to a tenth of a second.
//!
//! [`palette`]: crate::chat::palette

use std::ffi::OsStr;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SchemeColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::chat::palette::{
    Palette, Rgb, BLUE, BROWN, COMMENT, CYAN, FOREGROUND, GREEN, MAGENTA, ORANGE, RED, YELLOW,
};
use crate::style::{Span, Style};

/// The longest source, in bytes, that is highlighted rather than left plain.
pub const MAX_SOURCE: usize = 64 * 1024;

/// A language whose grammar the highlighter holds.
#[derive(Clone, Copy, Debug)]
pub struct Language {
    syntax: &'static SyntaxReference,
}

impl Language {
    /// # Returns
    ///
    /// The language a fence's tag names, by name as `rust` or by extension as `rs`, and otherwise
    /// the language of the file the tag names, as `src/main.rs`; or `None` where it names no
    /// language the grammars know, or names plain text.
    #[must_use]
    pub fn of_tag(tag: &str) -> Option<Self> {
        let named = tag.split(ATTRIBUTES).next().unwrap_or_default().trim();
        if named.is_empty() {
            return None;
        }

        SYNTAXES
            .find_syntax_by_token(named)
            .map_or_else(|| Self::of_path(named), Self::of)
    }

    /// # Returns
    ///
    /// The language of the file at `path`, read off its extension or, where a grammar is known by
    /// the whole name of a file as a makefile's is, off its name; or `None` where neither names a
    /// language the grammars know, or names plain text.
    #[must_use]
    pub fn of_path(path: &str) -> Option<Self> {
        let path = Path::new(path);
        let named = |part: Option<&OsStr>| {
            part.and_then(OsStr::to_str)
                .and_then(|part| SYNTAXES.find_syntax_by_extension(part))
        };

        named(path.extension())
            .or_else(|| named(path.file_name()))
            .and_then(Self::of)
    }

    /// # Returns
    ///
    /// The name the grammar gives the language, as `Rust`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.syntax.name
    }

    /// Highlights `source` as written in this language, in the colours `palette` draws.
    ///
    /// # Returns
    ///
    /// The spans colouring `source`, ascending and disjoint, which cover none of it that the scheme
    /// leaves in its foreground; and none at all where `source` is longer than [`MAX_SOURCE`] or
    /// where the grammar fails partway through it, so that a block is coloured whole or not at all.
    #[must_use]
    pub fn spans(&self, source: &str, palette: Palette) -> Vec<Span> {
        if MAX_SOURCE < source.len() {
            return Vec::new();
        }
        HIGHLIGHTED.fetch_add(1, Ordering::Relaxed);

        let mut lines = HighlightLines::new(self.syntax, &THEME);
        let mut spans: Vec<Span> = Vec::new();
        let mut offset = 0;
        for line in LinesWithEndings::from(source) {
            let Ok(pieces) = lines.highlight_line(line, &SYNTAXES) else {
                return Vec::new();
            };
            for (drawn, piece) in pieces {
                let start = offset;
                offset += piece.len();
                let colour = drawn.foreground;
                let rgb = Rgb::new(colour.r, colour.g, colour.b);
                if FOREGROUND == rgb {
                    continue;
                }

                let style = Style::new().fg(palette.color(rgb));
                let joined = spans
                    .last()
                    .is_some_and(|last| last.range().end == start && last.style() == style);
                if joined {
                    let last = spans.pop().expect("a joined span follows the one it joins");
                    spans.push(Span::new(last.range().start..offset, style));
                } else {
                    spans.push(Span::new(start..offset, style));
                }
            }
        }

        spans
    }

    /// # Returns
    ///
    /// The language `syntax` is the grammar of, or `None` where `syntax` is plain text, which
    /// colours nothing.
    fn of(syntax: &'static SyntaxReference) -> Option<Self> {
        (!std::ptr::eq(syntax, SYNTAXES.find_syntax_plain_text())).then_some(Self { syntax })
    }
}

/// # Returns
///
/// The number of sources the program has highlighted, which is what reading a conversation costs
/// in highlighting: a source is counted each time it is highlighted, and one left plain is not
/// counted at all.
#[must_use]
pub fn highlighted() -> u64 {
    HIGHLIGHTED.load(Ordering::Relaxed)
}

/// The character a fence's language is followed by where attributes are written after it, as in
/// `rust,ignore`.
const ATTRIBUTES: char = ',';

/// The scopes the scheme colours, each with the colour it is drawn in, after base16's own TextMate
/// scheme. Where several match a scope, the one naming more of it wins, so `entity.name.function`
/// is drawn as a call and every other `entity.name` as a type.
const SCHEME: [(&str, Rgb); 31] = [
    ("comment", COMMENT),
    ("punctuation.definition.comment", COMMENT),
    ("string", GREEN),
    ("punctuation.definition.string", GREEN),
    ("constant.other.symbol", GREEN),
    ("entity.other.inherited-class", GREEN),
    ("markup.inserted", GREEN),
    ("markup.raw", GREEN),
    ("constant", ORANGE),
    ("variable.parameter", ORANGE),
    ("support.constant", ORANGE),
    ("entity.other.attribute-name", ORANGE),
    ("entity.name", YELLOW),
    ("support.type", YELLOW),
    ("support.class", YELLOW),
    ("constant.character.escape", CYAN),
    ("string.regexp", CYAN),
    ("support.function", CYAN),
    ("markup.quote", CYAN),
    ("entity.name.function", BLUE),
    ("variable.function", BLUE),
    ("support.macro", BLUE),
    ("markup.heading", BLUE),
    ("keyword", MAGENTA),
    ("storage", MAGENTA),
    ("markup.changed", MAGENTA),
    ("keyword.operator", FOREGROUND),
    ("entity.name.tag", RED),
    ("markup.deleted", RED),
    ("invalid", RED),
    ("invalid.deprecated", BROWN),
];

/// The grammars, read once for the life of the program.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// The scheme, built once for the life of the program.
static THEME: LazyLock<Theme> = LazyLock::new(scheme);

/// The number of sources the program has highlighted.
static HIGHLIGHTED: AtomicU64 = AtomicU64::new(0);

/// # Returns
///
/// The scheme code is highlighted in: the colours of [`SCHEME`] over the scopes it names, and
/// [`FOREGROUND`] over every scope it does not.
///
/// # Panics
///
/// Panics if a selector of [`SCHEME`] does not parse, which none of them fails to.
fn scheme() -> Theme {
    let item = |(selector, rgb): &(&str, Rgb)| ThemeItem {
        scope: ScopeSelectors::from_str(selector).expect("every selector of the scheme parses"),
        style: StyleModifier {
            foreground: Some(scheme_color(*rgb)),
            background: None,
            font_style: None,
        },
    };

    Theme {
        settings: ThemeSettings {
            foreground: Some(scheme_color(FOREGROUND)),
            ..ThemeSettings::default()
        },
        scopes: SCHEME.iter().map(item).collect(),
        ..Theme::default()
    }
}

/// # Returns
///
/// `rgb` as the scheme names a colour.
fn scheme_color(rgb: Rgb) -> SchemeColor {
    SchemeColor {
        r: rgb.red(),
        g: rgb.green(),
        b: rgb.blue(),
        a: u8::MAX,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use crate::chat::palette::{Palette, Rgb, BLUE, COMMENT, CYAN, GREEN, MAGENTA, ORANGE};
    use crate::style::Span;

    use super::{Language, MAX_SOURCE};

    /// A block of Rust holding one of each token the scheme tells apart.
    const RUST: &str = "fn main() {\n    println!(\"hi\\n\");\n    let n: u32 = 42; // done\n}";

    #[test]
    fn a_fence_names_its_language_by_name_by_extension_or_by_the_file_it_holds() {
        for (tag, name) in [
            ("rust", "Rust"),
            ("Rust", "Rust"),
            ("rs", "Rust"),
            ("rust,ignore", "Rust"),
            ("src/main.rs", "Rust"),
            ("py", "Python"),
            ("sh", "Bourne Again Shell (bash)"),
            ("Makefile", "Makefile"),
        ] {
            assert_eq!(
                Some(name),
                Language::of_tag(tag).as_ref().map(Language::name),
                "the tag {tag:?} named the wrong language"
            );
        }
    }

    #[test]
    fn a_tag_naming_no_language_or_plain_text_names_none() {
        for tag in ["", " ", ",rust", "txt", "text", "no-such-language"] {
            assert_eq!(
                None,
                Language::of_tag(tag).as_ref().map(Language::name),
                "the tag {tag:?} named a language"
            );
        }
    }

    #[test]
    fn a_path_names_the_language_of_the_file_it_holds() {
        assert_eq!(
            Some("Rust"),
            Language::of_path("crates/vbc-editor/src/main.rs")
                .as_ref()
                .map(Language::name)
        );
        assert_eq!(
            None,
            Language::of_path("notes.txt").as_ref().map(Language::name)
        );
        assert_eq!(
            None,
            Language::of_path("no-extension")
                .as_ref()
                .map(Language::name)
        );
    }

    #[test]
    fn each_token_is_drawn_in_its_own_colour_and_punctuation_in_none() {
        let spans = rust().spans(RUST, Palette::Truecolor);

        for (token, rgb) in [
            ("fn", MAGENTA),
            ("main", BLUE),
            ("println!", BLUE),
            ("\"hi", GREEN),
            ("\\n", CYAN),
            ("u32", MAGENTA),
            ("42", ORANGE),
            ("// done", COMMENT),
        ] {
            assert_eq!(
                vec![Some(Palette::Truecolor.color(rgb)); token.len()],
                colours(&spans, token),
                "{token:?} was drawn in the wrong colour"
            );
        }
        for token in ["(", "{", ";", "="] {
            assert_eq!(
                vec![None; token.len()],
                colours(&spans, token),
                "{token:?} was coloured"
            );
        }
    }

    #[test]
    fn spans_are_ascending_disjoint_and_inside_the_source() {
        let spans = rust().spans(RUST, Palette::Indexed);
        assert!(
            3 <= spans.len(),
            "the fixture was coloured in too few spans: {spans:?}"
        );

        let mut end = 0;
        for span in &spans {
            assert!(
                end <= span.range().start && span.range().start < span.range().end,
                "{span:?} overlaps the span before it or is empty"
            );
            end = span.range().end;
        }
        assert!(end <= RUST.len(), "a span reaches past the source");
    }

    #[test]
    fn a_source_is_coloured_alike_in_either_palette_at_that_palettes_depth() {
        let indexed = rust().spans(RUST, Palette::Indexed);
        let truecolor = rust().spans(RUST, Palette::Truecolor);

        assert_eq!(ranges(&truecolor), ranges(&indexed));
        for (indexed, truecolor) in indexed.iter().zip(&truecolor) {
            let Some(Color::Rgb(red, green, blue)) = truecolor.style().fg else {
                panic!("{truecolor:?} is not drawn in 24-bit colour");
            };
            assert_eq!(
                Some(Color::Indexed(Rgb::new(red, green, blue).indexed())),
                indexed.style().fg
            );
        }
    }

    #[test]
    fn a_source_at_the_bound_is_coloured_and_one_past_it_is_left_plain() {
        let longest = format!("// {}", "x".repeat(MAX_SOURCE - 3));
        assert_eq!(MAX_SOURCE, longest.len());
        assert!(
            !rust().spans(&longest, Palette::Indexed).is_empty(),
            "a source at the bound was left plain"
        );

        let longer = format!("{longest}x");
        assert_eq!(&[] as &[Span], rust().spans(&longer, Palette::Indexed));
    }

    /// # Returns
    ///
    /// The language every case highlights in.
    fn rust() -> Language {
        Language::of_tag("rust").expect("the grammars know Rust")
    }

    /// # Returns
    ///
    /// The foreground each byte of the first `token` in [`RUST`] is drawn in by `spans`, `None` for
    /// a byte no span colours.
    fn colours(spans: &[Span], token: &str) -> Vec<Option<Color>> {
        let start = RUST.find(token).expect("the fixture holds the token");

        (start..start + token.len())
            .map(|byte| {
                spans
                    .iter()
                    .find(|span| span.range().contains(&byte))
                    .and_then(|span| span.style().fg)
            })
            .collect()
    }

    /// # Returns
    ///
    /// The range of the source each of `spans` colours.
    fn ranges(spans: &[Span]) -> Vec<std::ops::Range<usize>> {
        spans.iter().map(|span| span.range().clone()).collect()
    }
}
