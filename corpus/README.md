# The differential test corpus

Every file in this directory whose name ends in `.toml` is a *section* of the corpus. A section
declares one or more cases, and a case is one starting state plus the keys to replay against both
vim and the vimbecode editor. The loader lives in `crates/vbc-oracle/src/corpus.rs`.

```toml
[[case]]
id = "ascii-prose-delete-word"                  # unique across the whole corpus
description = "Deleting the second word."       # one sentence
buffer = """
The quick brown fox
"""
keys = "wdw"                                    # vim notation, for example `dw` or `iabc<Esc>`
viewport_width = 40                             # cells
viewport_height = 24                            # text window lines, optional, 24 by default
tags = ["ascii", "wrap"]                        # at least one
options = { tabstop = 4 }                       # optional, see below
```

A case is replayed in the viewport it declares: the engine is given a text window `viewport_width`
cells wide and `viewport_height` lines tall, stripped of line numbers, sign column, fold column,
status line and tab line. The declared width is the width of the text itself, and the declared
height is the number of screen lines the buffer is drawn on -- the command line an editor keeps
below its text window is not one of them, so vim is asked for a screen one line taller than the
viewport. `viewport_height` is what `H`, `M`, `L` and the half-page scrolls are measured against:
on a buffer with lines to spare, `L` lands on the `viewport_height`-th line of the window.

A viewport is between 12 and 10000 cells wide and between 1 and 999 lines tall, and the loader
rejects a case declaring anything outside that. vim quietly widens a narrower window, quietly
shrinks a larger screen, and keeps no window shorter than one text line, so a case outside the
range would not be laid out where it says it is.

## Options

`options` is optional, and so is every field inside it. A field left out takes vim's own default.

| Field | Default | Meaning |
| --- | --- | --- |
| `wrap` | `true` | Whether a line too long for the viewport continues on the next screen line. |
| `breakindent` | `false` | Whether continuation screen lines repeat the line's indent. |
| `showbreak` | `""` | The marker put in front of a continuation screen line. |
| `linebreak` | `false` | Whether a line too long for the viewport breaks at a word boundary. |
| `tabstop` | `8` | The number of cells a tab advances to. |
| `shiftwidth` | `8` | The number of cells `>>` shifts by, zero to follow `tabstop`. |
| `expandtab` | `false` | Whether an inserted tab is spelled with spaces. |
| `ambiwidth` | `"single"` | How ambiguous-width characters are measured: `"single"` or `"double"`. |

## Tags

`ambiwidth`, `ascii`, `breakindent`, `cjk`, `code`, `combining`, `emoji`, `flag`, `nfd`, `nowrap`,
`showbreak`, `tab`, `wrap`, `word-motion`. A tag the loader does not know is a load failure, so
adding one means adding it to `Tag` in the loader first.

## Writing text

Prefer literal characters for text that is visible on its own, such as CJK. Write invisible or
composing code points -- zero-width joiners, variation selectors, regional indicators, and
combining marks -- as TOML `\u` escapes inside a basic (`"` or `"""`) string, so that a reviewer
can see which code points a case actually contains. Escapes are not processed inside literal
(`'''`) strings.

## Adding a case

The loader rejects a section that would contribute an unusable case: an unknown field or tag, an
empty key sequence, an untagged case, a viewport outside the range above, a zero tabstop, an
identifier repeated anywhere in the corpus, or a file that is not valid UTF-8. Every failure names
the offending file. The case count and the per-tag breakdown are asserted in
`corpus::tests::case_count_and_tag_breakdown_are_stable`, so adding a case means updating those two
constants, and recording the baseline again as described below.

Tags are checked against the case they label: a tag naming a class of code point requires the
buffer to hold one, a tag naming a display option requires the option to be set, and a case whose
buffer or options call for a tag has to carry it. A case holding a tab is tagged `tab` whatever
else it exercises.

## The baseline

`baseline.json` is the golden record of the state vim ends every case in: buffer, cursor, display
position, mode, and registers with their types. A differential run compares two engines with each
other and so says nothing about the reference side moving; the baseline is what catches a rewritten
capture, a different vim, or an edited case changing what vim is taken to say.

```bash
cargo run --bin differential-run -- --check-baseline   # report every case that no longer holds
cargo run --bin differential-run -- --record-baseline  # write the states vim ends the cases in
```

The file's header records the vim version the states were captured from, a hash of everything the
cases declare, and the version of the file's own schema. The hash and the schema version are
checked against any vim, so an edited case fails the check until the baseline is recorded again.

The recorded states are compared only when the check runs against the vim release series they were
captured from, since two vim releases end a case in different states by themselves. The
continuous-integration job installs that vim and checks the states strictly, and its
`record-baseline` job -- run by hand from the Actions tab -- is where a new baseline is recorded:
commit the artifact it uploads rather than a file recorded from whichever vim a developer has. A
check run against another vim says so, and passes without comparing the states.
