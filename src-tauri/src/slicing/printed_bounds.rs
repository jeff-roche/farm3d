//! D11 check 5: a slice's **printed bounds**, the extent of the moves that
//! actually lay down plastic inside the slicer's print body.
//!
//! [`PrintedBoundsScanner`] is fed one decoded line at a time by the P4
//! G-code inspector's streaming pass, so it keeps a few numbers and never
//! the file.
//!
//! - **Extruding moves only.** A `G0`/`G1` (or an arc's end point, `G2`/`G3`)
//!   counts when its E delta is positive. Both ends of the segment are
//!   included. Travel moves, Z-hops, retracts, and wipes (negative E) are
//!   not.
//! - **Positioning is tracked** as firmware does: `G90`/`G91` switch X, Y,
//!   and Z between absolute and relative, and `G91` also makes E relative.
//!   `M82`/`M83` switch E alone, and `G92` sets positions (all of them to
//!   zero with no words). `G28` makes the homed axes unknown until a move
//!   sets them.
//! - **The print body** runs from each `;LAYER_CHANGE` to the next
//!   `;TYPE:Custom` or `; EXECUTABLE_BLOCK_END`, the markers OrcaSlicer
//!   2.4.2 writes around the layers. The machine start G-code (with any
//!   off-bed purge line) comes before the first layer change, and the
//!   machine end G-code (wipe and park moves) after the last layer's
//!   `;TYPE:Custom`. A `;TYPE:Custom` inside the layers would only pause
//!   the check until the next layer change, never fail a slice.
//! - **No markers.** A G-code with no `;LAYER_CHANGE` is bounded by all its
//!   extruding moves.
//! - **Untrackable positioning** (inch units, or an extruding move whose
//!   start, end, or extruder position is unknown) in the chosen scope
//!   skips the check, and [`BoundsCheck::Skipped`] records why.
//!
//! Arc moves contribute their end points only; OrcaSlicer's arc fitting is
//! off by default.

use serde::{Deserialize, Serialize};

use crate::library::formats::BoundsMm;

/// Where OrcaSlicer's layers start (each layer change).
pub const BODY_START_MARKER: &str = ";LAYER_CHANGE";
/// Custom G-code, which the machine end G-code is written as.
pub const CUSTOM_GCODE_MARKER: &str = ";TYPE:Custom";
/// The end of the executable G-code.
pub const EXECUTABLE_END_MARKER: &str = "; EXECUTABLE_BLOCK_END";

pub const INCH_UNITS: &str = "The G-code uses inch units.";
pub const UNKNOWN_POSITION: &str = "The G-code extrudes from a position farm3d can't track.";
pub const UNKNOWN_EXTRUDER: &str =
    "The G-code extrudes in absolute mode before setting the extruder position.";

/// Which moves the printed bounds cover.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum BoundsScope {
    /// Extruding moves between the print-body markers.
    PrintBody,
    /// Every extruding move: the G-code has no print-body markers.
    AllExtrusions,
}

/// What the scanner found.
#[derive(Clone, Debug, PartialEq)]
pub enum PrintedBounds {
    /// `bounds` is `None` when nothing in `scope` extruded.
    Tracked {
        scope: BoundsScope,
        bounds: Option<BoundsMm>,
    },
    Untracked {
        reason: &'static str,
    },
}

/// Whether D11 check 5 ran, as the invocation manifest records it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BoundsCheck {
    Checked { scope: BoundsScope },
    Skipped { reason: String },
}

#[derive(Default)]
struct Extent {
    bounds: Option<BoundsMm>,
    untracked: Option<&'static str>,
}

impl Extent {
    fn include(&mut self, point: [f64; 3]) {
        match &mut self.bounds {
            Some(bounds) => bounds.include(point),
            None => self.bounds = Some(BoundsMm::point(point)),
        }
    }

    fn untrack(&mut self, reason: &'static str) {
        self.untracked.get_or_insert(reason);
    }
}

/// Streams G-code lines into printed bounds. See the module docs.
pub struct PrintedBoundsScanner {
    relative_xyz: bool,
    /// `M83`. E is also relative while `relative_xyz` is set.
    relative_e_mode: bool,
    position: [Option<f64>; 3],
    extruder: Option<f64>,
    in_body: bool,
    body_seen: bool,
    body: Extent,
    all: Extent,
}

impl Default for PrintedBoundsScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PrintedBoundsScanner {
    pub fn new() -> Self {
        Self {
            relative_xyz: false,
            relative_e_mode: false,
            position: [None; 3],
            extruder: None,
            in_body: false,
            body_seen: false,
            body: Extent::default(),
            all: Extent::default(),
        }
    }

    /// One line, without its line ending.
    pub fn line(&mut self, line: &str) {
        let line = line.trim();
        if line.starts_with(';') {
            self.marker(line);
            return;
        }
        let code = line.split(';').next().unwrap_or_default();
        let mut words = code.split_whitespace();
        let Some(command) = words.next() else {
            return;
        };
        let mut chars = command.chars();
        let letter = chars.next().map(|c| c.to_ascii_uppercase());
        let Ok(number) = chars.as_str().parse::<u32>() else {
            return;
        };
        match (letter, number) {
            (Some('G'), 0..=3) => self.motion(words),
            (Some('G'), 20) => self.untrack_all(INCH_UNITS),
            (Some('G'), 28) => self.home(words),
            (Some('G'), 90) => self.relative_xyz = false,
            (Some('G'), 91) => self.relative_xyz = true,
            (Some('G'), 92) => self.set_position(words),
            (Some('M'), 82) => self.relative_e_mode = false,
            (Some('M'), 83) => self.relative_e_mode = true,
            _ => {}
        }
    }

    pub fn finish(self) -> PrintedBounds {
        let (scope, extent) = if self.body_seen {
            (BoundsScope::PrintBody, self.body)
        } else {
            (BoundsScope::AllExtrusions, self.all)
        };
        match extent.untracked {
            Some(reason) => PrintedBounds::Untracked { reason },
            None => PrintedBounds::Tracked {
                scope,
                bounds: extent.bounds,
            },
        }
    }

    fn marker(&mut self, comment: &str) {
        if comment == BODY_START_MARKER {
            self.in_body = true;
            self.body_seen = true;
        } else if comment.starts_with(CUSTOM_GCODE_MARKER)
            || comment.starts_with(EXECUTABLE_END_MARKER)
        {
            self.in_body = false;
        }
    }

    fn untrack_all(&mut self, reason: &'static str) {
        self.body.untrack(reason);
        self.all.untrack(reason);
    }

    /// Where an extruding move lands in the current scopes.
    fn untrack_in_scope(&mut self, reason: &'static str) {
        if self.in_body {
            self.body.untrack(reason);
        }
        self.all.untrack(reason);
    }

    fn relative_e(&self) -> bool {
        self.relative_xyz || self.relative_e_mode
    }

    fn motion<'a>(&mut self, words: impl Iterator<Item = &'a str>) {
        let mut target = self.position;
        let mut e_word = None;
        for (letter, value) in words.filter_map(word) {
            match letter {
                'X' | 'Y' | 'Z' => {
                    let axis = axis_index(letter);
                    target[axis] = if self.relative_xyz {
                        self.position[axis].map(|position| position + value)
                    } else {
                        Some(value)
                    };
                }
                'E' => e_word = Some(value),
                _ => {}
            }
        }
        let mut extruding = false;
        if let Some(value) = e_word {
            if self.relative_e() {
                extruding = value > 0.0;
                self.extruder = self.extruder.map(|extruder| extruder + value);
            } else {
                match self.extruder {
                    Some(extruder) => extruding = value > extruder,
                    None => self.untrack_in_scope(UNKNOWN_EXTRUDER),
                }
                self.extruder = Some(value);
            }
        }
        if extruding {
            match (known(self.position), known(target)) {
                (Some(start), Some(end)) => {
                    for point in [start, end] {
                        if self.in_body {
                            self.body.include(point);
                        }
                        self.all.include(point);
                    }
                }
                _ => self.untrack_in_scope(UNKNOWN_POSITION),
            }
        }
        self.position = target;
    }

    fn home<'a>(&mut self, words: impl Iterator<Item = &'a str>) {
        let axes: Vec<usize> = words
            .filter_map(|word| word.chars().next())
            .map(|letter| letter.to_ascii_uppercase())
            .filter(|letter| matches!(letter, 'X' | 'Y' | 'Z'))
            .map(axis_index)
            .collect();
        if axes.is_empty() {
            self.position = [None; 3];
        } else {
            for axis in axes {
                self.position[axis] = None;
            }
        }
    }

    fn set_position<'a>(&mut self, words: impl Iterator<Item = &'a str>) {
        let mut any = false;
        for (letter, value) in words.filter_map(word) {
            match letter {
                'X' | 'Y' | 'Z' => self.position[axis_index(letter)] = Some(value),
                'E' => self.extruder = Some(value),
                _ => continue,
            }
            any = true;
        }
        if !any {
            self.position = [Some(0.0); 3];
            self.extruder = Some(0.0);
        }
    }
}

fn axis_index(letter: char) -> usize {
    match letter {
        'X' => 0,
        'Y' => 1,
        _ => 2,
    }
}

/// A `<letter><finite number>` word, the letter uppercased.
fn word(text: &str) -> Option<(char, f64)> {
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let value: f64 = chars.as_str().parse().ok()?;
    value.is_finite().then_some((letter, value))
}

fn known(position: [Option<f64>; 3]) -> Option<[f64; 3]> {
    Some([position[0]?, position[1]?, position[2]?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(gcode: &str) -> PrintedBounds {
        let mut scanner = PrintedBoundsScanner::new();
        for line in gcode.lines() {
            scanner.line(line);
        }
        scanner.finish()
    }

    fn tracked(gcode: &str) -> (BoundsScope, BoundsMm) {
        match scan(gcode) {
            PrintedBounds::Tracked {
                scope,
                bounds: Some(bounds),
            } => (scope, bounds),
            other => panic!("expected tracked bounds, got {other:?}"),
        }
    }

    const START: &str = "\
;TYPE:Custom
G90
M83
G28 W
G1 Z0.2 F720
G1 Y-3 F1000 ; go outside print area
G92 E0
G1 X60 E9 F1000 ; intro line
G1 X100 E12.5 F1000 ; intro line
G92 E0
";

    #[test]
    fn only_extruding_moves_inside_the_print_body_count() {
        let gcode = format!(
            "{START}\
;LAYER_CHANGE
;Z:0.2
G1 E-.8 F1800
G1 X50 Y50 F10800
G1 Z.6
G1 Z.2
G1 E.8 F1800
;TYPE:Outer wall
G1 X60 Y50 E1.0
G1 X60 Y70 E1.0
;WIPE_START
G1 X55 Y70 E-.24
;WIPE_END
G1 X200 Y200 F10800 ; travel
;TYPE:Custom
; filament end gcode
G1 Z11 F720 ; Move print head up
G1 X0 Y200 E5 ; an end-code extrusion, excluded
; EXECUTABLE_BLOCK_END
"
        );
        let (scope, bounds) = tracked(&gcode);

        assert_eq!(scope, BoundsScope::PrintBody);
        assert_eq!(bounds.min, [50.0, 50.0, 0.2]);
        assert_eq!(bounds.max, [60.0, 70.0, 0.2]);
    }

    #[test]
    fn without_markers_every_extruding_move_counts() {
        let (scope, bounds) =
            tracked("G90\nM83\nG1 X0 Y0 Z0.3\nG1 X10 Y-3 E1\nG1 Z5\nG1 X5 Y5 E1\n");

        assert_eq!(scope, BoundsScope::AllExtrusions);
        assert_eq!(bounds.min, [0.0, -3.0, 0.3]);
        assert_eq!(bounds.max, [10.0, 5.0, 5.0]);
    }

    #[test]
    fn absolute_and_relative_modes_and_g92_are_tracked() {
        let gcode = "\
;LAYER_CHANGE
G90
M82
G92 E0
G1 X10 Y10 Z1
G1 X20 Y10 E1 ; absolute E rises: extruding
G1 X30 Y10 E0.5 ; absolute E falls: a retract, not extruding
G92 E0
G1 X40 Y10 E0.2 ; after G92 E0, rising again
G91
G1 X5 Y5 E0.1 ; G91 makes X, Y, Z and E relative
G1 X100 E-1 ; relative retract, not extruding
G90
M83
G1 X20 Y12 E0 ; zero E delta, not extruding
G1 X12 Y12 E0.3 ; relative E under M83
";
        let (_, bounds) = tracked(gcode);

        assert_eq!(bounds.min, [10.0, 10.0, 1.0]);
        assert_eq!(bounds.max, [45.0, 15.0, 1.0]);
    }

    #[test]
    fn untrackable_positioning_skips_the_check() {
        for (gcode, reason) in [
            (";LAYER_CHANGE\nG20\nG1 X1 Y1 Z1 E1\n", INCH_UNITS),
            (";LAYER_CHANGE\nG28\nM83\nG1 X1 Y1 E1\n", UNKNOWN_POSITION),
            (";LAYER_CHANGE\nG91\nG1 X1 Y1 Z1 E1\n", UNKNOWN_POSITION),
            (
                ";LAYER_CHANGE\nG1 X1 Y1 Z1\nM82\nG1 X2 E1\n",
                UNKNOWN_EXTRUDER,
            ),
        ] {
            assert_eq!(scan(gcode), PrintedBounds::Untracked { reason }, "{gcode}");
        }
        // Outside the body, the same G-code doesn't matter.
        let (scope, _) =
            tracked(";TYPE:Custom\nG28\nM83\nG1 X1 Y1 E1\n;LAYER_CHANGE\nG1 X5 Y5 Z1\nG1 X6 E1\n");
        assert_eq!(scope, BoundsScope::PrintBody);
    }

    #[test]
    fn nothing_extruded_has_no_bounds() {
        assert_eq!(
            scan(";LAYER_CHANGE\nG1 X1 Y1 Z1\n"),
            PrintedBounds::Tracked {
                scope: BoundsScope::PrintBody,
                bounds: None,
            }
        );
    }

    #[test]
    fn the_orca_cube_body_stays_on_the_cube() {
        let gcode = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/library/orca-cube.gcode"),
        )
        .unwrap();
        let (scope, bounds) = tracked(&gcode);

        assert_eq!(scope, BoundsScope::PrintBody);
        // The 10 mm cube at (125, 131), with its travel hops excluded.
        assert!(bounds.min[0] > 120.0 && bounds.max[0] < 130.0, "{bounds:?}");
        assert!(bounds.min[1] > 126.0 && bounds.max[1] < 136.0, "{bounds:?}");
        assert!((bounds.min[2] - 0.2).abs() < 1e-9, "{bounds:?}");
        assert!((bounds.max[2] - 10.0).abs() < 1e-9, "{bounds:?}");
    }
}
