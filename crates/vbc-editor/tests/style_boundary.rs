//! The one type that says how a cell is drawn, wherever a cell is drawn.
//!
//! A style crosses three seams on its way to the screen: the spans a block is painted with, the
//! numbers a gutter draws, and the cells a renderer fills. A second type anywhere along that path
//! costs a conversion at every crossing, and a conversion that has to be written before the path
//! can be walked at all is a path nothing walks.
//!
//! The type is checked two ways, because either alone would let the other kind of regression
//! through. Handing one style to all three seams says they agree today, and would not compile if
//! they stopped; reading the sources says which type they agreed on, and catches a second one
//! reintroduced behind a conversion.

use std::any::TypeId;
use std::fs;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use vbc_editor::gutter::Options as GutterOptions;
use vbc_editor::render::Renderer;
use vbc_editor::style::{Span, Style};
use vbc_layout::width::Metrics;

/// The modules a style crosses between, each paired with the declaration naming the type it draws
/// in, so that a scan reading a file it did not understand fails rather than passes.
const SEAMS: [(&str, &str); 3] = [
    ("crates/vbc-editor/src/gutter.rs", "use ratatui::style::"),
    ("crates/vbc-editor/src/render.rs", "use ratatui::style::"),
    (
        "crates/vbc-editor/src/style.rs",
        "pub type Style = ratatui::style::Style;",
    ),
];

/// The spellings of a second style type, which is the shape a conversion at the boundary takes
/// however it is spelt.
const SECOND_STYLE_TYPES: [&str; 3] = ["crossterm::style", "ContentStyle", "ratatui_crossterm"];

#[test]
fn one_style_type_crosses_the_gutter_render_and_style_boundary() {
    let style = Style::new().fg(Color::Magenta);

    let span = Span::new(0..1, style);
    let gutter = GutterOptions::new().with_number_style(style);
    let renderer = Renderer::new(Metrics::default()).with_style(style);

    assert_eq!(style, span.style());
    assert_eq!(style, gutter.number_style());
    assert_eq!(style, renderer.style());
    assert_eq!(
        TypeId::of::<ratatui::style::Style>(),
        TypeId::of::<Style>(),
        "the editor's style is not the type the cells of a terminal buffer carry"
    );
}

#[test]
fn no_second_style_type_is_named_at_that_boundary() {
    let mut named = Vec::new();
    for (file, declaration) in SEAMS {
        let source = read(&workspace().join(file));
        assert!(
            source.contains(declaration),
            "{file} does not hold `{declaration}`, so it draws in some other type"
        );

        for (number, line) in source.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if SECOND_STYLE_TYPES.iter().any(|name| code.contains(name)) {
                named.push(format!("{file}:{}: {code}", number + 1));
            }
        }
    }

    assert_eq!(Vec::<String>::new(), named);
}

/// # Returns
///
/// The contents of a file of this workspace.
///
/// # Panics
///
/// Panics if the file cannot be read.
fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// # Returns
///
/// The root of the workspace this crate belongs to.
///
/// # Panics
///
/// Panics if this crate is not a member of a workspace.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below its workspace root")
        .to_owned()
}
