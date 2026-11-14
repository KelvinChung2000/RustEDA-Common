// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2026 RustEDA contributors

//! The PDK profile of a run (RCLI-REQ-0029): which PDK and cell library the frontend commands
//! work for, the timing corners and their Liberty groups, and the values the guards of
//! `frontend-physical` compare the physical and routing policy against.
//!
//! A context that pins `files:profile` (`rusteda.native-pdk-profile.v1`) and `files:contract`
//! (`rusteda.native-pdk-contract.v1`) states them. A context without `files:profile` is the
//! published IHP SG13G2/EZ130 recipe, whose values are the constants of [`Profile::ihp`]: its
//! commands read and write exactly what they did before profiles existed.

use std::path::Path;

use super::context::{Failure, Res, at, need, read_json, text};
use crate::estimate::Json;

const PROFILE_SCHEMA: &str = "rusteda.native-pdk-profile.v1";
const CONTRACT_SCHEMA: &str = "rusteda.native-pdk-contract.v1";

/// What `frontend-physical` requires of the physical and routing configuration.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Guards {
    /// The number of entries of `filler_cells`.
    pub(crate) filler_cells: usize,
    /// The required `tap.body_view`.
    pub(crate) body_view: String,
    /// The admitted tap lattices: `(tap.max_gap, tap.pattern)`, an absent pattern being null.
    pub(crate) lattices: Vec<(Json, Json)>,
    /// The required `track_router.min_routing_layer` and `max_routing_layer`.
    pub(crate) routing_layers: (String, String),
}

/// The PDK profile of a run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Profile {
    pub(crate) pdk: String,
    pub(crate) library: String,
    /// Corner names in order; the first is the reference corner of the cell profiles.
    pub(crate) corners: Vec<String>,
    /// Liberty groups in order: `std` (the cell library), then one per macro master in the
    /// order of the profile's `macros`. The Liberty file of a group at a corner is the pinned
    /// file `<group>_<corner>`.
    pub(crate) groups: Vec<String>,
    /// Wire model required for timing reports before placement-based estimation.
    pub(crate) early_wire_model: EarlyWireModel,
    /// The identifier of the sizing family; none: the longest common prefix of its cells.
    pub(crate) family: Option<String>,
    pub(crate) guards: Guards,
}

/// Wire models admitted for pre-placement diagnostic reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EarlyWireModel {
    Ideal,
    Lumped,
}

impl EarlyWireModel {
    pub(crate) fn report_model(self) -> &'static str {
        match self {
            Self::Ideal => "ideal",
            Self::Lumped => "wireload_lumped",
        }
    }

    pub(crate) fn scope(self) -> &'static str {
        match self {
            Self::Ideal => "ideal_diagnostic",
            Self::Lumped => "wireload_diagnostic",
        }
    }
}

/// `[a-z][a-z0-9]{0,7}`: a corner name or a Liberty group.
fn short_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=8).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// `[A-Za-z0-9_.-]{1,64}`: a PDK or library name.
fn pdk_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn staggered(phase: i128, unpaired: Option<&str>) -> Json {
    let mut items = vec![
        ("kind".to_string(), Json::str("staggered")),
        ("phase".to_string(), Json::Int(phase)),
    ];
    if let Some(u) = unpaired {
        items.push(("unpaired".to_string(), Json::str(u)));
    }
    Json::Object(items)
}

impl Profile {
    /// The published IHP SG13G2/EZ130 recipe.
    ///
    /// Its tap lattices are the ones with out-of-flow LU.a/LU.b evidence on its floorplan: the
    /// legacy aligned 15 um, the 28.98 um stagger, and the 42 um stagger with unpaired n-well
    /// edge densification. No in-flow step checks LU.a.
    pub(crate) fn ihp() -> Profile {
        Profile {
            pdk: "ihp-sg13g2".into(),
            library: "ez130_8t".into(),
            corners: vec!["tt".into(), "ss".into()],
            groups: vec!["std".into(), "ram".into()],
            early_wire_model: EarlyWireModel::Ideal,
            family: Some("BUFX".into()),
            guards: Guards {
                filler_cells: 9,
                body_view: "gds".into(),
                lattices: vec![
                    (Json::Int(15000), Json::Null),
                    (Json::Int(28980), staggered(14490, None)),
                    (Json::Int(42000), staggered(21000, Some("cell_top"))),
                ],
                routing_layers: ("Metal2".into(), "TopMetal2".into()),
            },
        }
    }

    /// The profile and contract documents of a profile-bound context.
    pub(crate) fn read(profile: &Path, contract: &Path) -> Res<Profile> {
        Profile::parse(
            &read_json(profile, 2 << 20)?,
            &read_json(contract, 2 << 20)?,
        )
    }

    pub(crate) fn parse(profile: &Json, contract: &Json) -> Res<Profile> {
        need(
            at(profile, "schema")?.as_str() == Some(PROFILE_SCHEMA),
            "PDK profile schema",
        )?;
        let (pdk, library) = (text(profile, "pdk")?, text(profile, "library")?);
        need(
            pdk_name(pdk) && pdk_name(library),
            "PDK profile identity: pdk and library names",
        )?;
        let listed = at(profile, "corners")?
            .as_list()
            .ok_or_else(|| Failure::new("INPUT", "list required: corners"))?;
        need(
            (1..=4).contains(&listed.len()),
            "PDK profile corners: one to four required",
        )?;
        // Liberty groups: the cell library, then one per macro master in the order of `macros`.
        let mut groups: Vec<String> = vec!["std".to_string()];
        if let Some(macros) = profile.get("macros") {
            for item in macros
                .as_list()
                .ok_or_else(|| Failure::new("INPUT", "list required: macros"))?
            {
                let id = text(item, "id")?;
                need(
                    short_name(id) && !groups.iter().any(|g| g == id) && groups.len() <= 8,
                    format!("PDK profile macro identifier: {id}"),
                )?;
                groups.push(id.to_string());
            }
        }
        let mut corners: Vec<String> = Vec::new();
        for corner in listed {
            let name = text(corner, "name")?;
            need(
                short_name(name) && !corners.iter().any(|c| c == name),
                format!("PDK profile corner name: {name}"),
            )?;
            need(
                at(corner, "rc")?.as_str() == Some("nom"),
                format!("PDK profile corner {name}: only the nom extraction corner is supported"),
            )?;
            // One Liberty file per group, in any key order.
            let liberty = at(corner, "liberty")?
                .as_object()
                .ok_or_else(|| Failure::new("INPUT", "object required: liberty"))?;
            need(
                liberty.len() == groups.len()
                    && groups.iter().all(|g| liberty.iter().any(|(k, _)| k == g)),
                format!("PDK profile corner {name}: liberty needs std and every macro identifier"),
            )?;
            corners.push(name.to_string());
        }
        need(
            at(contract, "schema")?.as_str() == Some(CONTRACT_SCHEMA),
            "PDK contract schema",
        )?;
        let physical = at(contract, "physical")?;
        let tap = at(physical, "tap")?;
        let filler_cells = at(physical, "filler_cells")?
            .as_int()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| Failure::new("INPUT", "PDK contract: physical.filler_cells"))?;
        let mut lattices = Vec::new();
        for lattice in at(tap, "lattices")?
            .as_list()
            .ok_or_else(|| Failure::new("INPUT", "list required: lattices"))?
        {
            lattices.push((
                at(lattice, "max_gap")?.clone(),
                lattice.get("pattern").cloned().unwrap_or(Json::Null),
            ));
        }
        need(
            (1..=16).contains(&lattices.len()),
            "PDK contract: one to sixteen tap lattices required",
        )?;
        let routing = at(contract, "routing")?;
        let early_wire_model = match profile.get("early_wire_model") {
            None => EarlyWireModel::Ideal,
            Some(value) if value.as_str() == Some("ideal") => EarlyWireModel::Ideal,
            Some(value) if value.as_str() == Some("wireload_lumped") => EarlyWireModel::Lumped,
            Some(_) => {
                return Err(Failure::new(
                    "INPUT",
                    "invalid PDK profile early_wire_model",
                ));
            }
        };
        Ok(Profile {
            pdk: pdk.to_string(),
            library: library.to_string(),
            corners,
            groups,
            early_wire_model,
            family: None,
            guards: Guards {
                filler_cells,
                body_view: text(tap, "body_view")?.to_string(),
                lattices,
                routing_layers: (
                    text(routing, "min_routing_layer")?.to_string(),
                    text(routing, "max_routing_layer")?.to_string(),
                ),
            },
        })
    }

    /// The pinned Liberty files of the run: `<group>_<corner>`, corners in order, the groups
    /// of each in order.
    pub(crate) fn liberty_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for corner in &self.corners {
            for group in &self.groups {
                keys.push(format!("{group}_{corner}"));
            }
        }
        keys
    }

    /// The corners as a label reads them: `TT/SS`.
    pub(crate) fn corner_label(&self) -> String {
        self.corners
            .iter()
            .map(|c| c.to_uppercase())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The identifier of a sizing family of `cells`: the recipe's own, else the longest common
    /// prefix of the cell names, else `buffers`.
    pub(crate) fn family_id(&self, cells: &[&str]) -> String {
        if let Some(id) = &self.family {
            return id.clone();
        }
        let Some((first, rest)) = cells.split_first() else {
            return "buffers".into();
        };
        let mut end = first.len();
        for name in rest {
            end = first
                .bytes()
                .zip(name.bytes())
                .take(end)
                .take_while(|(a, b)| a == b)
                .count();
        }
        // Cell names are ASCII; a prefix of another alphabet is cut at a character boundary.
        while !first.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            "buffers".into()
        } else {
            first[..end].to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estimate::parse_json;

    fn documents() -> (Json, Json) {
        let profile = parse_json(
            r#"{"schema": "rusteda.native-pdk-profile.v1", "name": "gf180mcu", "pdk": "gf180mcuD",
                "library": "gf180mcu_fd_sc_mcu7t5v0",
                "macros": [{"id": "ram", "master": "sram512x8"}],
                "corners": [
                  {"name": "tt", "rc": "nom", "liberty": {"std": "pdk:a.lib", "ram": "pdk:b.lib"}},
                  {"name": "ss", "rc": "nom", "liberty": {"ram": "pdk:d.lib", "std": "pdk:c.lib"}}]}"#,
        )
        .unwrap();
        let contract = parse_json(
            r#"{"schema": "rusteda.native-pdk-contract.v1",
                "physical": {"filler_cells": 12, "tap": {"body_view": "lef",
                  "lattices": [{"max_gap": 28000}, {"max_gap": 30000, "pattern": {"kind": "staggered", "phase": 15000}}]}},
                "routing": {"min_routing_layer": "Metal2", "max_routing_layer": "Metal5"}}"#,
        )
        .unwrap();
        (profile, contract)
    }

    /// RCLI-REQ-0029: a profile and its contract give the identity, corners, groups and guards.
    #[test]
    fn a_profile_states_identity_corners_and_guards() {
        let (profile, contract) = documents();
        let p = Profile::parse(&profile, &contract).unwrap();
        assert_eq!(p.pdk, "gf180mcuD");
        assert_eq!(p.library, "gf180mcu_fd_sc_mcu7t5v0");
        assert_eq!(p.corners, ["tt", "ss"]);
        assert_eq!(p.groups, ["std", "ram"]);
        assert_eq!(p.early_wire_model, EarlyWireModel::Ideal);
        assert_eq!(p.liberty_keys(), ["std_tt", "ram_tt", "std_ss", "ram_ss"]);
        assert_eq!(p.corner_label(), "TT/SS");
        assert_eq!(p.guards.filler_cells, 12);
        assert_eq!(p.guards.body_view, "lef");
        assert_eq!(p.guards.lattices[0], (Json::Int(28000), Json::Null));
        assert_eq!(p.guards.lattices[1].1, staggered(15000, None));
        assert_eq!(
            p.guards.routing_layers,
            ("Metal2".to_string(), "Metal5".to_string())
        );
        assert_eq!(
            p.family_id(&[
                "gf180mcu_fd_sc_mcu7t5v0__buf_1",
                "gf180mcu_fd_sc_mcu7t5v0__buf_16"
            ]),
            "gf180mcu_fd_sc_mcu7t5v0__buf_1"
        );
        assert_eq!(p.family_id(&["buf_2", "clkbuf_2"]), "buffers");
        assert_eq!(p.family_id(&[]), "buffers");
    }

    #[test]
    fn a_profile_can_require_lumped_preplacement_reports() {
        let (mut profile, contract) = documents();
        profile.set("early_wire_model", Json::str("wireload_lumped"));
        let parsed = Profile::parse(&profile, &contract).unwrap();
        assert_eq!(parsed.early_wire_model.report_model(), "wireload_lumped");
        assert_eq!(parsed.early_wire_model.scope(), "wireload_diagnostic");
        profile.set("early_wire_model", Json::str("spef_rc"));
        assert!(Profile::parse(&profile, &contract).is_err());
    }

    /// RCLI-REQ-0029: the recipe without a profile is the IHP one, with its literal values.
    #[test]
    fn the_recipe_without_a_profile_is_the_ihp_one() {
        let p = Profile::ihp();
        assert_eq!(
            (p.pdk.as_str(), p.library.as_str()),
            ("ihp-sg13g2", "ez130_8t")
        );
        assert_eq!(p.liberty_keys(), ["std_tt", "ram_tt", "std_ss", "ram_ss"]);
        assert_eq!(p.corner_label(), "TT/SS");
        assert_eq!(p.family_id(&["BUFX1", "BUFX16"]), "BUFX");
        assert_eq!(p.guards.filler_cells, 9);
        assert_eq!(p.guards.lattices.len(), 3);
        // The family identifier of the IHP buffers is also what the generic rule derives.
        let generic = Profile {
            family: None,
            ..Profile::ihp()
        };
        let buffers = [
            "BUFX1", "BUFX2", "BUFX3", "BUFX4", "BUFX6", "BUFX8", "BUFX12", "BUFX16", "BUFX24",
            "BUFX32", "BUFX44", "BUFX48", "BUFX64",
        ];
        assert_eq!(generic.family_id(&buffers), "BUFX");
    }

    /// RCLI-REQ-0029: malformed profiles and contracts are refused.
    #[test]
    fn malformed_profiles_are_refused() {
        let (profile, contract) = documents();
        let change = |doc: &Json, path: &[&str], value: Json| -> Json {
            fn set(node: &mut Json, path: &[&str], value: Json) {
                match (node, path) {
                    (Json::Object(items), [key]) => {
                        match items.iter_mut().find(|(k, _)| k == key) {
                            Some(slot) => slot.1 = value,
                            None => items.push((key.to_string(), value)),
                        }
                    }
                    (Json::Object(items), [key, rest @ ..]) => {
                        if let Some(slot) = items.iter_mut().find(|(k, _)| k == key) {
                            set(&mut slot.1, rest, value);
                        }
                    }
                    (Json::List(items), [index, rest @ ..]) => {
                        let k: usize = index.parse().unwrap();
                        if rest.is_empty() {
                            items[k] = value;
                        } else {
                            set(&mut items[k], rest, value);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = doc.clone();
            set(&mut out, path, value);
            out
        };
        let bad_profiles = [
            change(&profile, &["schema"], Json::str("other")),
            change(&profile, &["pdk"], Json::str("gf 180")),
            change(&profile, &["library"], Json::str("")),
            change(&profile, &["corners"], Json::List(Vec::new())),
            change(&profile, &["corners", "1", "name"], Json::str("tt")),
            change(&profile, &["corners", "1", "name"], Json::str("SS")),
            change(&profile, &["corners", "0", "rc"], Json::str("max")),
            change(
                &profile,
                &["corners", "1", "liberty"],
                parse_json(r#"{"std": "x"}"#).unwrap(),
            ),
            change(
                &profile,
                &["corners", "0", "liberty"],
                parse_json(r#"{"rom": "x", "std": "y"}"#).unwrap(),
            ),
            change(
                &profile,
                &["macros"],
                parse_json(r#"[{"id": "ram"}, {"id": "ram"}]"#).unwrap(),
            ),
            change(
                &profile,
                &["macros"],
                parse_json(r#"[{"id": "std"}]"#).unwrap(),
            ),
        ];
        for (k, doc) in bad_profiles.iter().enumerate() {
            assert!(Profile::parse(doc, &contract).is_err(), "profile case {k}");
        }
        let bad_contracts = [
            change(&contract, &["schema"], Json::str("other")),
            change(&contract, &["physical", "filler_cells"], Json::Int(-1)),
            change(&contract, &["physical", "filler_cells"], Json::str("9")),
            change(
                &contract,
                &["physical", "tap", "lattices"],
                Json::List(Vec::new()),
            ),
            change(&contract, &["physical", "tap", "body_view"], Json::Int(1)),
            change(&contract, &["routing", "max_routing_layer"], Json::Null),
        ];
        for (k, doc) in bad_contracts.iter().enumerate() {
            assert!(Profile::parse(&profile, doc).is_err(), "contract case {k}");
        }
    }
}
