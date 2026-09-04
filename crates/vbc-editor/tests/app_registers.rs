//! The one register file the whole application holds, driven the way a reader drives it.
//!
//! This is the gesture vimbecode exists for, and until now it worked everywhere except in the
//! program. `Engine::sharing` and `Panel::sharing` were written, and `shared_registers.rs` drove
//! them side by side and stayed green, because that file hands the two halves one register file in
//! a fixture of its own. The binary never did: `App` built an engine, built a panel beside it, and
//! handed neither the other's registers, so `yac` in the transcript filled a drawer `p` in the
//! file could not open. A test that wires the thing it is testing the wiring of proves the
//! mechanism and says nothing about the product.
//!
//! So nothing here constructs an `Engine`, a `Panel` or a `Registers`, and nothing here calls
//! `sharing`. Every case builds an [`App`] the way the binary builds one, types the keys a reader
//! types -- `<C-T>` to cross, `j` to walk, `yac` to take, `p` to put -- and reads back the text the
//! file now holds. What is left to read off state rather than off the text is only what the file
//! editor cannot show: the shape a register was filled with, and what the panel mirrored to the
//! desktop's clipboard.
//!
//! Five things are asked of the crossing rather than one, because a register is more than the
//! bytes it holds. Every structural yank crosses, `yad`'s among them, and the patch that arrives
//! is handed to `git apply` rather than compared against a string, because a patch that reads
//! right and does not apply is no patch. The named registers cross in both directions. The shape
//! crosses, so a linewise yank in the panel is a linewise put in the file and a charwise one lands
//! inside a line. And a plain `p` still reads the unnamed register alone, or `p` would stop
//! meaning "the thing I just yanked".
//!
//! A put is spelled at more than one place in more than one file, because the last line of a
//! one-line file is the one place a linewise register carrying no line break of its own reads
//! right, and it is not where a reader puts a code block. So every structural yank is put at the
//! end of a file, above a line of one, three times over with a count, and above with `P`.
//!
//! The last two cases are the guard, and they are what `shared_registers.rs` cannot be. One walks
//! every way an application can be built -- every ordering of the builders, a transcript replaced
//! by another, a file opened off the disk -- and requires the crossing of each, so a constructor
//! that forgets the register file fails here however green the component tests are. The other
//! reads the source that builds the application and requires that it builds exactly one register
//! file: every engine and every panel it constructs beyond the first is handed that one, which is
//! the property a constructor written next year is held to without anyone remembering to add it
//! below.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::engine::{typed, Shape};
use vbc_editor::gutter::Options as GutterOptions;
use vbc_layout::buffer::Buffer;
use vbc_layout::line::Options;
use vbc_layout::width::Metrics;

/// The window every case is driven in, which is wide enough that the fixture's lines do not wrap
/// and tall enough to draw the whole of it.
const COLUMNS: u16 = 80;
const ROWS: u16 = 24;

/// The file the reader has open behind the transcript, which is what every put here lands in.
const FILE: &str = "a file the reader left open";

/// The line under it, so that a put lands between two lines of the file rather than only ever
/// after its last.
const BELOW: &str = "a line the reader wrote under it";

/// What was asked, and the answer holding the code the reader came for.
const ASKED: &str = "why does it not build";
const ANSWERED: &str = concat!(
    "Here it is:\n",
    "\n",
    "```rust\n",
    "fn main() {\n",
    "    todo!();\n",
    "}\n",
    "```\n",
    "\n",
    "That should do it.",
);

/// The code that answer fenced, which is what `yac` takes out of it.
const CODE: &str = "fn main() {\n    todo!();\n}";

/// What the tool answered, which is the block that folds.
const WROTE: &str = "   Compiling vimbecode\nerror: a semicolon was expected";

/// The edit that answer made: the file, the text it replaced, and the text it wrote.
const PATH: &str = "src/main.rs";
const BEFORE: &str = "fn main() {}\n";
const AFTER: &str = "fn main() {\n    todo!();\n}\n";

/// The line of the flattened transcript the code the reader wants is written on, the line the
/// answer's last is, and the line the closed fold over what the tool wrote is drawn in.
const INSIDE_THE_CODE: usize = 5;
const LAST_OF_THE_ANSWER: usize = 9;
const THE_CLOSED_FOLD: usize = LAST_OF_THE_ANSWER + 1;

/// The first word of the question, which is what a charwise yank in the panel takes.
const FIRST_WORD: &str = "why";

/// The name the file opened off the disk is written under, and the name the patch is written under
/// while `git apply` is asked to take it.
const OPENED_FILE: &str = "left-open.txt";
const PATCH_FILE: &str = "yanked.patch";

/// What `git apply` says when it had to move a hunk to make it fit.
const MOVED: &str = "with fuzz";

/// The source that builds the application, which is what the guard reads.
const APPLICATION: &str = "crates/vbc-editor/src/app.rs";

/// The constructions that would build a register file of their own, and the call that hands one an
/// existing file instead.
const CONSTRUCTIONS: [&str; 3] = ["Engine::new(", "Panel::new(", "Panel::laid_out_in("];
const SHARING: &str = ".sharing(";

/// The one construction the application is allowed to leave unshared, which is the file every
/// other engine and panel it builds is handed.
const ORIGIN: &str = "Engine::new(";

/// The application as it was built before the register file was hoisted: an engine of its own and
/// a panel of its own, neither handed the other's registers. The guard is required to find it, so
/// that a guard which has stopped covering the code it names fails rather than passes quietly.
const KEEPING_TWO_FILES: &str = "\
let mut app = Self {
    engine: Engine::new(&written(&text)),
    panel: Panel::new(Transcript::new()),
};
";

/// Validation 1: the headline gesture, spelled at the program rather than at its parts.
#[test]
fn a_code_block_yanked_in_the_panel_is_what_a_put_in_the_file_inserts() {
    let mut app = reading(FILE);
    walk(&mut app, INSIDE_THE_CODE);
    press(&mut app, "yac");
    cross(&mut app);
    press(&mut app, "p");

    assert_eq!(Focus::Text, app.focus(), "`<C-T>` did not come back");
    assert_eq!(format!("{FILE}\n{CODE}"), app.text().text());
}

/// Validation 2: every structural yank crosses, put at every place a reader puts one.
#[test]
fn every_structural_yank_crosses_into_the_file_the_reader_puts_it_in() {
    let two_lines = format!("{FILE}\n{BELOW}");
    for (down_to, keys, taken) in [
        (INSIDE_THE_CODE, "yac", CODE),
        (INSIDE_THE_CODE, "yam", ANSWERED),
        (THE_CLOSED_FOLD, "yat", WROTE),
    ] {
        for (put, file, laid) in [
            ("p", FILE, format!("{FILE}\n{taken}")),
            ("p", two_lines.as_str(), format!("{FILE}\n{taken}\n{BELOW}")),
            (
                "3p",
                two_lines.as_str(),
                format!("{FILE}\n{taken}\n{taken}\n{taken}\n{BELOW}"),
            ),
            (
                "jP",
                two_lines.as_str(),
                format!("{FILE}\n{taken}\n{BELOW}"),
            ),
        ] {
            let mut app = reading(file);
            walk(&mut app, down_to);
            press(&mut app, keys);
            cross(&mut app);
            press(&mut app, put);

            assert_eq!(
                laid,
                app.text().text(),
                "`{keys}` in the panel is not what `{put}` in the file put"
            );
        }
    }
}

/// Validation 2, continued: what `yad` puts into the file is a patch rather than something that
/// reads like one.
#[test]
fn a_diff_yanked_in_the_panel_is_a_patch_git_apply_takes_out_of_the_file() -> Result<()> {
    let mut app = reading(FILE);
    press(&mut app, "G");
    press(&mut app, "yad");
    cross(&mut app);
    press(&mut app, "p");

    let put = app.text().text();
    let patch: String = put
        .lines()
        .skip(1)
        .map(|line| format!("{line}\n"))
        .collect();

    assert!(
        patch.contains("@@"),
        "what `yad` put into the file is no unified patch: {patch:?}"
    );
    assert_eq!(Some(AFTER.to_owned()), applied(PATH, BEFORE, &patch)?);

    Ok(())
}

/// Validation 3: a register a reader named is the same register on the other side of `<C-T>`,
/// whichever side it was filled at.
#[test]
fn a_named_register_crosses_the_boundary_in_both_directions() {
    let mut app = reading(FILE);
    press(&mut app, "\"ayy");
    cross(&mut app);
    press(&mut app, "\"ap");

    assert_eq!(
        format!("{FILE}\n{ASKED}"),
        app.text().text(),
        "`\"ayy` in the panel is not what `\"ap` in the file put"
    );

    let mut app = reading(FILE);
    cross(&mut app);
    press(&mut app, "\"byy");
    cross(&mut app);
    press(&mut app, "yy");

    assert_eq!(
        Some(format!("{FILE}\n")),
        app.panel().register('b').map(|held| held.text),
        "the register the file editor named is not one the panel can read"
    );

    cross(&mut app);
    press(&mut app, "\"bp");

    assert_eq!(
        format!("{FILE}\n{FILE}"),
        app.text().text(),
        "a yank in the panel overwrote the register the file editor named"
    );
}

/// Validation 4: a register is the shape it was filled with as well as the bytes it holds.
#[test]
fn the_shape_a_yank_in_the_panel_took_is_the_shape_the_put_in_the_file_lays_down() {
    let mut app = reading(FILE);
    press(&mut app, "yy");

    assert_eq!(
        Some(Shape::Linewise),
        app.panel().register('"').map(|held| held.shape)
    );

    cross(&mut app);
    press(&mut app, "p");

    assert_eq!(
        format!("{FILE}\n{ASKED}"),
        app.text().text(),
        "a linewise yank in the panel was not put back as whole lines"
    );

    let mut app = reading(FILE);
    press(&mut app, "vey");

    assert_eq!(
        Some(Shape::Charwise),
        app.panel().register('"').map(|held| held.shape)
    );
    assert_eq!(
        Some(FIRST_WORD.to_owned()),
        app.panel().register('"').map(|held| held.text)
    );

    cross(&mut app);
    press(&mut app, "p");
    let (first, rest) = FILE.split_at(1);

    assert_eq!(
        format!("{first}{FIRST_WORD}{rest}"),
        app.text().text(),
        "a charwise yank in the panel was not put back inside the line"
    );
}

/// Validation 5: what the panel mirrored to the desktop is not what the next `p` comes back with.
#[test]
fn a_plain_put_reads_what_was_yanked_rather_than_what_reached_the_clipboard() {
    let mut app = reading(FILE);
    walk(&mut app, INSIDE_THE_CODE);
    press(&mut app, "yac");
    cross(&mut app);
    press(&mut app, "yyp");

    assert_eq!(
        format!("{FILE}\n{FILE}"),
        app.text().text(),
        "a plain put read the clipboard's register rather than the unnamed one"
    );
    assert_eq!(
        Some(format!("{CODE}\n")),
        app.panel().register('+').map(|held| held.text),
        "what the panel yanked never reached the clipboard's register"
    );
    assert_eq!(
        Some(format!("{FILE}\n")),
        app.panel().register('"').map(|held| held.text),
        "the unnamed register the put read is not the one the panel reads"
    );
}

/// Validation 6: every way an application is built is a way the crossing works, so a constructor
/// that builds a panel or an engine of its own fails here rather than in a bug report.
#[test]
fn every_way_an_application_is_built_hands_both_halves_the_one_register_file() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(OPENED_FILE);
    fs::write(&path, format!("{FILE}\n"))?;

    let built = vec![
        (
            "a transcript and nothing else",
            App::new(Buffer::from_text(FILE)).with_transcript(said()),
        ),
        (
            "a transcript given after the other builders",
            App::new(Buffer::from_text(FILE))
                .with_status(true)
                .with_gutter(GutterOptions::new().with_number(true))
                .with_scrolloff(1)
                .with_transcript(said()),
        ),
        (
            "a transcript given before the other builders",
            App::new(Buffer::from_text(FILE))
                .with_transcript(said())
                .with_status(true)
                .with_metrics(Metrics::default())
                .with_options(Options::new())
                .with_path(path.clone()),
        ),
        (
            "a transcript replaced by another",
            App::new(Buffer::from_text(FILE))
                .with_transcript(Transcript::new())
                .with_transcript(said()),
        ),
        (
            "a file opened off the disk",
            App::opened(path.clone())?.with_transcript(said()),
        ),
    ];

    for (built_by, mut app) in built {
        cross(&mut app);

        assert_eq!(
            Focus::Transcript,
            app.focus(),
            "`<C-T>` reached no panel in an application built with {built_by}"
        );

        walk(&mut app, INSIDE_THE_CODE);
        press(&mut app, "yac");
        cross(&mut app);
        press(&mut app, "p");

        assert_eq!(
            format!("{FILE}\n{CODE}"),
            app.text().text(),
            "an application built with {built_by} keeps registers its other half cannot read"
        );
    }

    Ok(())
}

/// Validation 6, continued: the source that builds the application builds one register file, so a
/// constructor written after this one is held to handing that file on rather than to being listed
/// above.
#[test]
fn the_application_builds_one_register_file_and_hands_it_to_everything_else_it_builds() {
    let held = fs::read_to_string(workspace().join(APPLICATION))
        .expect("the workspace holds the source that builds the application");

    assert_eq!(
        vec![ORIGIN.to_owned()],
        unshared(&held),
        "the application builds an engine or a panel that keeps registers of its own"
    );
}

/// The proof that the guard bites: the application as it was built while the gesture did not work
/// is what the scan is required to find.
#[test]
fn the_guard_finds_the_application_that_kept_a_register_file_on_either_side() {
    assert_eq!(
        vec!["Engine::new(".to_owned(), "Panel::new(".to_owned()],
        unshared(KEEPING_TWO_FILES),
        "the scan read an application whose two halves shared nothing and found it sound"
    );
}

/// # Returns
///
/// The application a reader types at, over `file`, with the transcript panel already reached by
/// `<C-T>`.
///
/// # Panics
///
/// Panics if `<C-T>` did not reach the panel.
fn reading(file: &str) -> App {
    let mut app = App::new(Buffer::from_text(file))
        .with_status(true)
        .with_transcript(said());
    app.press(area(), control('t'));

    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    app
}

/// # Returns
///
/// The exchange every case is driven over: a question, the answer fencing the code the reader came
/// for, what a tool answered, and the diff the answer wrote.
fn said() -> Transcript {
    [
        Block::new(Kind::Message(Role::User), ASKED.to_owned()),
        Block::new(Kind::Message(Role::Assistant), ANSWERED.to_owned()),
        Block::new(Kind::ToolResult, WROTE.to_owned()),
        Block::diff(PATH.to_owned(), BEFORE, AFTER),
    ]
    .into_iter()
    .collect()
}

/// Types `keys` at the application, one key at a time, at whichever half has them.
fn press(app: &mut App, keys: &str) {
    for key in keys.chars() {
        app.press(area(), typed(key));
    }
}

/// Carries the cursor of whichever half has the keys `rows` rows down.
fn walk(app: &mut App, rows: usize) {
    press(app, &"j".repeat(rows));
}

/// Gives the keys to the other half of the application, as `<C-T>` does.
fn cross(app: &mut App) {
    app.press(area(), control('t'));
}

/// # Returns
///
/// The area every case is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The key event a terminal reports when `character` is typed with control held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}

/// # Returns
///
/// Every construction in `source` that builds a register file of its own rather than being handed
/// one, named by the call that builds it and sorted.
fn unshared(source: &str) -> Vec<String> {
    let code: Vec<&str> = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    let code = code.join("\n");
    let mut found = Vec::new();
    for construction in CONSTRUCTIONS {
        for (at, _) in code.match_indices(construction) {
            if !shares(&code, at) {
                found.push(construction.to_owned());
            }
        }
    }
    found.sort();

    found
}

/// # Returns
///
/// Whether the call beginning at `at` of `source` is handed a register file, which it is where
/// `.sharing(` is the very next thing written after the call is closed.
fn shares(source: &str, at: usize) -> bool {
    let mut depth = 0usize;
    for (index, character) in source[at..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' if 0 == depth => return false,
            ')' if 1 == depth => return source[at + index + 1..].trim_start().starts_with(SHARING),
            ')' => depth -= 1,
            _ => {}
        }
    }

    false
}

/// # Returns
///
/// The root of the workspace, which is two directories above this crate's own.
///
/// # Panics
///
/// Panics if this crate is not a crate of a workspace, which it is.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below the workspace")
        .to_path_buf()
}

/// Writes `old` to `path` in a directory of its own, hands `written` to `git apply` there, and
/// reads back what the file holds afterwards.
///
/// # Returns
///
/// What the file holds once the patch was applied, and `None` where `git apply` refused it.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`tempfile::tempdir`]'s return values on failure.
/// * Forwards [`std::fs::create_dir_all`]'s return values on failure.
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`std::process::Command::output`]'s return values on failure.
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
///
/// # Panics
///
/// Panics if `git apply` had to move a hunk away from the line the patch numbered it at.
fn applied(path: &str, old: &str, written: &str) -> Result<Option<String>> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join(path);
    fs::create_dir_all(file.parent().expect("a file in a directory has a parent"))?;
    fs::write(&file, old)?;
    fs::write(directory.path().join(PATCH_FILE), written)?;

    let ran = Command::new("git")
        .arg("apply")
        .arg("--verbose")
        .arg(PATCH_FILE)
        .current_dir(directory.path())
        .output()?;
    if !ran.status.success() {
        return Ok(None);
    }
    let said = String::from_utf8_lossy(&ran.stderr);

    assert!(
        !said.contains(MOVED),
        "`git apply` had to move a hunk: {said}"
    );

    Ok(Some(fs::read_to_string(&file)?))
}
