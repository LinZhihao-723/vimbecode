//! Searches for text, viewports, and anchors on which the anchor-relative mapping disagrees with
//! the screen the same text is drawn on.
//!
//! The oracle here lays the whole document out, which is exactly what the mapping refuses to do:
//! a test may spend the whole buffer to know the right answer, a renderer may not. Every property
//! is checked from several anchors, one of them generated, so a mapping that only ever walks
//! forwards from the start of the text fails these tests.
//!
//! The two directions are checked apart as well as together. A round trip that only ever composes
//! the two mappings passes whenever they are wrong in the same way, and a mapping checked only
//! against a single-line buffer never walks a line boundary at all.

use std::num::NonZeroUsize;

use proptest::prelude::*;
use vbc_layout::anchor::{
    char_idx_at_visual_offset, visual_offset_from_anchor, Error, VisualOffset, Wrapping,
};
use vbc_layout::invariants::LogicalPosition;
use vbc_layout::line::{self, DisplayRow, Options};
use vbc_layout::width::{graphemes, AmbiWidth, Metrics};

/// The graphemes lines are generated from, covering plain, wide, combining, joined, and tab text.
const ALPHABET: [&str; 8] = ["a", "b", " ", "\t", "é", "e\u{0301}", "漢", ZWJ_FAMILY];

/// The ZWJ sequence for the family of a man, a woman, a girl, and a boy: seven code points and one
/// glyph.
const ZWJ_FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";

/// The continuation markers a generated case is drawn with, the empty one for no marker.
const SHOW_BREAKS: [&str; 3] = ["", ">>", "…"];

/// The bounds a generated case is drawn from.
const MAX_LINES: usize = 5;
const MAX_LINE_LEN: usize = 12;
const MIN_WIDTH: usize = 3;
const MAX_WIDTH: usize = 14;

/// The number of tab stops a generated case is measured under, kept short so that a tab does not
/// swallow a whole narrow row.
const TAB_STOP: usize = 4;

/// The rows a mapping is allowed to walk, which every generated case fits inside.
const MAX_ROWS: usize = 4096;

/// One row of the screen a whole document is drawn on: the logical line it renders a slice of, and
/// the row itself.
#[derive(Clone, Debug)]
struct ScreenRow {
    line: usize,
    row: DisplayRow,
}

/// One generated case: the text, the way it is drawn, and an anchor drawn from it.
#[derive(Clone, Debug)]
struct MappingCase {
    lines: Vec<String>,
    wrapping: Wrapping,
    anchor: LogicalPosition,
}

impl MappingCase {
    /// # Returns
    ///
    /// The anchors this case's properties are checked from: the first position of the text, the
    /// last, and the generated one, so that a walk runs forwards, backwards, and both.
    fn anchors(&self) -> Vec<LogicalPosition> {
        let last = self.lines.len() - 1;
        vec![
            LogicalPosition {
                line: 0,
                grapheme: 0,
            },
            LogicalPosition {
                line: last,
                grapheme: graphemes(&self.lines[last]).count(),
            },
            self.anchor,
        ]
    }

    /// # Returns
    ///
    /// Every position of the text, the position past the end of each line included.
    fn positions(&self) -> Vec<LogicalPosition> {
        self.lines
            .iter()
            .enumerate()
            .flat_map(|(line, text)| {
                (0..=graphemes(text).count())
                    .map(move |grapheme| LogicalPosition { line, grapheme })
            })
            .collect()
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 192,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// The mapping onto the screen, checked on its own against the screen the text is drawn on.
    #[test]
    fn a_position_is_drawn_where_the_screen_draws_it(case in mapping_case()) {
        let screen = screen(&case);
        for anchor in case.anchors() {
            let origin = anchor_row(&screen, anchor);
            for position in case.positions() {
                let offset = visual_offset_from_anchor(
                    &case.lines,
                    anchor,
                    position,
                    &case.wrapping,
                    MAX_ROWS,
                )
                .expect("a position of the text is mapped");
                let (row, column) = drawn_at(&screen, position, &case.wrapping);

                prop_assert_eq!(
                    VisualOffset { rows: row as isize - origin as isize, column },
                    offset,
                    "{} from the anchor {}",
                    position,
                    anchor
                );
            }
        }
    }

    /// The mapping back off the screen, checked on its own against the screen the text is drawn on.
    #[test]
    fn a_cell_maps_back_to_the_grapheme_the_screen_draws_in_it(case in mapping_case()) {
        let screen = screen(&case);
        for anchor in case.anchors() {
            let origin = anchor_row(&screen, anchor);
            for (row, drawn) in screen.iter().enumerate() {
                for (index, &column) in row_columns(drawn).iter().enumerate() {
                    let offset = VisualOffset { rows: row as isize - origin as isize, column };
                    let landing = char_idx_at_visual_offset(
                        &case.lines,
                        anchor,
                        offset,
                        &case.wrapping,
                    )
                    .expect("a cell of the screen is mapped");

                    prop_assert_eq!(
                        LogicalPosition {
                            line: drawn.line,
                            grapheme: drawn.row.start() + index,
                        },
                        landing.position,
                        "{} from the anchor {}",
                        offset,
                        anchor
                    );
                    prop_assert_eq!(
                        offset,
                        landing.offset,
                        "{} from the anchor {}",
                        offset,
                        anchor
                    );
                }
            }
        }
    }

    /// The cells a wide grapheme covers besides the one it starts in.
    #[test]
    fn a_cell_inside_a_grapheme_maps_to_the_grapheme_covering_it(case in mapping_case()) {
        let screen = screen(&case);
        for anchor in case.anchors() {
            let origin = anchor_row(&screen, anchor);
            for (row, drawn) in screen.iter().enumerate() {
                let columns = drawn.row.columns();
                for (index, span) in columns.windows(2).enumerate() {
                    let [start, end] = span else {
                        panic!("a window of two holds two columns");
                    };
                    for column in (start + 1)..*end {
                        let offset = VisualOffset { rows: row as isize - origin as isize, column };
                        let landing = char_idx_at_visual_offset(
                            &case.lines,
                            anchor,
                            offset,
                            &case.wrapping,
                        )
                        .expect("a cell of the screen is mapped");

                        prop_assert_eq!(
                            LogicalPosition {
                                line: drawn.line,
                                grapheme: drawn.row.start() + index,
                            },
                            landing.position,
                            "{} from the anchor {}",
                            offset,
                            anchor
                        );
                        prop_assert_eq!(
                            VisualOffset { rows: offset.rows, column: *start },
                            landing.offset,
                            "{} from the anchor {}",
                            offset,
                            anchor
                        );
                    }
                }
            }
        }
    }

    /// The round trip that starts in the text.
    #[test]
    fn a_position_survives_the_trip_through_the_screen(case in mapping_case()) {
        let screen = screen(&case);
        for anchor in case.anchors() {
            for position in case.positions() {
                if past_a_full_row(&screen, position, &case.wrapping) {
                    continue;
                }

                let offset = visual_offset_from_anchor(
                    &case.lines,
                    anchor,
                    position,
                    &case.wrapping,
                    MAX_ROWS,
                )
                .expect("a position of the text is mapped");
                let landing = char_idx_at_visual_offset(
                    &case.lines,
                    anchor,
                    offset,
                    &case.wrapping,
                )
                .expect("an offset of a mapped position is mapped back");

                prop_assert_eq!(
                    position,
                    landing.position,
                    "{} was drawn at {} from the anchor {}",
                    position,
                    offset,
                    anchor
                );
                prop_assert_eq!(offset, landing.offset);
            }
        }
    }

    /// The round trip that starts on the screen.
    #[test]
    fn a_cell_survives_the_trip_through_the_text(case in mapping_case()) {
        let screen = screen(&case);
        for anchor in case.anchors() {
            let origin = anchor_row(&screen, anchor);
            for (row, drawn) in screen.iter().enumerate() {
                for &column in &row_columns(drawn) {
                    let offset = VisualOffset { rows: row as isize - origin as isize, column };
                    let landing = char_idx_at_visual_offset(
                        &case.lines,
                        anchor,
                        offset,
                        &case.wrapping,
                    )
                    .expect("a cell of the screen is mapped");
                    let mapped_back = visual_offset_from_anchor(
                        &case.lines,
                        anchor,
                        landing.position,
                        &case.wrapping,
                        MAX_ROWS,
                    )
                    .expect("the position drawn in a cell is mapped");

                    prop_assert_eq!(
                        offset,
                        mapped_back,
                        "{} from the anchor {} landed on {}",
                        offset,
                        anchor,
                        landing.position
                    );
                }
            }
        }
    }

    /// The cursor `A` leaves behind, which rests past the last grapheme of its line.
    #[test]
    fn the_cursor_past_the_end_of_a_line_is_drawn_inside_the_viewport(case in mapping_case()) {
        let screen = screen(&case);
        let width = case.wrapping.width().get();
        for anchor in case.anchors() {
            let origin = anchor_row(&screen, anchor);
            for (line, text) in case.lines.iter().enumerate() {
                let position = LogicalPosition {
                    line,
                    grapheme: graphemes(text).count(),
                };
                let offset = visual_offset_from_anchor(
                    &case.lines,
                    anchor,
                    position,
                    &case.wrapping,
                    MAX_ROWS,
                )
                .expect("the position past the end of a line is mapped");

                prop_assert!(
                    offset.column < width,
                    "{} is drawn at {} from the anchor {}, outside a viewport {} wide",
                    position,
                    offset,
                    anchor,
                    width
                );

                let row = origin as isize + offset.rows;
                prop_assert!(
                    0 <= row,
                    "{} is drawn {} rows above the screen",
                    position,
                    row
                );
                let last = last_row_of(&screen, line);
                let full = width <= screen[last].row.width();
                let expected = if full { last + 1 } else { last };
                prop_assert_eq!(
                    expected as isize,
                    row,
                    "{} is drawn on row {} rather than {}",
                    position,
                    row,
                    expected
                );
                prop_assert_eq!(if full { 0 } else { screen[last].row.width() }, offset.column);
            }
        }
    }
}

#[test]
fn a_position_further_than_the_rows_asked_for_is_refused() {
    let lines: Vec<String> = (0..64).map(|index| format!("line {index}")).collect();
    let wrapping = Wrapping::new(
        NonZeroUsize::new(20).expect("a test's width is not zero"),
        Metrics::default(),
        Options::new(),
    );
    let anchor = LogicalPosition {
        line: 0,
        grapheme: 0,
    };

    for line in 0..lines.len() {
        let position = LogicalPosition { line, grapheme: 0 };
        let mapped = visual_offset_from_anchor(&lines, anchor, position, &wrapping, 8)
            .map(|offset| offset.rows);
        let expected = if line <= 8 {
            Ok(line as isize)
        } else {
            Err(())
        };

        assert_eq!(expected, mapped.map_err(|_| ()), "line {line} from the top");
    }
}

#[test]
fn a_position_the_text_does_not_hold_is_refused() {
    let lines = vec!["abc".to_owned(), "de".to_owned()];
    let wrapping = Wrapping::new(
        NonZeroUsize::new(20).expect("a test's width is not zero"),
        Metrics::default(),
        Options::new(),
    );
    let anchor = LogicalPosition {
        line: 0,
        grapheme: 0,
    };
    let missing_line = LogicalPosition {
        line: 2,
        grapheme: 0,
    };
    let missing_grapheme = LogicalPosition {
        line: 1,
        grapheme: 3,
    };

    assert_eq!(
        Err(Error::LineOutOfBounds {
            line: 2,
            line_count: 2
        }),
        visual_offset_from_anchor(&lines, anchor, missing_line, &wrapping, MAX_ROWS)
    );
    assert_eq!(
        Err(Error::GraphemeOutOfBounds {
            position: missing_grapheme,
            line_len: 2
        }),
        visual_offset_from_anchor(&lines, anchor, missing_grapheme, &wrapping, MAX_ROWS)
    );
    assert_eq!(
        Err(Error::LineOutOfBounds {
            line: 2,
            line_count: 2
        }),
        char_idx_at_visual_offset(
            &lines,
            missing_line,
            VisualOffset { rows: 0, column: 0 },
            &wrapping
        )
    );
}

/// # Returns
///
/// A strategy generating the text, the way it is drawn, and the anchor a mapping is checked from.
fn mapping_case() -> impl Strategy<Value = MappingCase> {
    let line = proptest::collection::vec(
        proptest::sample::select(ALPHABET.as_slice()),
        0..=MAX_LINE_LEN,
    )
    .prop_map(|line_graphemes| line_graphemes.concat());
    (
        proptest::collection::vec(line, 1..=MAX_LINES),
        MIN_WIDTH..=MAX_WIDTH,
        any::<bool>(),
        proptest::sample::select(SHOW_BREAKS.as_slice()),
        any::<bool>(),
        any::<bool>(),
        0..MAX_LINES,
        0..=MAX_LINE_LEN,
    )
        .prop_map(
            |(
                lines,
                width,
                break_indent,
                show_break,
                line_break,
                ambiwidth,
                anchor_line,
                anchor_grapheme,
            )| {
                let options = Options::new()
                    .with_break_indent(break_indent)
                    .with_show_break(show_break.to_owned())
                    .with_line_break(line_break);
                let metrics = Metrics::new(
                    if ambiwidth {
                        AmbiWidth::Double
                    } else {
                        AmbiWidth::Single
                    },
                    NonZeroUsize::new(TAB_STOP).expect("a test's tab stop is not zero"),
                );
                let line = anchor_line % lines.len();
                let anchor = LogicalPosition {
                    line,
                    grapheme: anchor_grapheme.min(graphemes(&lines[line]).count()),
                };
                MappingCase {
                    lines,
                    wrapping: Wrapping::new(
                        NonZeroUsize::new(width).expect("a generated width is not zero"),
                        metrics,
                        options,
                    ),
                    anchor,
                }
            },
        )
}

/// Lays the whole document out, which is what the mapping under test never does.
///
/// # Returns
///
/// Every row of the screen the case's text is drawn on, top to bottom.
fn screen(case: &MappingCase) -> Vec<ScreenRow> {
    case.lines
        .iter()
        .enumerate()
        .flat_map(|(line, text)| {
            line::lay_out(
                line,
                text,
                case.wrapping.width(),
                case.wrapping.metrics(),
                case.wrapping.options(),
            )
            .into_iter()
            .map(move |row| ScreenRow { line, row })
        })
        .collect()
}

/// # Returns
///
/// The index of the screen row that draws `position`, which is its line's last row for the
/// position past that line's last grapheme.
///
/// # Panics
///
/// Panics if the screen holds no row for the position's line.
fn anchor_row(screen: &[ScreenRow], position: LogicalPosition) -> usize {
    let last = last_row_of(screen, position.line);
    (0..=last)
        .find(|&row| {
            screen[row].line == position.line
                && screen[row].row.start() <= position.grapheme
                && position.grapheme < screen[row].row.end()
        })
        .unwrap_or(last)
}

/// # Returns
///
/// * The index of the screen row `position` is drawn on.
/// * The column of that row it is drawn in.
fn drawn_at(
    screen: &[ScreenRow],
    position: LogicalPosition,
    wrapping: &Wrapping,
) -> (usize, usize) {
    let row = anchor_row(screen, position);
    let drawn = &screen[row].row;
    let column = drawn.columns()[position.grapheme - drawn.start()];
    if drawn.end() == position.grapheme && wrapping.width().get() <= column {
        return (row + 1, 0);
    }

    (row, column)
}

/// # Returns
///
/// Whether `position` rests past the last grapheme of a line whose last row is full, where it is
/// drawn on a row belonging to the line below and so is not a position that cell maps back to.
fn past_a_full_row(screen: &[ScreenRow], position: LogicalPosition, wrapping: &Wrapping) -> bool {
    let last = last_row_of(screen, position.line);

    screen[last].row.end() == position.grapheme
        && wrapping.width().get() <= screen[last].row.width()
}

/// # Returns
///
/// The index of the last screen row that draws a slice of `line`.
///
/// # Panics
///
/// Panics if the screen holds no row for the line.
fn last_row_of(screen: &[ScreenRow], line: usize) -> usize {
    screen
        .iter()
        .rposition(|row| row.line == line)
        .expect("every line is drawn on a row of its own")
}

/// # Returns
///
/// The column each grapheme of a screen row is drawn in, the column past its text excluded.
fn row_columns(drawn: &ScreenRow) -> Vec<usize> {
    let columns = drawn.row.columns();

    columns[..columns.len() - 1].to_vec()
}
