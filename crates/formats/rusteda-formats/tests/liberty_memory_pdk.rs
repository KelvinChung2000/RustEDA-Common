// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2026 RustEDA contributors

//! Explicit real-PDK check; absent pinned input is an error, never a verification pass.

use std::path::PathBuf;

use rusteda_formats::liberty::{Direction, read_file_diags};

fn required_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing required {name}")))
}

#[test]
#[ignore = "requires RUSTEDA_TEST_SRAM22_DIR, RUSTEDA_TEST_SKY130_LIB and RUSTEDA_TEST_GF180_LIB"]
fn original_sram22_corners_and_both_standard_cell_pdks() {
    let directory = required_path("RUSTEDA_TEST_SRAM22_DIR");
    for corner in ["tt_025C_1v80", "ss_100C_1v60", "ff_n40C_1v95"] {
        let file = directory.join(format!("sram22_512x32m4w8_{corner}.lib"));
        let (library, diagnostics) = read_file_diags(&file).expect("original macro library");
        assert_eq!(library.cells.len(), 1, "{corner}");
        let cell = &library.cells[0];
        assert!(cell.problems.is_empty(), "{corner}: {diagnostics:#?}");
        assert!(cell.is_macro_cell, "memory contents must remain opaque");
        assert_eq!(
            cell.pins.len(),
            81,
            "{corner}: every scalar and bus pin survives"
        );
        assert_eq!(cell.pg_pin_groups.len(), 2, "{corner}: supply pins survive");
        let mut arcs = 0;
        for pin in &cell.pins {
            arcs += pin.arcs.len();
            match pin.direction {
                Direction::Input => assert_eq!(library.pin_max_transition(pin), Some(0.351)),
                Direction::Output => {
                    assert_eq!(library.pin_max_capacitance(pin), Some(0.52));
                    assert_eq!(pin.arcs.len(), 1, "{corner}: {}", pin.name);
                    assert_eq!(pin.arcs[0].related_pin, "clk");
                    assert!(pin.arcs[0].cell_rise.is_some() && pin.arcs[0].cell_fall.is_some());
                }
                _ => panic!("unexpected signal direction: {}", pin.name),
            }
        }
        assert_eq!(
            arcs, 130,
            "{corner}: setup, hold, pulse/period and clock-to-output arcs"
        );
        for (name, width) in [("addr", 9), ("din", 32), ("wmask", 4)] {
            for bit in 0..width {
                let pin = cell.pin(&format!("{name}[{bit}]")).expect("bus member");
                assert_eq!(pin.arcs.len(), 2, "{corner}: {}", pin.name);
                assert!(
                    pin.arcs
                        .iter()
                        .all(|arc| arc.rise_constraint.is_some() && arc.fall_constraint.is_some())
                );
            }
        }
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code == "L0114" && diag.message.contains("retention"))
        );
        assert!(diagnostics.iter().all(|diag| !diag.code.starts_with("L02")));
    }
    // Drop each immutable parsed library before loading the next. There is no per-test
    // multi-megabyte clone or parallel library reparse.
    for variable in ["RUSTEDA_TEST_SKY130_LIB", "RUSTEDA_TEST_GF180_LIB"] {
        let file = required_path(variable);
        let (library, diagnostics) = read_file_diags(&file).expect("standard-cell library");
        assert!(
            library.cells.len() > 100,
            "{variable}: real library required"
        );
        assert!(diagnostics.is_empty(), "{variable}: {diagnostics:#?}");
        assert!(
            library.cells.iter().all(|cell| cell.problems.is_empty()),
            "{variable}"
        );
    }
}
