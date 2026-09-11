//! An edit drawn the way a reviewer reads one, proven on the cells the application draws and on
//! the registers its keys fill.
//!
//! The cases drive [`App`] and read the cells it drew rather than the rows a block renders,
//! because a band the rows carry is not a band a reader sees until the renderer fills the panel
//! with it. They draw the panel at more than one width, and with the keys both in it and in the
//! prompt below it, because a band that reached the edge of one panel and stopped short of another
//! would pass a case that drew only one.
//!
//! The patch the tool reports is the one Claude Code 2.1.269 reported for this very edit, read off
//! a live session, so the case that redraws a diff from it is fed the frame the child writes
//! rather than one written to suit the parser.

use std::fs;
use std::process::Command;

use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde_json::json;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::diff::SNIPPET_NOTE;
use vbc_editor::chat::palette::{
    Palette, ADDED_BAND, ADDED_EMPHASIS, REMOVED_BAND, REMOVED_EMPHASIS,
};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::chat::yank::patch;
use vbc_editor::engine::typed;
use vbc_editor::session::blocks::Conversation;
use vbc_editor::session::event::Event;
use vbc_layout::buffer::Buffer;

/// The rows every case is drawn in, and the widths the panel is drawn at.
const ROWS: u16 = 24;
const WIDTHS: [u16; 3] = [41, 80, 120];

/// What the file editor holds, which no case reads.
const FILE: &str = "a file the reader left open";

/// What the reader asked for, which holds neither word the edit changes so that the row a word is
/// found in is a row of the diff.
const ASKED: &str = "change the greeting";

/// The file the edit is to, and what it held before the edit and after it.
const PATH: &str = "src/main.rs";
const BEFORE: &str = concat!(
    "fn main() {\n",
    "    let greeting = \"hello\";\n",
    "    println!(\"{greeting}\");\n",
    "    let count = 1;\n",
    "    let other = 2;\n",
    "    let more = 3;\n",
    "    let last = 4;\n",
    "}\n",
);
const AFTER: &str = concat!(
    "fn main() {\n",
    "    let greeting = \"goodbye\";\n",
    "    println!(\"{greeting}\");\n",
    "    let count = 1;\n",
    "    let other = 2;\n",
    "    let more = 3;\n",
    "    let last = 4;\n",
    "}\n",
);

/// The line the edit took away and the line it put in its place, which are what the Edit tool is
/// called with, and the word each of them holds that the other does not.
const TAKEN: &str = "    let greeting = \"hello\";";
const PUT: &str = "    let greeting = \"goodbye\";";
const TAKEN_WORD: &str = "hello";
const PUT_WORD: &str = "goodbye";

/// The patch `yad` took out of this edit before a diff was drawn with a gutter, which is the one
/// it must take now.
const PATCH: &str = concat!(
    "--- a/src/main.rs\n",
    "+++ b/src/main.rs\n",
    "@@ -1,5 +1,5 @@\n",
    " fn main() {\n",
    "-    let greeting = \"hello\";\n",
    "+    let greeting = \"goodbye\";\n",
    "     println!(\"{greeting}\");\n",
    "     let count = 1;\n",
    "     let other = 2;\n",
);

/// The row of the panel the added line is drawn in, below the question, the diff's header, the
/// line above the change, and the line taken away.
const ADDED_ROW: usize = 4;

/// The id the edit is called under, and what the tool answered it with.
const EDIT_ID: &str = "toolu_01UtAH3RCrfU4Fo9iG9jNZ6d";
const UPDATED: &str = "The file src/main.rs has been updated successfully.";

/// The file `git apply` is handed the patch in.
const PATCH_FILE: &str = "yanked.patch";

#[test]
fn a_changed_row_is_drawn_on_its_band_across_the_whole_panel_and_numbered_beside_it() {
    let palette = Palette::detected();
    for width in WIDTHS {
        for (app, focused) in [(composing(), false), (reading(), true)] {
            let area = area(width);
            let cells = drawn(&app, area);
            let taken = row_holding(&cells, area, TAKEN_WORD);
            let put = row_holding(&cells, area, PUT_WORD);
            let kept = row_holding(&cells, area, "fn main() {");

            for (row, band, emphasis) in [
                (taken, REMOVED_BAND, REMOVED_EMPHASIS),
                (put, ADDED_BAND, ADDED_EMPHASIS),
            ] {
                let band = palette.color(band);
                let emphasis = palette.color(emphasis);
                let backgrounds: Vec<Color> = (0..width).map(|x| cells[(x, row)].bg).collect();
                assert!(
                    backgrounds
                        .iter()
                        .all(|background| [band, emphasis].contains(background)),
                    "a changed row at {width} columns (history focused: {focused}) was drawn on \
                     {backgrounds:?} rather than on its band"
                );
                assert_eq!(
                    Some(&band),
                    backgrounds.last(),
                    "the band stopped short of the edge of a panel {width} columns wide"
                );
            }
            assert_eq!(
                Color::Reset,
                cells[(width - 1, kept)].bg,
                "an unchanged row was drawn on a band"
            );

            assert!(
                text_of(&cells, area, taken).starts_with("2   -"),
                "the line taken away was not numbered as the replaced text numbers it: {:?}",
                text_of(&cells, area, taken)
            );
            assert!(
                text_of(&cells, area, put).starts_with("  2 +"),
                "the line put in was not numbered as the written text numbers it: {:?}",
                text_of(&cells, area, put)
            );
            assert!(
                text_of(&cells, area, kept).starts_with("1 1  fn main() {"),
                "an unchanged line was not numbered in both texts: {:?}",
                text_of(&cells, area, kept)
            );
        }
    }
}

#[test]
fn a_one_word_change_is_emphasised_in_that_word_and_nowhere_else_on_the_line() {
    let palette = Palette::detected();
    let area = area(WIDTHS[1]);
    let cells = drawn(&composing(), area);

    for (word, emphasis) in [(TAKEN_WORD, REMOVED_EMPHASIS), (PUT_WORD, ADDED_EMPHASIS)] {
        let row = row_holding(&cells, area, word);
        assert_eq!(
            word,
            drawn_on(&cells, area, row, palette.color(emphasis)),
            "the emphasis on the line holding {word:?} covered something other than that word"
        );
    }
}

#[test]
fn yy_on_an_added_row_and_yad_over_the_edit_take_what_they_took_before() -> Result<()> {
    let mut app = reading();
    for _ in 0..ADDED_ROW {
        app.press(area(WIDTHS[1]), typed('j'));
    }
    for key in "yy".chars() {
        app.press(area(WIDTHS[1]), typed(key));
    }
    assert_eq!(
        format!("+{PUT}\n"),
        unnamed(&mut app),
        "`yy` on the added row took something other than the row's own line, which is what it \
         took before the diff was numbered"
    );

    for key in "yad".chars() {
        app.press(area(WIDTHS[1]), typed(key));
    }
    let written = unnamed(&mut app);
    assert_eq!(PATCH, written);
    assert_eq!(Some(AFTER.to_owned()), applied(BEFORE, &written)?);

    Ok(())
}

#[test]
fn an_edit_is_drawn_again_from_the_patch_the_tool_reports_numbered_as_the_file_numbers_it(
) -> Result<()> {
    let mut conversation = Conversation::new();
    conversation.read(&called()?);

    let snippet = conversation
        .transcript()
        .block(0)
        .ok_or(anyhow!("the call made no block"))?;
    assert_eq!(
        &Kind::Diff {
            path: PATH.to_owned()
        },
        snippet.kind()
    );
    assert!(
        first_line(snippet.source()).ends_with(SNIPPET_NOTE),
        "a diff numbered within the edit did not say so: {:?}",
        snippet.source()
    );

    conversation.read(&answered(PATH)?);
    let reported = conversation
        .transcript()
        .block(0)
        .ok_or(anyhow!("the answer took the diff away"))?;
    assert!(
        !first_line(reported.source()).ends_with(SNIPPET_NOTE),
        "a diff numbered as the file numbers it said it was numbered within the edit"
    );
    assert_eq!(Some(PATCH.to_owned()), patch(reported));
    assert_eq!(
        Some(&Kind::ToolResult),
        conversation.transcript().block(1).map(Block::kind),
        "the answer itself was not kept"
    );

    let (transcript, tags) = conversation.into_panel();
    let app = App::chat().with_conversation(transcript, tags);
    let area = area(WIDTHS[1]);
    let cells = drawn(&app, area);
    let later = row_holding(&cells, area, "let other = 2;");
    assert!(
        text_of(&cells, area, later).starts_with("5 5  "),
        "a line the file holds below the edit was not numbered where the file numbers it: {:?}",
        text_of(&cells, area, later)
    );

    Ok(())
}

#[test]
fn a_patch_reported_for_another_file_leaves_the_diff_as_the_call_drew_it() -> Result<()> {
    let mut conversation = Conversation::new();
    conversation.read(&called()?);
    let before = conversation.transcript().clone();

    conversation.read(&answered("src/other.rs")?);

    assert_eq!(before.block(0), conversation.transcript().block(0));

    Ok(())
}

/// # Returns
///
/// The conversation screen showing the edit, with the keys at the prompt below it.
fn composing() -> App {
    App::chat().with_transcript(said())
}

/// # Returns
///
/// The application with the keys in the history showing the edit, reached by `<C-T>`.
fn reading() -> App {
    let mut app = App::new(Buffer::from_text(FILE))
        .with_status(true)
        .with_transcript(said());
    app.press(area(WIDTHS[1]), control('t'));
    assert_eq!(Focus::History, app.focus(), "`<C-T>` reached no panel");

    app
}

/// # Returns
///
/// The exchange every case is drawn over: the question, and the diff of the edit it asked for.
fn said() -> Transcript {
    [
        Block::new(Kind::Message(Role::User), ASKED.to_owned()),
        Block::diff(PATH.to_owned(), BEFORE, AFTER),
    ]
    .into_iter()
    .collect()
}

/// # Returns
///
/// The assistant frame calling the Edit tool with [`TAKEN`] and [`PUT`].
fn called() -> Result<Event> {
    let frame = json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "id": EDIT_ID,
            "name": "Edit",
            "input": {
                "replace_all": false,
                "file_path": PATH,
                "old_string": TAKEN,
                "new_string": PUT,
            },
        }]},
        "parent_tool_use_id": null,
    });

    Ok(Event::decoded(&frame.to_string())?)
}

/// # Returns
///
/// The user frame answering the edit, reporting the patch it applied to the file at `path`.
fn answered(path: &str) -> Result<Event> {
    let frame = json!({
        "type": "user",
        "message": {"content": [{
            "tool_use_id": EDIT_ID,
            "type": "tool_result",
            "content": UPDATED,
        }]},
        "parent_tool_use_id": null,
        "tool_use_result": {
            "filePath": path,
            "oldString": TAKEN,
            "newString": PUT,
            "structuredPatch": [{
                "oldStart": 1,
                "oldLines": 5,
                "newStart": 1,
                "newLines": 5,
                "lines": [
                    " fn main() {",
                    "-    let greeting = \"hello\";",
                    "+    let greeting = \"goodbye\";",
                    "     println!(\"{greeting}\");",
                    "     let count = 1;",
                    "     let other = 2;",
                ],
            }],
            "userModified": false,
            "replaceAll": false,
        },
    });

    Ok(Event::decoded(&frame.to_string())?)
}

/// # Returns
///
/// The cells `app` draws into `area`.
fn drawn(app: &App, area: Rect) -> Cells {
    let mut cells = Cells::empty(area);
    app.draw(&mut cells, area);

    cells
}

/// # Returns
///
/// The first row of `cells` whose text holds `text`.
///
/// # Panics
///
/// Panics if no row does, which means the panel did not draw what the case is about.
fn row_holding(cells: &Cells, area: Rect, text: &str) -> u16 {
    (area.y..area.bottom())
        .find(|row| text_of(cells, area, *row).contains(text))
        .unwrap_or_else(|| panic!("no row of the panel drew {text:?}"))
}

/// # Returns
///
/// The text row `row` of `cells` was drawn with, trailing blanks left off.
fn text_of(cells: &Cells, area: Rect, row: u16) -> String {
    let drawn: String = (area.x..area.right())
        .map(|column| cells[(column, row)].symbol().to_owned())
        .collect();

    drawn.trim_end().to_owned()
}

/// # Returns
///
/// The text of row `row` of `cells` drawn on the background `background`.
fn drawn_on(cells: &Cells, area: Rect, row: u16, background: Color) -> String {
    (area.x..area.right())
        .filter(|column| background == cells[(*column, row)].bg)
        .map(|column| cells[(column, row)].symbol().to_owned())
        .collect()
}

/// # Returns
///
/// What the unnamed register of `app`'s history holds.
///
/// # Panics
///
/// Panics if it holds nothing, which means the yank the case typed took nothing.
fn unnamed(app: &mut App) -> String {
    app.panel()
        .registers()
        .get(&'"')
        .map(|held| held.text.clone())
        .expect("the yank filled the unnamed register")
}

/// Applies the patch `written` to a file at [`PATH`] holding `old`, as `git apply` does.
///
/// # Returns
///
/// What the file holds once the patch is applied, or `None` where `git apply` refused it.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`tempfile::tempdir`]'s return values on failure.
/// * Forwards [`fs::create_dir_all`]'s return values on failure.
/// * Forwards [`fs::write`]'s return values on failure.
/// * Forwards [`Command::output`]'s return values on failure.
/// * Forwards [`fs::read_to_string`]'s return values on failure.
fn applied(old: &str, written: &str) -> Result<Option<String>> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join(PATH);
    fs::create_dir_all(file.parent().expect("a file in a directory has a parent"))?;
    fs::write(&file, old)?;
    fs::write(directory.path().join(PATCH_FILE), written)?;

    let ran = Command::new("git")
        .arg("apply")
        .arg(PATCH_FILE)
        .current_dir(directory.path())
        .output()?;
    if !ran.status.success() {
        return Ok(None);
    }

    Ok(Some(fs::read_to_string(&file)?))
}

/// # Returns
///
/// The first line of `source`, which is a diff's header.
fn first_line(source: &str) -> &str {
    source.lines().next().unwrap_or_default()
}

/// # Returns
///
/// The area every case is drawn in, `width` columns wide.
fn area(width: u16) -> Rect {
    Rect::new(0, 0, width, ROWS)
}

/// # Returns
///
/// The key event a terminal reports when `character` is typed with control held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}
