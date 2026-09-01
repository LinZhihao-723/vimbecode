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
tags = ["ascii", "wrap"]                        # at least one
options = { tabstop = 4 }                       # optional, see below
```

## Options

`options` is optional, and so is every field inside it. A field left out takes vim's own default.

| Field | Default | Meaning |
| --- | --- | --- |
| `wrap` | `true` | Whether a line too long for the viewport continues on the next screen line. |
| `breakindent` | `false` | Whether continuation screen lines repeat the line's indent. |
| `showbreak` | `""` | The marker put in front of a continuation screen line. |
| `tabstop` | `8` | The number of cells a tab advances to. |
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
empty key sequence, an untagged case, a zero-width viewport, a zero tabstop, an identifier repeated
anywhere in the corpus, or a file that is not valid UTF-8. Every failure names the offending file.
The case count and the per-tag breakdown are asserted in
`corpus::tests::case_count_and_tag_breakdown_are_stable`, so adding a case means updating those two
constants.

Tags are checked against the case they label: a tag naming a class of code point requires the
buffer to hold one, a tag naming a display option requires the option to be set, and a case whose
buffer or options call for a tag has to carry it. A case holding a tab is tagged `tab` whatever
else it exercises.
