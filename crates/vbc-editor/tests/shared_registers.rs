//! What a reader yanks in the transcript panel, put into the file they came to write.
//!
//! This is the gesture vimbecode exists for. Claude answers with a code block, the reader takes it
//! out of the answer with `yac`, and pastes it into the file with `p`. The panel and the file are
//! two engines, each with its own text, its own cursor and its own mode, and for as long as they
//! also had registers of their own the gesture went nowhere: `yac` filled a register `p` could not
//! see, and the editor's headline feature was a yank into a drawer nothing opens.
//!
//! So the two are handed one register file, and every case here types the keys a reader types at
//! both ends of the crossing. Nothing calls `Yank::structural` or writes a register by hand: a
//! yank is spelled at the panel through the table the panel reads its keys with, a put is spelled
//! at the file editor, and what is read back is the text the file now holds. A case that filled a
//! register itself would pass against a dispatch no keystroke arrives at, which is the fault this
//! file exists to catch.
//!
//! A put is spelled at more than one place in more than one file, because the last line of a
//! one-line file is the one place a linewise register carrying no line break of its own reads
//! right, and it is not where a reader puts a code block. So every structural yank is put at the
//! end of a file, above a line of one, three times over with a count, and above with `P`.
//!
//! Five things are required of the crossing rather than one, because a register is more than the
//! bytes it holds. The structural yanks all cross, `yad`'s among them, and the patch that arrives
//! in the file is handed to `git apply` rather than compared against a string, because a patch
//! that reads right and does not apply is no patch. The named registers cross, and the black hole
//! still swallows. The shape crosses, so that a linewise yank in the panel is a linewise put in
//! the file and a charwise one lands inside a line. And a plain `p` still reads the unnamed
//! register alone: what the panel mirrored into `"+` for the desktop is not what the next `p`
//! comes back with, or `p` would stop meaning "the thing I just yanked".

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::Result;

use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::policy::Panel;
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::engine::{typed, Engine, Registers, Shape};

/// The file the reader has open behind the transcript, which is what every put here lands in.
const FILE: &str = "a file the reader left open";

/// The line under it, so that a put lands between two lines of the file rather than only ever
/// after its last. A linewise put that carried no line break of its own reads right at the end of
/// a file and joins two lines everywhere else, which is where a reader puts a code block.
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

/// The same answer of the same shape, fencing code that is nothing a column of cells is a good
/// model of: a tab, characters two columns wide, a cluster of several code points, and a line of
/// nothing. What crosses is bytes, so what arrives has to be the bytes that were taken.
const ANSWERED_HARD: &str = concat!(
    "Here it is:\n",
    "\n",
    "```rust\n",
    "fn main() {\n",
    "\tlet 名前 = \"\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} e\u{301}\";\n",
    "\n",
    "}\n",
    "```\n",
    "\n",
    "That should do it.",
);
const CODE_HARD: &str = concat!(
    "fn main() {\n",
    "\tlet 名前 = \"\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} e\u{301}\";\n",
    "\n",
    "}",
);

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

/// The name the patch is written under while `git apply` is asked to take it.
const PATCH_FILE: &str = "yanked.patch";

/// What `git apply` says when it had to move a hunk to make it fit.
const MOVED: &str = "with fuzz";

#[test]
fn a_code_block_yanked_in_the_panel_is_what_a_put_in_the_file_inserts() -> Result<()> {
    let (mut panel, mut engine) = reading();
    press(&mut panel, &down(INSIDE_THE_CODE))?;
    press(&mut panel, "yac")?;
    type_at(&mut engine, "p")?;

    assert_eq!(format!("{FILE}\n{CODE}\n"), engine.text());

    Ok(())
}

#[test]
fn every_structural_yank_crosses_into_the_file_the_reader_puts_it_in() -> Result<()> {
    for (down_to, keys, taken) in [
        (INSIDE_THE_CODE, "yac", CODE),
        (INSIDE_THE_CODE, "yam", ANSWERED),
        (THE_CLOSED_FOLD, "yat", WROTE),
    ] {
        let two_lines = format!("{FILE}\n{BELOW}");
        for (put, file, laid) in [
            ("p", FILE, format!("{FILE}\n{taken}\n")),
            (
                "p",
                two_lines.as_str(),
                format!("{FILE}\n{taken}\n{BELOW}\n"),
            ),
            (
                "3p",
                two_lines.as_str(),
                format!("{FILE}\n{taken}\n{taken}\n{taken}\n{BELOW}\n"),
            ),
            (
                "jP",
                two_lines.as_str(),
                format!("{FILE}\n{taken}\n{BELOW}\n"),
            ),
        ] {
            let (mut panel, mut engine) = reading_over(file);
            press(&mut panel, &down(down_to))?;
            press(&mut panel, keys)?;
            type_at(&mut engine, put)?;

            assert_eq!(
                laid,
                engine.text(),
                "`{keys}` in the panel is not what `{put}` in the file put"
            );
        }
    }

    Ok(())
}

#[test]
fn a_code_block_of_more_than_printable_ascii_crosses_byte_for_byte() -> Result<()> {
    let two_lines = format!("{FILE}\n{BELOW}");
    for (file, laid) in [
        (FILE, format!("{FILE}\n{CODE_HARD}\n")),
        (
            two_lines.as_str(),
            format!("{FILE}\n{CODE_HARD}\n{BELOW}\n"),
        ),
    ] {
        let (mut panel, mut engine) = reading_answering(ANSWERED_HARD, file);
        press(&mut panel, &down(INSIDE_THE_CODE))?;
        press(&mut panel, "yac")?;
        type_at(&mut engine, "p")?;

        assert_eq!(
            laid,
            engine.text(),
            "a code block of tabs, wide characters and clusters did not cross whole"
        );
    }

    Ok(())
}

#[test]
fn a_diff_yanked_in_the_panel_is_a_patch_git_apply_takes_out_of_the_file() -> Result<()> {
    let (mut panel, mut engine) = reading();
    press(&mut panel, "G")?;
    press(&mut panel, "yad")?;
    type_at(&mut engine, "p")?;

    let put = engine.text();
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

#[test]
fn a_named_register_filled_in_the_panel_is_the_one_the_file_puts_from() -> Result<()> {
    let (mut panel, mut engine) = reading();
    press(&mut panel, "\"ayy")?;
    type_at(&mut engine, "\"ap")?;

    assert_eq!(format!("{FILE}\n{ASKED}\n"), engine.text());

    Ok(())
}

#[test]
fn the_black_hole_swallows_what_was_yanked_into_it_on_either_side_of_the_crossing() -> Result<()> {
    let (mut panel, mut engine) = reading();
    press(&mut panel, &down(INSIDE_THE_CODE))?;
    press(&mut panel, "yac")?;
    press(&mut panel, "gg")?;
    press(&mut panel, "\"_yy")?;
    type_at(&mut engine, "p")?;

    assert_eq!(
        format!("{FILE}\n{CODE}\n"),
        engine.text(),
        "a yank into the black hole reached the register a plain put reads"
    );
    assert_eq!(None, engine.register('_'));

    Ok(())
}

#[test]
fn a_plain_put_reads_what_was_yanked_rather_than_what_reached_the_clipboard() -> Result<()> {
    let (mut panel, mut engine) = reading();
    press(&mut panel, &down(INSIDE_THE_CODE))?;
    press(&mut panel, "yac")?;
    type_at(&mut engine, "yyp")?;

    assert_eq!(
        format!("{FILE}\n{FILE}\n"),
        engine.text(),
        "a plain put read the clipboard's register rather than the unnamed one"
    );
    assert_eq!(
        Some(format!("{CODE}\n")),
        engine.register('+').map(|held| held.text),
        "what the panel yanked never reached the clipboard's register"
    );

    Ok(())
}

#[test]
fn the_shape_a_yank_in_the_panel_took_is_the_shape_the_put_in_the_file_lays_down() -> Result<()> {
    let (mut panel, mut engine) = reading();
    press(&mut panel, "yy")?;

    assert_eq!(
        Some(Shape::Linewise),
        engine.register('"').map(|held| held.shape)
    );

    type_at(&mut engine, "p")?;

    assert_eq!(
        format!("{FILE}\n{ASKED}\n"),
        engine.text(),
        "a linewise yank in the panel was not put back as whole lines"
    );

    let (mut panel, mut engine) = reading();
    press(&mut panel, "vey")?;

    assert_eq!(
        Some(Shape::Charwise),
        engine.register('"').map(|held| held.shape)
    );
    assert_eq!(
        Some(FIRST_WORD.to_owned()),
        engine.register('"').map(|held| held.text)
    );

    type_at(&mut engine, "p")?;
    let (first, rest) = FILE.split_at(1);

    assert_eq!(
        format!("{first}{FIRST_WORD}{rest}\n"),
        engine.text(),
        "a charwise yank in the panel was not put back inside the line"
    );

    Ok(())
}

#[test]
fn the_two_engines_a_reader_types_at_hold_the_same_registers_and_nobody_elses() -> Result<()> {
    let (mut panel, mut engine) = reading();
    let (mut elsewhere, _file) = reading();
    press(&mut panel, &down(INSIDE_THE_CODE))?;
    press(&mut panel, "yac")?;
    press(&mut elsewhere, "yy")?;

    assert_eq!(
        Some(format!("{CODE}\n")),
        engine.register('"').map(|held| held.text),
        "the panel and the file editor do not read the same registers"
    );

    type_at(&mut engine, "p")?;

    assert_eq!(
        format!("{FILE}\n{CODE}\n"),
        engine.text(),
        "a yank in a panel nobody shares registers with reached the file"
    );

    Ok(())
}

/// # Returns
///
/// The transcript panel and the file editor a reader has open, sharing the one register file that
/// makes a yank in either a put in the other.
fn reading() -> (Panel, Engine) {
    reading_over(FILE)
}

/// # Returns
///
/// The same two, the file editor laid over `file` rather than over the one line of [`FILE`], so
/// that a put lands somewhere other than after the last line of what the reader has open.
fn reading_over(file: &str) -> (Panel, Engine) {
    reading_answering(ANSWERED, file)
}

/// # Returns
///
/// The same two again, the transcript answering `answer` rather than [`ANSWERED`], so that what
/// crosses is something other than the printable ASCII a terminal is a good model of.
fn reading_answering(answer: &str, file: &str) -> (Panel, Engine) {
    let registers = Registers::new();
    let panel = Panel::new(said(answer)).sharing(registers.clone());
    let engine = Engine::new(file).sharing(registers);

    (panel, engine)
}

/// # Returns
///
/// The exchange every case is driven over: a question, `answer` fencing the code the reader came
/// for, what a tool answered, and the diff the answer wrote.
fn said(answer: &str) -> Transcript {
    [
        Block::new(Kind::Message(Role::User), ASKED.to_owned()),
        Block::new(Kind::Message(Role::Assistant), answer.to_owned()),
        Block::new(Kind::ToolResult, WROTE.to_owned()),
        Block::diff(PATH.to_owned(), BEFORE, AFTER),
    ]
    .into_iter()
    .collect()
}

/// Types `keys` at the transcript panel, one key at a time.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Panel::press_all`]'s return values on failure.
fn press(panel: &mut Panel, keys: &str) -> Result<()> {
    panel.press_all(keys.chars().map(typed))?;

    Ok(())
}

/// Types `keys` at the file editor, one key at a time.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Engine::press_all`]'s return values on failure.
fn type_at(engine: &mut Engine, keys: &str) -> Result<()> {
    engine.press_all(keys.chars().map(typed))?;

    Ok(())
}

/// # Returns
///
/// The keys that carry the panel's cursor `lines` lines down from where it started.
fn down(lines: usize) -> String {
    "j".repeat(lines)
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

    Ok(Some(read(&file)?))
}

/// # Returns
///
/// What the file at `path` holds.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
fn read(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?)
}
