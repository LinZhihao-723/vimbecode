//! The invariant search over the reference layout: that it survives a hundred thousand generated
//! views, that the harness catches each invariant being broken, that a failure is reported as a
//! shrunk case with a replayable seed, and that the generator really draws the text and the
//! options the search is only useful for covering.
//!
//! A hundred thousand views is minutes of search rather than seconds, and a soak everybody's
//! `cargo test` pays for is a soak somebody eventually shrinks. So it is the one test here the
//! default run skips, and continuous integration runs it on every pull request instead.
//!
//! What makes that a gate rather than a way of quietly losing the soak is read here too, because
//! neither half of it fails on its own: a workflow step nobody runs is green, and a filter that
//! matches no test reports success just as loudly as one that matches. The workflow is therefore
//! required to run the soak from an unconditional job, by the name and with the flags that really
//! run it, and the soak is required to be the only test of this crate the default run leaves out
//! and to be declared in the target that workflow names, since a soak moved into another source
//! answers to the filter no better than a renamed one.

mod fuzz;

use std::fs;
use std::path::{Path, PathBuf};

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, TestRunner};

use fuzz::harness::{
    layout_input, search, FuzzFailure, LayoutInput, Seed, DEFAULT_CASES, FLAG, ZWJ_FAMILY,
};
use fuzz::reference::WholeDocumentLayout;
use fuzz::violations::{
    DropsAGrapheme, MergesLastCell, OverdrawsRows, OverflowsEndOfLine, PadsWithAnEmptyRow,
};
use vbc_layout::invariants::{graphemes, Invariant, Layout};
use vbc_layout::line;
use vbc_layout::width::{AmbiWidth, DEFAULT_TAB_STOP};

/// The seed every search in these tests starts from, chosen so each planted defect is found.
const SEED: Seed = Seed::new(0x7669_6D62_6563_6F64);

/// A second seed, used to check that a search follows the seed it is given.
const ALTERNATE_SEED: Seed = Seed::new(0x6C61_796F_7574_0001);

/// The number of cases the search over the reference layout runs. A layout defect that shows up
/// on one case in ten thousand is a defect a reader meets, so the search is run at a scale that
/// meets it too, which is the scale that keeps it out of the default run.
const SOAK_CASES: u32 = 100_000;

/// The least a soak may search and still be one. A search shrunk until it fits a default run is
/// the cheapest way to lose one, and it is the only way that would leave every test here green, so
/// it stops the crate compiling instead.
const LEAST_SOAK_CASES: u32 = 100_000;
const _: () = assert!(LEAST_SOAK_CASES <= SOAK_CASES, "the soak stopped soaking");

/// The soak's own name, which is the name continuous integration runs it by.
const SOAK_TEST: &str = "the_reference_layout_survives_a_hundred_thousand_cases";

/// The test target the soak is written in, which continuous integration names to run it.
const SOAK_TARGET: &str = "layout_fuzz";

/// The workflow that must run the soak, and the job of it that every pull request runs.
const WORKFLOW: [&str; 3] = [".github", "workflows", "ci.yaml"];
const WORKFLOW_JOB: &str = "test";

/// The event the workflow must be triggered by for a pull request to pay for the soak.
const PULL_REQUEST: &str = "pull_request:";

/// What a job of the workflow is written under, and what everything inside one is indented past.
const JOB_KEY: &str = "  ";
const INSIDE_A_JOB: &str = "    ";

/// The keys that would excuse a job of the workflow, or a step of one, from being run and from
/// failing. The job that runs the soak is required to hold neither, anywhere: a soak run only
/// sometimes, or run and then forgiven, is a soak a pull request does not have to pass.
const CONDITION: &str = "if:";
const FORGIVEN: &str = "continue-on-error:";

/// What opens a line of the workflow that is read rather than run, and which is therefore no
/// evidence that anything runs the soak.
const COMMENT: &str = "#";

/// The attribute that keeps a test out of the default run, and the keyword opening the test it is
/// written above.
const SKIPPED: &str = "#[ignore";
const TEST_FUNCTION: &str = "fn ";

/// The flags that run a test the default run skips and no other, which is what the workflow has to
/// name the soak with: a run that filtered every test out would report success just as loudly as
/// one that searched.
const SOAK_FLAGS: &str = "--include-ignored --exact";

/// The seed the coverage tests draw their cases from, and the number they draw.
const COVERAGE_SEED: Seed = Seed::new(0x636F_7665_7261_6765);
const COVERAGE_CASES: usize = 2_000;

/// The number of the drawn cases that must carry each of the two shapes layouts break on, set
/// above what the generator's unconstrained arm reaches on its own so that dropping a deliberate
/// arm turns this into a failing test rather than a smaller number nobody reads.
const HARD_SHAPE_CASES: usize = 420;

/// Draws cases from the generator a search runs over, so that what a search covers can be measured
/// rather than assumed.
///
/// # Returns
///
/// `count` cases, generated from `seed`.
///
/// # Panics
///
/// Panics if the case generator cannot produce a case.
fn cases(seed: Seed, count: usize) -> Vec<LayoutInput> {
    let mut runner = TestRunner::new_with_rng(
        Config {
            failure_persistence: None,
            ..Config::default()
        },
        seed.rng(),
    );
    let strategy = layout_input();

    (0..count)
        .map(|_| {
            strategy
                .new_tree(&mut runner)
                .expect("the case generator produces a case")
                .current()
        })
        .collect()
}

/// Searches a layout that is known to be broken.
///
/// # Type Parameters
///
/// * `LayoutType` - The broken layout.
///
/// # Returns
///
/// The failure the harness found.
///
/// # Panics
///
/// Panics if the harness cleared the layout.
fn expect_failure<LayoutType: Layout>(layout: &LayoutType, seed: Seed) -> FuzzFailure {
    *search(layout, seed, DEFAULT_CASES)
        .expect_err("the harness must catch a layout that breaks an invariant")
}

/// Asserts that a failure breaks the invariant its layout was built to break, and only that one.
///
/// A layout that breaks several invariants at once would satisfy every one of these tests without
/// any of them exercising the check it is named for, so exclusivity is what makes each test
/// evidence that its own invariant is enforced.
fn assert_violates(failure: &FuzzFailure, invariant: Invariant) {
    let broken: Vec<Invariant> = failure
        .violations
        .iter()
        .map(|violation| violation.invariant)
        .collect();

    assert_eq!(broken, vec![invariant], "the harness reported:\n{failure}");
}

/// Asserts that enough of the generated cases carry a shape the search is only useful for
/// covering.
fn assert_covers(
    drawn: &[LayoutInput],
    shape: &str,
    minimum: usize,
    covered: impl Fn(&LayoutInput) -> bool,
) {
    let count = drawn.iter().filter(|input| covered(input)).count();

    assert!(
        minimum <= count,
        "only {count} of {} generated cases carry {shape}, fewer than the {minimum} a search needs",
        drawn.len()
    );
}

/// # Returns
///
/// Whether any line of the case holds `text`.
fn holds(input: &LayoutInput, text: &str) -> bool {
    input.buffer.lines().iter().any(|line| line.contains(text))
}

/// # Returns
///
/// Whether the case draws a line whose continuation rows are decorated with under two columns left
/// beside the decoration, and whose text meets that decoration with a two-column cluster.
fn squeezes_a_wide_cluster(input: &LayoutInput) -> bool {
    let wrapping = &input.viewport.wrapping;
    let metrics = wrapping.metrics();
    let width = input.viewport.width();

    input.buffer.lines().iter().enumerate().any(|(line, text)| {
        let decoration =
            line::continuation_decoration(text, wrapping.width(), metrics, wrapping.options());
        let decoration_width = metrics.text_width(&decoration, 0);
        if decoration.is_empty() || decoration_width + 2 <= width {
            return false;
        }

        line::lay_out(line, text, wrapping.width(), metrics, wrapping.options())
            .iter()
            .skip(1)
            .any(|row| {
                graphemes(row.text())
                    .next()
                    .is_some_and(|grapheme| 2 == metrics.grapheme_width(grapheme, 0))
            })
    })
}

/// # Returns
///
/// Whether the case's cursor rests past the last grapheme of its line.
fn rests_past_a_line(input: &LayoutInput) -> bool {
    input.buffer.line_len(input.cursor.line) == Some(input.cursor.grapheme)
}

/// # Returns
///
/// Whether the case's cursor rests past the last grapheme of a line whose last row is exactly
/// full, where the cell drawing it belongs to the row below.
fn rests_past_a_full_row(input: &LayoutInput) -> bool {
    if !rests_past_a_line(input) {
        return false;
    }
    let wrapping = &input.viewport.wrapping;
    let Some(text) = input.buffer.line(input.cursor.line) else {
        return false;
    };

    line::lay_out(
        input.cursor.line,
        text,
        wrapping.width(),
        wrapping.metrics(),
        wrapping.options(),
    )
    .last()
    .is_some_and(|row| input.viewport.width() <= row.width())
}

/// # Returns
///
/// The command continuous integration must run the soak with, down to its flags.
fn soak_command() -> String {
    format!("cargo test -p vbc-layout --test {SOAK_TARGET} -- {SOAK_FLAGS} {SOAK_TEST}")
}

/// # Returns
///
/// The workflow that must run the soak.
///
/// # Panics
///
/// Panics if the workflow cannot be read.
fn workflow() -> String {
    let path = WORKFLOW
        .iter()
        .fold(workspace(), |path, name| path.join(name));

    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// # Returns
///
/// What keeps `workflow` from running the soak on every pull request, empty if nothing does.
fn unrun_by(workflow: &str) -> Vec<String> {
    let mut complaints = Vec::new();
    let command = soak_command();
    let job = job(workflow, WORKFLOW_JOB);

    if !workflow.contains(PULL_REQUEST) {
        complaints.push(format!("the workflow is not triggered by `{PULL_REQUEST}`"));
    }
    if job.is_empty() {
        complaints.push(format!("the workflow holds no `{WORKFLOW_JOB}` job"));
    }
    for excuse in excuses(&job) {
        let excuse = excuse.trim();
        complaints.push(format!("`{WORKFLOW_JOB}` is excused by `{excuse}`"));
    }
    if !collapsed(&job).contains(&command) {
        complaints.push(format!("`{WORKFLOW_JOB}` does not run `{command}`"));
    }

    complaints
}

/// # Returns
///
/// The lines of one job of `workflow`, which are the lines under it indented inside it.
fn job<'workflow>(workflow: &'workflow str, name: &str) -> Vec<&'workflow str> {
    let opening = format!("{JOB_KEY}{name}:");

    workflow
        .lines()
        .skip_while(|line| line.trim_end() != opening)
        .skip(1)
        .take_while(|line| line.trim().is_empty() || line.starts_with(INSIDE_A_JOB))
        .collect()
}

/// # Returns
///
/// The lines of a job that would excuse it, or a step of it, from being run and from failing,
/// which a job every pull request has to pass has none of.
fn excuses<'job>(job: &[&'job str]) -> Vec<&'job str> {
    job.iter()
        .filter(|line| {
            let key = line.trim().trim_start_matches("- ");
            key.starts_with(CONDITION) || key.starts_with(FORGIVEN)
        })
        .copied()
        .collect()
}

/// # Returns
///
/// The lines a job runs, with the breaks and the blank space between their words collapsed, so
/// that a command a workflow wrapped over several lines reads as the one command it runs and a
/// command written where it is only read reads as nothing.
fn collapsed(lines: &[&str]) -> String {
    lines
        .iter()
        .filter(|line| !line.trim_start().starts_with(COMMENT))
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\\', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// # Returns
///
/// The workflow with the step that runs the soak struck out of it.
fn without_the_soak(workflow: &str) -> String {
    workflow
        .lines()
        .filter(|line| !line.contains(SOAK_TEST))
        .collect::<Vec<_>>()
        .join("\n")
}

/// # Returns
///
/// The workflow with the command that runs the soak commented out where it is written, which is a
/// step that still reads as one and runs nothing.
fn commented_out(workflow: &str) -> String {
    workflow
        .lines()
        .map(|line| {
            if !line.contains(SOAK_TARGET) && !line.contains(SOAK_TEST) {
                return line.to_owned();
            }
            let command = line.trim_start();
            let indent = &line[..line.len() - command.len()];

            format!("{indent}{COMMENT} {command}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// # Returns
///
/// The workflow with `excuse` written into the job that runs the soak.
fn excused_by(workflow: &str, excuse: &str) -> String {
    let opening = format!("{JOB_KEY}{WORKFLOW_JOB}:\n");

    workflow.replace(&opening, &format!("{opening}{INSIDE_A_JOB}{excuse}\n"))
}

/// # Returns
///
/// The name of every test of this crate the default run skips, each written under the source that
/// declares it, sorted.
///
/// The source is half of what a name is worth here, because the workflow names a test target as
/// well as a test: a soak moved into another source of the crate answers to neither the name nor
/// the target `--exact` is given, and a filter that matches nothing is the silence this gate
/// exists to break.
///
/// # Panics
///
/// Panics if a source of the crate cannot be read.
fn skipped_tests() -> Vec<String> {
    let mut skipped: Vec<String> = sources(Path::new(env!("CARGO_MANIFEST_DIR")))
        .iter()
        .flat_map(|source| {
            let text = fs::read_to_string(source)
                .unwrap_or_else(|error| panic!("{} is readable: {error}", source.display()));

            declared_in(source, skipped_in(&text))
        })
        .collect();
    skipped.sort();

    skipped
}

/// # Returns
///
/// Each of `tests` written under the source declaring it, which for a source that is a test target
/// of its own is the target continuous integration names to run it.
///
/// # Panics
///
/// Panics if `source` is not a named file.
fn declared_in(source: &Path, tests: Vec<String>) -> Vec<String> {
    let declaring = source
        .file_stem()
        .expect("a source read out of the tree is a named file")
        .to_string_lossy()
        .into_owned();

    tests
        .into_iter()
        .map(|test| format!("{declaring}::{test}"))
        .collect()
}

/// # Returns
///
/// The name of every test `source` keeps out of the default run.
fn skipped_in(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();

    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with(SKIPPED))
        .filter_map(|(index, _)| {
            lines[index..]
                .iter()
                .find_map(|line| line.strip_prefix(TEST_FUNCTION))
        })
        .map(|signature| {
            let end = signature.find(['(', '<']).unwrap_or(signature.len());
            signature[..end].trim().to_owned()
        })
        .collect()
}

/// # Returns
///
/// Every Rust source of the tree rooted at `directory`.
///
/// # Panics
///
/// Panics if the tree cannot be read.
fn sources(directory: &Path) -> Vec<PathBuf> {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", directory.display()));

    entries
        .flat_map(|entry| {
            let path = entry
                .unwrap_or_else(|error| panic!("{} is readable: {error}", directory.display()))
                .path();
            if path.is_dir() {
                sources(&path)
            } else if path.extension().is_some_and(|extension| "rs" == extension) {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}

/// # Returns
///
/// The root of the workspace this crate sits in.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below its workspace root")
        .to_owned()
}

#[test]
fn the_reference_layout_satisfies_every_invariant() {
    for seed in 0..8 {
        if let Err(failure) = search(&WholeDocumentLayout, Seed::new(seed), DEFAULT_CASES) {
            panic!("the reference layout broke an invariant:\n{failure}");
        }
    }
}

#[test]
#[ignore = "minutes of search; continuous integration runs it on every pull request"]
fn the_reference_layout_survives_a_hundred_thousand_cases() {
    if let Err(failure) = search(&WholeDocumentLayout, SEED, SOAK_CASES) {
        panic!("the reference layout broke an invariant:\n{failure}");
    }
}

#[test]
fn the_default_run_skips_the_soak_and_nothing_else() {
    assert_eq!(vec![format!("{SOAK_TARGET}::{SOAK_TEST}")], skipped_tests());
}

#[test]
fn continuous_integration_runs_the_soak_on_every_pull_request() {
    assert_eq!(Vec::<String>::new(), unrun_by(&workflow()));
}

#[test]
fn a_workflow_that_stopped_running_the_soak_is_caught() {
    let workflow = workflow();
    let stopped = [
        without_the_soak(&workflow),
        commented_out(&workflow),
        excused_by(
            &workflow,
            &format!("{CONDITION} \"github.event_name == 'push'\""),
        ),
        excused_by(&workflow, &format!("{FORGIVEN} true")),
        workflow.replace(SOAK_FLAGS, "--exact"),
        workflow.replace(SOAK_TEST, "a_test_by_another_name"),
        workflow.replace(&format!("{JOB_KEY}{WORKFLOW_JOB}:"), "  released:"),
        workflow.replace(PULL_REQUEST, "schedule:"),
    ];

    for workflow in stopped {
        assert_ne!(
            Vec::<String>::new(),
            unrun_by(&workflow),
            "a workflow that stopped running the soak was passed:\n{workflow}"
        );
    }
}

#[test]
fn a_test_kept_out_of_the_default_run_is_read_off_its_source() {
    let soak = format!("#[test]\n{SKIPPED} = \"minutes\"]\n{TEST_FUNCTION}a_soak() {{}}\n");
    let search = format!("#[test]\n{TEST_FUNCTION}a_search() {{}}\n");

    assert_eq!(vec!["a_soak".to_owned()], skipped_in(&soak));
    assert_eq!(Vec::<String>::new(), skipped_in(&search));
    assert_eq!(
        vec!["another_target::a_soak".to_owned()],
        declared_in(Path::new("tests/another_target.rs"), skipped_in(&soak))
    );
}

#[test]
fn row_width_violation_is_caught() {
    assert_violates(&expect_failure(&OverdrawsRows, SEED), Invariant::RowWidth);
}

#[test]
fn grapheme_conservation_violation_is_caught() {
    assert_violates(
        &expect_failure(&DropsAGrapheme, SEED),
        Invariant::GraphemeConservation,
    );
}

#[test]
fn no_empty_rows_violation_is_caught() {
    assert_violates(
        &expect_failure(&PadsWithAnEmptyRow, SEED),
        Invariant::NoEmptyRows,
    );
}

#[test]
fn cursor_visible_violation_is_caught() {
    assert_violates(
        &expect_failure(&OverflowsEndOfLine, SEED),
        Invariant::CursorVisible,
    );
}

#[test]
fn round_trip_violation_is_caught() {
    assert_violates(&expect_failure(&MergesLastCell, SEED), Invariant::RoundTrip);
}

#[test]
fn printed_seed_replays_the_same_failure() {
    let failure = expect_failure(&MergesLastCell, SEED);
    let replayed_seed: Seed = failure
        .seed
        .to_string()
        .parse()
        .expect("the printed seed must parse back");
    let replayed = expect_failure(&MergesLastCell, replayed_seed);

    assert_eq!(failure, replayed);
}

#[test]
fn different_seeds_search_different_cases() {
    let failure = expect_failure(&OverdrawsRows, SEED);
    let alternate = expect_failure(&OverdrawsRows, ALTERNATE_SEED);

    assert_ne!(
        failure.original, alternate.original,
        "both seeds searched the same cases, so a reported seed replays nothing"
    );
}

#[test]
fn shrinking_reduces_the_failing_case() {
    let failure = expect_failure(&MergesLastCell, SEED);

    assert!(
        failure.minimal.size() < failure.original.size(),
        "shrinking did not reduce the case:\n{failure}"
    );
}

#[test]
fn the_generator_draws_the_text_a_layout_is_hard_on() {
    let drawn = cases(COVERAGE_SEED, COVERAGE_CASES);

    assert_covers(&drawn, "a tab", 100, |input| holds(input, "\t"));
    assert_covers(&drawn, "a joined emoji", 100, |input| {
        holds(input, ZWJ_FAMILY)
    });
    assert_covers(&drawn, "a flag", 100, |input| holds(input, FLAG));
    assert_covers(&drawn, "a combining mark", 100, |input| {
        holds(input, "e\u{0301}") || holds(input, "a\u{301}\u{302}\u{323}")
    });
    assert_covers(&drawn, "a double-width cluster", 100, |input| {
        holds(input, "漢")
    });
    assert_covers(&drawn, "an ambiguous-width letter", 100, |input| {
        holds(input, "α")
    });
}

#[test]
fn the_generator_draws_the_display_options() {
    let drawn = cases(COVERAGE_SEED, COVERAGE_CASES);

    assert_covers(&drawn, "breakindent", 100, |input| {
        input.viewport.wrapping.options().break_indent()
    });
    assert_covers(&drawn, "a showbreak marker", 100, |input| {
        !input.viewport.wrapping.options().show_break().is_empty()
    });
    assert_covers(&drawn, "linebreak", 100, |input| {
        input.viewport.wrapping.options().line_break()
    });
    assert_covers(&drawn, "a tab stop vim does not default to", 100, |input| {
        DEFAULT_TAB_STOP != input.viewport.wrapping.metrics().tab_stop().get()
    });
    assert_covers(&drawn, "vim's default tab stop", 20, |input| {
        DEFAULT_TAB_STOP == input.viewport.wrapping.metrics().tab_stop().get()
    });
    assert_covers(&drawn, "ambiwidth=double", 100, |input| {
        AmbiWidth::Double == input.viewport.wrapping.metrics().ambiwidth()
    });
    assert_covers(&drawn, "a window one row tall", 100, |input| {
        1 == input.viewport.height.get()
    });
    assert_covers(&drawn, "a window several rows tall", 100, |input| {
        1 < input.viewport.height.get()
    });
}

#[test]
fn the_generator_draws_the_shapes_layouts_break_on() {
    let drawn = cases(COVERAGE_SEED, COVERAGE_CASES);

    assert_covers(
        &drawn,
        "a two-column cluster with under two columns beside a continuation decoration",
        HARD_SHAPE_CASES,
        squeezes_a_wide_cluster,
    );
    assert_covers(&drawn, "a cursor past the end of its line", 100, |input| {
        rests_past_a_line(input)
    });
    assert_covers(
        &drawn,
        "a cursor past the end of a line whose last row is full",
        HARD_SHAPE_CASES,
        rests_past_a_full_row,
    );
}

#[test]
fn every_generated_grapheme_fits_a_row_and_fills_a_cell() {
    for input in cases(COVERAGE_SEED, COVERAGE_CASES) {
        let metrics = input.viewport.wrapping.metrics();
        let width = input.viewport.width();
        for line in input.buffer.lines() {
            let mut column = 0;
            for grapheme in graphemes(line) {
                let occupied = metrics.grapheme_width(grapheme, column);
                assert!(
                    0 < occupied && occupied <= width,
                    "`{}` occupies {occupied} columns of a viewport {width} wide: {input}",
                    grapheme.escape_debug()
                );
                column += occupied;
            }
        }
    }
}
