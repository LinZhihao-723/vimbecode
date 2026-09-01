//! The state schema through which the vimbecode editor and vim are compared.
//!
//! A snapshot captures everything the differential harness observes about an engine, and comparing
//! two snapshots reports each dimension in which they diverge.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The name a register is addressed by, for example `"` for the unnamed register.
pub type RegisterName = char;

/// How a register's text is laid out, which determines how a put reinserts it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RegisterType {
    /// The text is inserted at the cursor, without starting a new line.
    Charwise,

    /// The text is inserted as whole lines, above or below the cursor's line.
    Linewise,

    /// The text is inserted as a rectangular block spanning consecutive lines.
    Blockwise,
}

/// The text held by a register, together with the layout it was yanked or deleted with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Register {
    /// The register's text, byte-exact.
    pub text: String,

    /// The layout the text is put back with.
    pub register_type: RegisterType,
}

/// The cursor's position, as a zero-based line index and a zero-based byte offset within that line.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Cursor {
    /// The zero-based index of the line the cursor is on.
    pub line: u64,

    /// The zero-based byte offset of the cursor within its line.
    pub column: u64,
}

/// Where the cursor is drawn, as a zero-based screen row and a zero-based screen column within
/// the viewport the buffer is laid out in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DisplayPosition {
    /// The screen row the cursor is drawn on, counted from the top of the viewport.
    pub row: u64,

    /// The screen column the cursor is drawn in, counted from the left of the viewport.
    pub column: u64,
}

/// The mode an engine is in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Mode {
    /// Keys are interpreted as commands.
    Normal,

    /// Keys are inserted into the buffer.
    Insert,

    /// Keys overwrite the text under the cursor.
    Replace,

    /// A characterwise selection is active.
    Visual,

    /// A linewise selection is active.
    VisualLine,

    /// A blockwise selection is active.
    VisualBlock,

    /// An operator is waiting for the motion that completes it.
    OperatorPending,

    /// A command line, search, or filter prompt is being typed.
    CommandLine,
}

/// A snapshot of everything the differential harness compares between two engines.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EditorState {
    /// The buffer's full text, compared byte for byte.
    pub buffer: String,

    /// The cursor's position.
    pub cursor: Cursor,

    /// Where the cursor is drawn, which the buffer's layout in the viewport decides.
    pub display_position: DisplayPosition,

    /// The mode the engine is in.
    pub mode: Mode,

    /// The registers holding text, keyed by name. A register absent from the map holds nothing.
    pub registers: BTreeMap<RegisterName, Register>,
}

impl EditorState {
    /// Compares two snapshots dimension by dimension.
    ///
    /// # Returns
    ///
    /// Every dimension in which the two snapshots disagree, empty if they are equal.
    #[must_use]
    pub fn diff(&self, other: &Self) -> Vec<Divergence> {
        let mut divergences = Vec::new();

        if self.buffer != other.buffer {
            divergences.push(Divergence::Buffer {
                left: self.buffer.clone(),
                right: other.buffer.clone(),
            });
        }
        if self.cursor != other.cursor {
            divergences.push(Divergence::Cursor {
                left: self.cursor,
                right: other.cursor,
            });
        }
        if self.display_position != other.display_position {
            divergences.push(Divergence::DisplayPosition {
                left: self.display_position,
                right: other.display_position,
            });
        }
        if self.mode != other.mode {
            divergences.push(Divergence::Mode {
                left: self.mode,
                right: other.mode,
            });
        }

        let names: BTreeSet<RegisterName> = self
            .registers
            .keys()
            .chain(other.registers.keys())
            .copied()
            .collect();
        for name in names {
            let left = self.registers.get(&name);
            let right = other.registers.get(&name);
            if left != right {
                divergences.push(Divergence::Register {
                    name,
                    left: left.cloned(),
                    right: right.cloned(),
                });
            }
        }

        divergences
    }
}

/// A single dimension in which two snapshots disagree, holding both sides' values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Divergence {
    /// The buffers' texts differ.
    Buffer {
        /// The left snapshot's buffer.
        left: String,

        /// The right snapshot's buffer.
        right: String,
    },

    /// The cursors' positions differ.
    Cursor {
        /// The left snapshot's cursor.
        left: Cursor,

        /// The right snapshot's cursor.
        right: Cursor,
    },

    /// The cursors are drawn in different places, which a buffer laid out differently causes
    /// even when the cursors hold the same position.
    DisplayPosition {
        /// The left snapshot's display position.
        left: DisplayPosition,

        /// The right snapshot's display position.
        right: DisplayPosition,
    },

    /// The modes differ.
    Mode {
        /// The left snapshot's mode.
        left: Mode,

        /// The right snapshot's mode.
        right: Mode,
    },

    /// One register's content or type differs, or only one snapshot holds it.
    Register {
        /// The register the snapshots disagree on.
        name: RegisterName,

        /// The left snapshot's register, `None` if it holds nothing.
        left: Option<Register>,

        /// The right snapshot's register, `None` if it holds nothing.
        right: Option<Register>,
    },
}

/// # Returns
///
/// A register's type and text, or that it holds nothing.
#[must_use]
pub fn describe_register(register: Option<&Register>) -> String {
    let Some(register) = register else {
        return "holds nothing".to_owned();
    };
    let register_type = match register.register_type {
        RegisterType::Charwise => "charwise",
        RegisterType::Linewise => "linewise",
        RegisterType::Blockwise => "blockwise",
    };

    format!("{register_type} {:?}", register.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// # Returns
    ///
    /// A state holding one register of each type, which the tests perturb one dimension at a time.
    fn sample_state() -> EditorState {
        EditorState {
            buffer: "alpha\nbeta\ngamma\n".to_owned(),
            cursor: Cursor { line: 1, column: 2 },
            display_position: DisplayPosition { row: 1, column: 2 },
            mode: Mode::Normal,
            registers: BTreeMap::from([
                (
                    '"',
                    Register {
                        text: "beta\n".to_owned(),
                        register_type: RegisterType::Linewise,
                    },
                ),
                (
                    'a',
                    Register {
                        text: "eta".to_owned(),
                        register_type: RegisterType::Charwise,
                    },
                ),
                (
                    '0',
                    Register {
                        text: "al\nbe\n".to_owned(),
                        register_type: RegisterType::Blockwise,
                    },
                ),
            ]),
        }
    }

    #[test]
    fn serialization_round_trips_unchanged() -> anyhow::Result<()> {
        let state = sample_state();
        let restored: EditorState = serde_json::from_str(&serde_json::to_string(&state)?)?;
        assert_eq!(restored, state);
        Ok(())
    }

    #[test]
    fn identical_states_do_not_diverge() {
        assert_eq!(sample_state().diff(&sample_state()), vec![]);
    }

    #[test]
    fn linewise_and_charwise_yanks_of_the_same_text_are_unequal() {
        let text = "beta\n";
        let mut linewise = sample_state();
        linewise.registers.insert(
            '"',
            Register {
                text: text.to_owned(),
                register_type: RegisterType::Linewise,
            },
        );
        let mut charwise = sample_state();
        charwise.registers.insert(
            '"',
            Register {
                text: text.to_owned(),
                register_type: RegisterType::Charwise,
            },
        );

        assert_ne!(charwise, linewise);
        assert_eq!(
            linewise.diff(&charwise),
            vec![Divergence::Register {
                name: '"',
                left: Some(Register {
                    text: text.to_owned(),
                    register_type: RegisterType::Linewise,
                }),
                right: Some(Register {
                    text: text.to_owned(),
                    register_type: RegisterType::Charwise,
                }),
            }]
        );
    }

    #[test]
    fn cursor_only_difference_reports_the_cursor() {
        let left = sample_state();
        let mut right = sample_state();
        right.cursor = Cursor { line: 1, column: 3 };

        assert_eq!(
            left.diff(&right),
            vec![Divergence::Cursor {
                left: Cursor { line: 1, column: 2 },
                right: Cursor { line: 1, column: 3 },
            }]
        );
    }

    #[test]
    fn display_position_only_difference_reports_the_display_position() {
        let left = sample_state();
        let mut right = sample_state();
        right.display_position = DisplayPosition { row: 2, column: 0 };

        assert_eq!(
            left.diff(&right),
            vec![Divergence::DisplayPosition {
                left: DisplayPosition { row: 1, column: 2 },
                right: DisplayPosition { row: 2, column: 0 },
            }]
        );
    }

    #[test]
    fn display_position_round_trips_through_serialization() -> anyhow::Result<()> {
        let mut state = sample_state();
        state.display_position = DisplayPosition { row: 3, column: 17 };

        let restored: EditorState = serde_json::from_str(&serde_json::to_string(&state)?)?;

        assert_eq!(restored.display_position, state.display_position);
        assert_eq!(restored, state);
        assert_ne!(
            serde_json::to_string(&state)?,
            serde_json::to_string(&sample_state())?,
            "two states that differ only in their display position serialize the same way"
        );
        Ok(())
    }

    #[test]
    fn mode_only_difference_reports_the_mode() {
        let left = sample_state();
        let mut right = sample_state();
        right.mode = Mode::Insert;

        assert_eq!(
            left.diff(&right),
            vec![Divergence::Mode {
                left: Mode::Normal,
                right: Mode::Insert,
            }]
        );
    }

    #[test]
    fn register_only_difference_reports_the_register() {
        let left = sample_state();
        let mut right = sample_state();
        right.registers.remove(&'a');

        assert_eq!(
            left.diff(&right),
            vec![Divergence::Register {
                name: 'a',
                left: Some(Register {
                    text: "eta".to_owned(),
                    register_type: RegisterType::Charwise,
                }),
                right: None,
            }]
        );
    }

    #[test]
    fn register_held_only_by_the_right_side_is_reported() {
        let left = sample_state();
        let mut right = sample_state();
        let added = Register {
            text: "gamma".to_owned(),
            register_type: RegisterType::Charwise,
        };
        right.registers.insert('z', added.clone());

        assert_eq!(
            left.diff(&right),
            vec![Divergence::Register {
                name: 'z',
                left: None,
                right: Some(added),
            }]
        );
    }

    #[test]
    fn buffer_comparison_is_byte_exact() {
        let left = sample_state();
        for buffer in [
            "alpha\nbeta\ngamma",
            "alpha\nbeta \ngamma\n",
            " alpha\nbeta\ngamma\n",
            "alpha\r\nbeta\r\ngamma\r\n",
        ] {
            let mut right = sample_state();
            right.buffer = buffer.to_owned();

            assert_eq!(
                left.diff(&right),
                vec![Divergence::Buffer {
                    left: left.buffer.clone(),
                    right: buffer.to_owned(),
                }]
            );
        }
    }
    #[test]
    fn every_diverging_dimension_is_reported() {
        let left = sample_state();
        let mut right = sample_state();
        right.buffer = "delta\n".to_owned();
        right.cursor = Cursor { line: 0, column: 0 };
        right.display_position = DisplayPosition { row: 0, column: 0 };
        right.mode = Mode::VisualBlock;
        right.registers.insert(
            '"',
            Register {
                text: "beta\n".to_owned(),
                register_type: RegisterType::Charwise,
            },
        );
        right.registers.remove(&'a');

        assert_eq!(
            left.diff(&right),
            vec![
                Divergence::Buffer {
                    left: "alpha\nbeta\ngamma\n".to_owned(),
                    right: "delta\n".to_owned(),
                },
                Divergence::Cursor {
                    left: Cursor { line: 1, column: 2 },
                    right: Cursor { line: 0, column: 0 },
                },
                Divergence::DisplayPosition {
                    left: DisplayPosition { row: 1, column: 2 },
                    right: DisplayPosition { row: 0, column: 0 },
                },
                Divergence::Mode {
                    left: Mode::Normal,
                    right: Mode::VisualBlock,
                },
                Divergence::Register {
                    name: '"',
                    left: Some(Register {
                        text: "beta\n".to_owned(),
                        register_type: RegisterType::Linewise,
                    }),
                    right: Some(Register {
                        text: "beta\n".to_owned(),
                        register_type: RegisterType::Charwise,
                    }),
                },
                Divergence::Register {
                    name: 'a',
                    left: Some(Register {
                        text: "eta".to_owned(),
                        register_type: RegisterType::Charwise,
                    }),
                    right: None,
                },
            ]
        );
    }
}
