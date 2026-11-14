// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2026 RustEDA contributors

//! Memory interface timing retains ordinary NLDM constraints and reports waveform scope.

use rusteda_formats::liberty::read_text_diags;

fn memory(bus: bool, retained: &str) -> String {
    let input = r#"direction: input; max_transition: 0.351;
        timing() { related_pin: CLK; timing_type: setup_rising; sdf_edges: both_edges;
            rise_constraint(scalar) { values(0.2); }
            fall_constraint(scalar) { values(0.3); }
        }"#;
    let output = format!(
        r#"direction: output; max_capacitance: 0.52;
        timing() {{ related_pin: CLK; timing_type: rising_edge;
            cell_rise(scalar) {{ values(1.2); }}
            cell_fall(scalar) {{ values(1.3); }}
            {retained}
        }}"#
    );
    let pins = if bus {
        format!(
            "bus(D) {{ bus_type:T; pin(D[1:0]) {{ {input} }} }}\n\
             bus(Q) {{ bus_type:T; pin(Q[1:0]) {{ {output} }} }}"
        )
    } else {
        format!("pin(D) {{ {input} }} pin(Q) {{ {output} }}")
    };
    format!(
        "library(M) {{ type(T) {{ bit_from:1; bit_to:0; bit_width:2; }}\n\
         cell(RAM) {{ memory() {{ type:ram; address_width:1; word_width:2; }}\n\
         pin(CLK) {{ direction:input; clock:true; }} {pins} }} }}"
    )
}

#[test]
fn memory_bus_and_scalar_keep_sdf_edges_without_losing_pins() {
    for bus in [false, true] {
        let source = memory(bus, "");
        let (lib, diags) =
            read_text_diags(&source, "sdf_edges.lib").expect("valid memory interface");
        let cell = &lib.cells[0];
        assert!(cell.problems.is_empty(), "bus={bus}: {diags:#?}");
        let pin = cell.pin(if bus { "D[0]" } else { "D" }).expect("data pin");
        assert_eq!(lib.pin_max_transition(pin), Some(0.351));
        assert_eq!(pin.arcs[0].rise_constraint.as_ref().unwrap().values, [0.2]);
        assert_eq!(pin.arcs[0].fall_constraint.as_ref().unwrap().values, [0.3]);
        assert!(
            diags.iter().any(|diag| diag.code == "L0114"),
            "SDF scope is explicit"
        );
    }
}

#[test]
fn retaining_tables_are_validated_with_explicit_scope_and_ordinary_delays_intact() {
    let retained = "retaining_rise(scalar) { values(0.1); }\n\
                    retaining_fall(scalar) { values(0.15); }\n\
                    retain_rise_slew(scalar) { values(0.02); }\n\
                    retain_fall_slew(scalar) { values(0.03); }";
    for bus in [false, true] {
        let source = memory(bus, retained);
        let (lib, diags) = read_text_diags(&source, "retain.lib").expect("valid retention tables");
        let cell = &lib.cells[0];
        assert!(cell.problems.is_empty(), "bus={bus}: {diags:#?}");
        let pin = cell
            .pin(if bus { "Q[0]" } else { "Q" })
            .expect("output pin");
        assert_eq!(lib.pin_max_capacitance(pin), Some(0.52));
        assert_eq!(pin.arcs[0].cell_rise.as_ref().unwrap().values, [1.2]);
        assert_eq!(pin.arcs[0].cell_fall.as_ref().unwrap().values, [1.3]);
        assert!(
            diags
                .iter()
                .any(|diag| diag.code == "L0114" && diag.message.contains("retention"))
        );
    }
}

#[test]
fn malformed_retaining_table_keeps_the_cell_unusable() {
    for bus in [false, true] {
        let source = memory(bus, "retaining_rise(missing_template) { values(0.1); }");
        let (lib, diags) = read_text_diags(&source, "bad-retain.lib").expect("syntax is valid");
        assert!(!lib.cells[0].problems.is_empty(), "bus={bus}");
        assert!(
            diags.iter().any(|diag| diag.code == "L0205"),
            "bus={bus}: {diags:#?}"
        );
    }
}

#[test]
fn unsupported_retention_waveform_attribute_is_not_silently_accepted() {
    for bus in [false, true] {
        let source = memory(
            bus,
            "retaining_rise(scalar) { values(0.1); future_option:1; }",
        );
        let (_, diags) = read_text_diags(&source, "future-retain.lib").expect("syntax is valid");
        assert!(
            diags
                .iter()
                .any(|diag| matches!(diag.code.as_str(), "L0101" | "L0208"))
        );
    }
}
