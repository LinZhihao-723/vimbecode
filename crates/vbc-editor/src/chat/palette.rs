//! The colours a transcript is drawn in, at the depth the terminal it is drawn to can show.
//!
//! A colour is named as the red, green and blue it is meant to be, and what reaches the terminal
//! is that colour at the terminal's depth: the colour itself where the terminal says it draws 24
//! bits of colour, and otherwise the nearest of the 240 colours of the xterm 256-colour palette
//! whose values do not depend on the terminal's own theme. A terminal says so through
//! `COLORTERM`, and one that says nothing is drawn to in 256 colours, because tmux under
//! `screen-256color` says nothing and does not draw 24-bit colour faithfully.
//!
//! The named colours are base16's Ocean scheme, drawn for a dark background, so that everything
//! drawn in colour -- the code in a reply and the diff of an edit alike -- is drawn from the same
//! few.

use std::ffi::OsStr;

use ratatui::style::Color;

/// The environment variable a terminal says how many colours it draws through.
pub const COLORTERM: &str = "COLORTERM";

/// The values of [`COLORTERM`] that say a terminal draws 24 bits of colour.
pub const TRUECOLOR: [&str; 2] = ["truecolor", "24bit"];

/// base16 Ocean's foreground, which is the colour of text nothing else colours.
pub const FOREGROUND: Rgb = Rgb::new(0xc0, 0xc5, 0xce);

/// base16 Ocean's comment colour.
pub const COMMENT: Rgb = Rgb::new(0x65, 0x73, 0x7e);

/// base16 Ocean's red accent.
pub const RED: Rgb = Rgb::new(0xbf, 0x61, 0x6a);

/// base16 Ocean's orange accent.
pub const ORANGE: Rgb = Rgb::new(0xd0, 0x87, 0x70);

/// base16 Ocean's yellow accent.
pub const YELLOW: Rgb = Rgb::new(0xeb, 0xcb, 0x8b);

/// base16 Ocean's green accent.
pub const GREEN: Rgb = Rgb::new(0xa3, 0xbe, 0x8c);

/// base16 Ocean's cyan accent.
pub const CYAN: Rgb = Rgb::new(0x96, 0xb5, 0xb4);

/// base16 Ocean's blue accent.
pub const BLUE: Rgb = Rgb::new(0x8f, 0xa1, 0xb3);

/// base16 Ocean's magenta accent.
pub const MAGENTA: Rgb = Rgb::new(0xb4, 0x8e, 0xad);

/// base16 Ocean's brown accent.
pub const BROWN: Rgb = Rgb::new(0xab, 0x79, 0x67);

/// A colour, named as the red, green and blue it is meant to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The colour of `red`, `green` and `blue`.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    #[must_use]
    pub fn red(&self) -> u8 {
        self.red
    }

    #[must_use]
    pub fn green(&self) -> u8 {
        self.green
    }

    #[must_use]
    pub fn blue(&self) -> u8 {
        self.blue
    }

    /// # Returns
    ///
    /// The index of the colour of the xterm 256-colour palette nearest this one, which is a
    /// colour of its cube or one of its greys and never one of the sixteen a terminal's theme
    /// redefines.
    #[must_use]
    pub fn indexed(&self) -> u8 {
        let levels = [self.red, self.green, self.blue].map(nearest_level);
        let [red, green, blue] = levels.map(|level| CUBE[usize::from(level)]);
        let cubed = Self::new(red, green, blue);

        let average = (u16::from(self.red) + u16::from(self.green) + u16::from(self.blue)) / 3;
        let step = u16::from(GREY_STEP);
        let grey = (average.saturating_sub(u16::from(GREY_FIRST)) + step / 2) / step;
        let grey = u8::try_from(grey.min(u16::from(GREYS - 1)))
            .expect("the index of a grey fits in a byte");
        let level = GREY_FIRST + GREY_STEP * grey;
        let greyed = Self::new(level, level, level);

        if self.distance(&greyed) < self.distance(&cubed) {
            return GREY_START + grey;
        }

        let [red, green, blue] = levels;
        CUBE_START + CUBE_SIDE * CUBE_SIDE * red + CUBE_SIDE * green + blue
    }

    /// # Returns
    ///
    /// The square of the distance between this colour and `other`, taken over their channels.
    fn distance(&self, other: &Self) -> u32 {
        [
            (self.red, other.red),
            (self.green, other.green),
            (self.blue, other.blue),
        ]
        .into_iter()
        .map(|(one, another)| u32::from(one.abs_diff(another)).pow(2))
        .sum()
    }
}

/// How many colours the terminal a transcript is drawn to shows, and so what a colour is drawn as
/// there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Palette {
    /// The xterm 256-colour palette, which is what a terminal that has not said otherwise is drawn
    /// in.
    Indexed,

    /// Any colour of 24 bits.
    Truecolor,
}

impl Palette {
    /// # Returns
    ///
    /// The palette the terminal the program runs in says it draws, through [`COLORTERM`].
    #[must_use]
    pub fn detected() -> Self {
        Self::of(std::env::var_os(COLORTERM).as_deref())
    }

    /// # Returns
    ///
    /// The palette of a terminal that set [`COLORTERM`] to `colorterm`, or that left it unset where
    /// `colorterm` is `None`: [`Palette::Truecolor`] where it named one of [`TRUECOLOR`], and
    /// [`Palette::Indexed`] otherwise.
    #[must_use]
    pub fn of(colorterm: Option<&OsStr>) -> Self {
        let told = colorterm.and_then(OsStr::to_str).is_some_and(|told| {
            TRUECOLOR
                .iter()
                .any(|truecolor| truecolor.eq_ignore_ascii_case(told))
        });

        if told {
            Self::Truecolor
        } else {
            Self::Indexed
        }
    }

    /// # Returns
    ///
    /// `rgb` as this palette draws it: itself in [`Palette::Truecolor`], and the nearest colour of
    /// the 256-colour palette in [`Palette::Indexed`].
    #[must_use]
    pub fn color(&self, rgb: Rgb) -> Color {
        match self {
            Self::Indexed => Color::Indexed(rgb.indexed()),
            Self::Truecolor => Color::Rgb(rgb.red, rgb.green, rgb.blue),
        }
    }
}

/// The levels each channel of the 256-colour palette's cube takes, how many there are, and the
/// index of the cube's first colour.
const CUBE: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
const CUBE_SIDE: u8 = 6;
const CUBE_START: u8 = 16;

/// The index of the 256-colour palette's first grey, the level of that grey, how far each grey
/// lies above the one before it, and how many greys there are.
const GREY_START: u8 = 232;
const GREY_FIRST: u8 = 8;
const GREY_STEP: u8 = 10;
const GREYS: u8 = 24;

/// # Returns
///
/// The index into [`CUBE`] of the level nearest `channel`.
fn nearest_level(channel: u8) -> u8 {
    let mut nearest = 0;
    for (index, level) in (0..CUBE_SIDE).zip(CUBE) {
        if channel.abs_diff(level) < channel.abs_diff(CUBE[usize::from(nearest)]) {
            nearest = index;
        }
    }

    nearest
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use ratatui::style::Color;

    use super::{
        Palette, Rgb, BLUE, BROWN, COMMENT, CUBE, CUBE_SIDE, CUBE_START, CYAN, GREEN, GREY_FIRST,
        GREY_START, GREY_STEP, MAGENTA, ORANGE, RED, YELLOW,
    };

    #[test]
    fn a_terminal_that_says_it_draws_24_bit_colour_is_drawn_in_it_and_any_other_in_256() {
        for (colorterm, palette) in [
            (None, Palette::Indexed),
            (Some(""), Palette::Indexed),
            (Some("256color"), Palette::Indexed),
            (Some("truecolor"), Palette::Truecolor),
            (Some("24bit"), Palette::Truecolor),
            (Some("TrueColor"), Palette::Truecolor),
        ] {
            assert_eq!(
                palette,
                Palette::of(colorterm.map(OsStr::new)),
                "COLORTERM={colorterm:?} chose the wrong palette"
            );
        }
    }

    #[test]
    fn every_colour_of_the_cube_and_the_greys_is_drawn_as_its_own_index() {
        for index in CUBE_START..=u8::MAX {
            let rgb = if index < GREY_START {
                let cubed = index - CUBE_START;
                Rgb::new(
                    CUBE[usize::from(cubed / (CUBE_SIDE * CUBE_SIDE))],
                    CUBE[usize::from(cubed / CUBE_SIDE % CUBE_SIDE)],
                    CUBE[usize::from(cubed % CUBE_SIDE)],
                )
            } else {
                let level = GREY_FIRST + GREY_STEP * (index - GREY_START);
                Rgb::new(level, level, level)
            };

            assert_eq!(
                index,
                rgb.indexed(),
                "{rgb:?} was not drawn as its own index"
            );
        }
    }

    #[test]
    fn no_colour_is_drawn_as_one_of_the_sixteen_a_terminal_theme_redefines() {
        for red in (0..=u8::MAX).step_by(15) {
            for green in (0..=u8::MAX).step_by(15) {
                for blue in (0..=u8::MAX).step_by(15) {
                    let rgb = Rgb::new(red, green, blue);
                    assert!(
                        CUBE_START <= rgb.indexed(),
                        "{rgb:?} was drawn as index {}",
                        rgb.indexed()
                    );
                }
            }
        }
    }

    #[test]
    fn a_colour_between_two_is_drawn_as_the_nearer() {
        assert_eq!(
            CUBE_START + CUBE_SIDE * CUBE_SIDE * 5,
            Rgb::new(0xf0, 0x10, 0x10).indexed()
        );
        assert_eq!(GREY_START + 12, Rgb::new(0x80, 0x80, 0x80).indexed());
        assert_eq!(CUBE_START, Rgb::new(0x02, 0x03, 0x04).indexed());
        assert_eq!(GREY_START, Rgb::new(0x0a, 0x0b, 0x0c).indexed());
    }

    #[test]
    fn a_colour_is_itself_in_24_bits_and_its_nearest_index_in_256_colours() {
        for rgb in [
            COMMENT, RED, ORANGE, YELLOW, GREEN, CYAN, BLUE, MAGENTA, BROWN,
        ] {
            assert_eq!(
                Color::Rgb(rgb.red(), rgb.green(), rgb.blue()),
                Palette::Truecolor.color(rgb)
            );
            assert_eq!(Color::Indexed(rgb.indexed()), Palette::Indexed.color(rgb));
        }
    }
}
