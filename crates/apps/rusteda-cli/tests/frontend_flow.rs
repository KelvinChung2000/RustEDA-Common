// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2026 RustEDA contributors

//! The native frontend commands (RCLI-REQ-0027, RCLI-REQ-0028) against the reference helpers of
//! the RustEDA Ibex recipe (`native_frontend_inputs.py`, `native_ecos.py`), byte for byte, on a
//! small frontend run: `frontend-profiles`, `frontend-physical`, `frontend-pins`, the `fanout`
//! round (ideal wires) and the `hold` round (estimated RC) of `eco-round --plan-only`, and
//! `frontend-validate`.
//!
//! `fixtures/flow/reference.py` wrote the fixture and, with the reference helpers, the expected
//! files. Paths and the digests of files that hold paths are placeholders there (see its
//! `normalize`); the receipt's run time, peak memory and planner hash are not compared. The ECO
//! rounds check their reports against the pinned `xxhsum`: they run when `RUSTEDA_TEST_XXHSUM`
//! names it or `xxhsum` is on `PATH`, and are skipped otherwise.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A run of the fixture: its directories and the pinned hash tool.
struct Run {
    fix: PathBuf,
    base: PathBuf,
    root: PathBuf,
    xxhsum: PathBuf,
    /// Whether `xxhsum` is the real tool (the ECO rounds need it).
    hashed: bool,
}

const FILES: [(&str, &str); 18] = [
    ("sdc", "pdk/constraints.sdc"),
    ("std_tt", "pdk/std_tt.lib"),
    ("std_ss", "pdk/std_ss.lib"),
    ("ram_tt", "pdk/ram_tt.lib"),
    ("ram_ss", "pdk/ram_ss.lib"),
    ("cell_lef", "pdk/cells.lef"),
    ("std_lef", "pdk/cells.lef"),
    ("floorplan", "recipe/floorplan.json"),
    ("physical", "recipe/physical.json"),
    ("routing", "recipe/routing.json"),
    ("interfaces", "recipe/interfaces.json"),
    ("ecos", "recipe/ecos.py"),
    ("prepare", "recipe/prepare.py"),
    ("delay_helper", "recipe/delay_helper.py"),
    ("sizing_helper", "recipe/sizing_helper.py"),
    ("liberty_reader", "recipe/liberty_lite.py"),
    ("estimator", "recipe/estimator.py"),
    ("rc_policy", "recipe/rc-policy.json"),
];

fn find_xxhsum() -> Option<PathBuf> {
    if let Some(p) = env::var_os("RUSTEDA_TEST_XXHSUM") {
        return fs::canonicalize(p).ok();
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join("xxhsum"))
        .find(|p| p.is_file())
        .and_then(|p| fs::canonicalize(p).ok())
}

/// A file record as the composer of a run writes it: `path`, `sha256`, `bytes`, in this order.
fn record(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    format!(
        "{{\"path\": {}, \"sha256\": \"{}\", \"bytes\": {}}}",
        Value::from(path.to_str().unwrap()),
        sha256(&bytes),
        bytes.len()
    )
}

fn copy_tree(from: &Path, to: &Path, run: &str, fix: &str) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target, run, fix);
        } else {
            let text = fs::read_to_string(entry.path()).unwrap();
            fs::write(target, text.replace("@RUN@", run).replace("@FIX@", fix)).unwrap();
        }
    }
}

impl Run {
    /// Copies the run tree of the fixture to a fresh directory and writes its context.
    fn new(label: &str) -> Run {
        let fix =
            fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flow"))
                .unwrap();
        let tmp = fs::canonicalize(env::temp_dir()).unwrap();
        let base = tmp.join(format!("rusteda-flow-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("run");
        copy_tree(
            &fix.join("run"),
            &root,
            root.to_str().unwrap(),
            fix.to_str().unwrap(),
        );
        // The raw Liberty copies of the two STA workspaces.
        for tag in ["ideal", "est"] {
            let cache = root.join(format!("frontend/sta-{tag}/cache"));
            fs::create_dir_all(&cache).unwrap();
            for co in ["tt", "ss"] {
                for kind in ["std", "ram"] {
                    fs::copy(
                        fix.join(format!("pdk/{kind}_{co}.lib")),
                        cache.join(format!("{kind}_{co}-0000000000000000.lib")),
                    )
                    .unwrap();
                }
            }
        }
        let (xxhsum, hashed) = match find_xxhsum() {
            Some(tool) => (tool, true),
            None => {
                // A stand-in: the helpers only pin the tool, the ECO rounds are skipped.
                let stand_in = base.join("xxhsum");
                fs::write(&stand_in, "stand-in of the pinned xxhsum\n").unwrap();
                (stand_in, false)
            }
        };
        // The context in the key order of the composer (the helpers copy its records).
        let files: Vec<String> = FILES
            .iter()
            .map(|(k, rel)| format!("\"{k}\": {}", record(&fix.join(rel))))
            .collect();
        let context = format!(
            "{{\"schema\": \"rusteda.native-frontend-context.v1\", \"run_root\": {}, \"files\": {{{}}}, \
             \"tools\": {{\"rustroad\": {}, \"xxhsum\": {}}}, \"selection\": {}}}\n",
            Value::from(root.to_str().unwrap()),
            files.join(", "),
            record(&fix.join("recipe/rustroad")),
            record(&xxhsum),
            fs::read_to_string(fix.join("recipe/selection.json")).unwrap()
        );
        fs::write(root.join("frontend/context.json"), context).unwrap();
        Run {
            fix,
            base,
            root,
            xxhsum,
            hashed,
        }
    }

    fn rusteda(&self, command: &str, workspace: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rusteda"))
            .arg(command)
            .arg("--context")
            .arg(self.root.join("frontend/context.json"))
            .arg("--workspace")
            .arg(self.root.join(workspace))
            .args(["--base-rev", "empty", "--threads", "2"])
            .args(extra)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, command: &str, workspace: &str, extra: &[&str]) -> Output {
        let out = self.rusteda(command, workspace, extra);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{command}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn path(&self, rel: &str) -> String {
        self.root.join(rel).to_str().unwrap().to_string()
    }

    /// The files whose bytes hold a path, by the label of their placeholders.
    fn volatile(&self) -> Vec<(String, PathBuf)> {
        let mut out = vec![
            ("xxhsum".to_string(), self.xxhsum.clone()),
            (
                "roots".into(),
                self.root.join("frontend/bundle-reset/roots.json"),
            ),
            (
                "routing".into(),
                self.root.join("frontend/physical/routing.json"),
            ),
            (
                "pins-receipt".into(),
                self.root.join("frontend/physical-pins/receipt.json"),
            ),
            ("cache".into(), self.root.join("frontend/sha256-cache.json")),
        ];
        for tag in ["ideal", "est"] {
            out.push((
                format!("{tag}-meta"),
                self.root
                    .join(format!("frontend/sta-{tag}/revisions/rev0002/meta.json")),
            ));
        }
        for tag in ["ideal", "est", "final"] {
            for co in ["tt", "ss"] {
                out.push((
                    format!("{tag}-drv-{co}"),
                    self.root.join(format!("logs/fe_{tag}_{co}_drv.stdout")),
                ));
            }
        }
        for co in ["tt", "ss"] {
            out.push((
                format!("est-endpoints-{co}"),
                self.root.join(format!("logs/fe_est_{co}_endpoints.stdout")),
            ));
        }
        out.push(("est-hold".into(), self.root.join("logs/fe_est_hold.stdout")));
        for op in ["fanout", "hold"] {
            out.push((
                format!("{op}-inputs"),
                self.root.join(format!("frontend/bundle-{op}/inputs.json")),
            ));
        }
        out
    }

    /// `normalize` of `reference.py`: digests of the volatile files, paths, and the receipt's
    /// run-dependent fields as placeholders.
    fn normalize(&self, mut text: String) -> String {
        for (label, path) in self.volatile() {
            let Ok(bytes) = fs::read(&path) else { continue };
            text = text.replace(&sha256(&bytes), &format!("@SHA:{label}@"));
            let key = format!("\"sha256\": \"@SHA:{label}@\",\n");
            let size = bytes.len().to_string();
            let mut out = String::with_capacity(text.len());
            let mut rest = text.as_str();
            while let Some(k) = rest.find(&key) {
                let (head, tail) = rest.split_at(k + key.len());
                out.push_str(head);
                let indent = tail.len() - tail.trim_start().len();
                let field = &tail[indent..];
                let number = field.strip_prefix("\"bytes\": ").and_then(|v| {
                    let digits = v.len() - v.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                    (v[..digits] == size).then_some(digits)
                });
                match number {
                    Some(digits) => {
                        out.push_str(&tail[..indent]);
                        out.push_str(&format!("\"bytes\": \"@BYTES:{label}@\""));
                        rest = &field["\"bytes\": ".len() + digits..];
                    }
                    None => rest = tail,
                }
            }
            out.push_str(rest);
            text = out;
        }
        text = text
            .replace(self.root.to_str().unwrap(), "@RUN@")
            .replace(self.fix.to_str().unwrap(), "@FIX@")
            .replace(self.xxhsum.to_str().unwrap(), "@XXHSUM@");
        for (key, mark) in [
            ("elapsed_seconds", "@ELAPSED@"),
            ("peak_rss_kib", "@PEAK@"),
            ("planner_sha256", "@PLANNER@"),
        ] {
            let start = format!("\n  \"{key}\": ");
            if let Some(k) = text.find(&start) {
                let from = k + start.len();
                let len = text[from..].find([',', '\n']).unwrap();
                text.replace_range(from..from + len, &format!("\"{mark}\""));
            }
        }
        text
    }

    /// Requires that the workspace holds exactly the expected files of the case, byte for byte
    /// after `normalize`.
    fn same(&self, case: &str, workspace: &str) {
        let expected = self.fix.join("expected").join(case);
        let actual = self.root.join(workspace);
        let names = |dir: &Path| {
            let mut names: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| n != "workspace.lock")
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(&actual), names(&expected), "files of {case}");
        for name in names(&expected) {
            if name == "digest-check.json" {
                continue;
            }
            let text = self.normalize(fs::read_to_string(actual.join(&name)).unwrap());
            assert_eq!(
                text,
                fs::read_to_string(expected.join(&name)).unwrap(),
                "{case}/{name}"
            );
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn round(run: &Run, op: &str, tag: &str) {
    let (bundle, plan) = (
        format!("frontend/bundle-{op}"),
        format!("frontend/plan-{op}"),
    );
    let out = run.ok(
        "eco-round",
        &plan,
        &[
            "--operation",
            op,
            "--timing-tag",
            tag,
            "--source",
            &run.path("frontend/mini.v"),
            "--graph",
            &run.path("frontend/mini-graph.json"),
            "--timed",
            &run.path(&format!("logs/fe_{tag}_export.stdout")),
            "--bundle",
            &run.path(&bundle),
            "--plan-only",
        ],
    );
    run.same(&format!("bundle-{op}"), &bundle);
    run.same(&format!("plan-{op}"), &plan);
    // The summary line is the receipt's.
    let summary: Value = serde_json::from_slice(&out.stdout).unwrap();
    let receipt: Value = serde_json::from_str(
        &fs::read_to_string(run.root.join(&plan).join("receipt.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(summary["status"], "planned_unverified");
    assert_eq!(summary["netlist_sha256"], Value::Null);
    for key in ["schema", "rev", "scope", "operations", "plan_sha256"] {
        assert_eq!(summary[key], receipt[key], "{key}");
    }
    assert_eq!(
        summary["plan_sha256"],
        sha256(&fs::read(run.root.join(&plan).join("plan.json")).unwrap())
    );
}

#[test]
fn the_commands_write_the_reference_helpers_files_byte_for_byte() {
    let run = Run::new("reference");
    run.ok("frontend-profiles", "frontend/profiles-out", &[]);
    run.same("profiles", "frontend/profiles-out");
    run.ok(
        "frontend-physical",
        "frontend/physical",
        &["--roots", &run.path("frontend/bundle-reset/roots.json")],
    );
    run.same("physical", "frontend/physical");
    let pins = run.ok(
        "frontend-pins",
        "frontend/physical-pins",
        &["--def", &run.path("logs/fe_pins_def.stdout")],
    );
    assert!(pins.stdout.is_empty());
    run.same("physical-pins", "frontend/physical-pins");
    if run.hashed {
        round(&run, "fanout", "ideal");
        round(&run, "hold", "est");
    } else {
        eprintln!("xxhsum not found (RUSTEDA_TEST_XXHSUM, PATH): the ECO rounds are skipped");
    }
    run.ok("frontend-validate", "frontend/result", &[]);
    run.same("result", "frontend/result");
    // The final digest check lists the pinned inputs whose digests the run shared through its
    // flow digest cache, each hashed again in full. Which inputs are listed depends on the file
    // system and on the age of the files (only settled files of local file systems are), so
    // the list is checked against the files themselves.
    let check: Value = serde_json::from_str(
        &fs::read_to_string(run.root.join("frontend/result/digest-check.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(check["schema"], "rusteda.frontend-digest-check.v1");
    for (path, digest) in check["checked"].as_object().unwrap() {
        assert_eq!(digest, &sha256(&fs::read(path).unwrap()), "{path}");
    }
    if let Some(cache) = check["cache"].as_object() {
        let bytes = fs::read(cache["path"].as_str().unwrap()).unwrap();
        assert_eq!(cache["sha256"], sha256(&bytes));
        assert_eq!(cache["bytes"], bytes.len());
    }
}

#[test]
fn electrical_buffers_a_violating_macro_pin_on_a_fanout_tree_once() {
    // The existing synthetic run plus one macro input: 24 logical cells, two
    // corners, fewer than 256 graph rows and <64 MiB including parsed reports.
    // Both corners report the same pin; it needs one pin buffer while its
    // fanout-tree level must still receive the usual aggregate sizing.
    let run = Run::new("macro-fanout");
    assert!(
        run.hashed,
        "xxhsum is required for the macro fanout ECO control"
    );
    let leaf = "native_fanout_900_0_0";
    for name in ["frontend/mini.v", "logs/fe_est_export.stdout"] {
        let path = run.root.join(name);
        let text = fs::read_to_string(&path).unwrap().replace("u_o", leaf);
        fs::write(
            path,
            text.replace("endmodule", " RAM8 u_ram (.A_DIN(q_o));\nendmodule"),
        )
        .unwrap();
    }
    let read = |name: &str| -> Value {
        serde_json::from_slice(&fs::read(run.root.join(name)).unwrap()).unwrap()
    };
    let write = |name: &str, value: &Value| {
        fs::write(
            run.root.join(name),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    };
    let mut graph = read("frontend/mini-graph.json");
    let cells = graph["modules"]["mini"]["cells"].as_object_mut().unwrap();
    let driver = cells.remove("u_o").unwrap();
    cells.insert(leaf.into(), driver);
    cells.insert(
        "u_ram".into(),
        json!({"type":"RAM8", "port_directions":{"A_DIN":"input"},
               "connections":{"A_DIN":[6]}}),
    );
    graph["modules"]["RAM8"] = json!({
        "attributes":{"blackbox":"1"},
        "ports":{"A_DIN":{"direction":"input", "bits":[2]}},
        "cells":{}, "netnames":{}
    });
    write("frontend/mini-graph.json", &graph);
    let def_path = run.root.join("logs/fe_est_def.stdout");
    let def = fs::read_to_string(&def_path).unwrap().replace("u_o", leaf);
    fs::write(&def_path, &def).unwrap();
    let mut estimate = read("frontend/estimate-est/result.json");
    estimate["inputs"]["def"]["sha256"] = json!(sha256(def.as_bytes()));
    write("frontend/estimate-est/result.json", &estimate);
    let mut seed = read("logs/fe_est_seed.stdout");
    let cells = seed["cells"].as_object_mut().unwrap();
    let at = cells.remove("u_o").unwrap();
    cells.insert(leaf.into(), at);
    cells.insert("u_ram".into(), json!([120000, 60000]));
    seed["source"]["imported_netlist_sha256"] =
        json!(sha256(&fs::read(run.root.join("frontend/mini.v")).unwrap()));
    write("logs/fe_est_seed.stdout", &seed);
    // Rebind the synthetic compiled-report source after changing the fixture.
    let hashed = Command::new(&run.xxhsum)
        .arg("-H3")
        .arg(run.root.join("logs/fe_est_export.stdout"))
        .output()
        .unwrap();
    assert!(hashed.status.success(), "fixture netlist hash failed");
    let text = String::from_utf8(hashed.stdout).unwrap();
    let digest = text
        .split_whitespace()
        .next()
        .unwrap()
        .strip_prefix("XXH3_")
        .unwrap();
    let mut meta = read("frontend/sta-est/revisions/rev0002/meta.json");
    for source in meta["sources"].as_array_mut().unwrap() {
        if source["table"] == "netlist" {
            source["hash"] = json!(format!("xxh3:{digest}"));
        }
    }
    write("frontend/sta-est/revisions/rev0002/meta.json", &meta);
    for corner in ["tt", "ss"] {
        let name = format!("logs/fe_est_{corner}_drv.stdout");
        let mut report = read(&name);
        report["metrics"]["max_capacitance_violating"] = json!(0);
        report["metrics"]["max_transition_violating"] = json!(1);
        let section = &mut report["sections"]["drv"];
        for key in ["total", "matched", "shown"] {
            section[key] = json!(1);
        }
        section["rows"] = json!([{"pin":"u_ram/A_DIN", "handle":"pin900",
            "corner":corner, "kind":"max_transition", "net":"q_o",
            "value":2.0, "limit":1.0, "slack":-1.0, "unit":"ns"}]);
        write(&name, &report);
    }
    // This control isolates signal-pin repair from clock balancing.
    let mut context = read("frontend/context.json");
    context["selection"]["electrical"]
        .as_object_mut()
        .unwrap()
        .remove("clock_balance");
    write("frontend/context.json", &context);
    run.ok(
        "eco-round",
        "frontend/plan-macro-fanout",
        &[
            "--operation",
            "electrical",
            "--timing-tag",
            "est",
            "--source",
            &run.path("frontend/mini.v"),
            "--graph",
            &run.path("frontend/mini-graph.json"),
            "--timed",
            &run.path("logs/fe_est_export.stdout"),
            "--bundle",
            &run.path("frontend/bundle-macro-fanout"),
            "--plan-only",
        ],
    );
    let plan = read("frontend/plan-macro-fanout/plan.json");
    let pin_buffers: Vec<_> = plan["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| {
            op["op"] == "buffer_sinks"
                && op["sinks"].as_array().is_some_and(|sinks| {
                    sinks
                        .iter()
                        .any(|sink| sink["instance"] == "u_ram" && sink["pin"] == "A_DIN")
                })
        })
        .collect();
    assert_eq!(
        pin_buffers.len(),
        1,
        "both corner violations need one macro-pin buffer: {plan}"
    );
    assert_eq!(pin_buffers[0]["cell"], "BUFX4");
    let receipt = read("frontend/plan-macro-fanout/receipt.json");
    assert!(
        receipt["checked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["action"] == "resize_level"),
        "macro pin repair must preserve aggregate tree sizing: {receipt}"
    );
}

#[test]
fn a_used_workspace_and_a_changed_pinned_input_are_refused() {
    let run = Run::new("refused");
    run.ok("frontend-profiles", "frontend/profiles-out", &[]);
    // A second helper into the same, no longer fresh, workspace.
    let again = run.rusteda("frontend-profiles", "frontend/profiles-out", &[]);
    assert_eq!(again.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&again.stderr).unwrap();
    assert_eq!(error["status"], "failed");
    assert_eq!(error["code"], "REVISION_CONFLICT");
    // An unknown round is refused before anything is written.
    let unknown = run.rusteda(
        "eco-round",
        "frontend/plan-x",
        &[
            "--operation",
            "resize",
            "--timing-tag",
            "ideal",
            "--source",
            &run.path("frontend/mini.v"),
            "--graph",
            &run.path("frontend/mini-graph.json"),
            "--bundle",
            &run.path("frontend/bundle-x"),
        ],
    );
    assert_eq!(unknown.status.code(), Some(2));
    assert!(!run.root.join("frontend/bundle-x").exists());
    // A context whose record no longer matches the pinned file.
    let context = run.root.join("frontend/context.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&context).unwrap()).unwrap();
    doc["files"]["sdc"]["sha256"] = json!("0".repeat(64));
    fs::write(&context, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    let changed = run.rusteda("frontend-profiles", "frontend/profiles-2", &[]);
    assert_eq!(changed.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&changed.stderr).unwrap();
    assert_eq!(error["error"], "pinned input changed: sdc");
    assert!(!run.root.join("frontend/profiles-2").exists());
}

/// RCLI-REQ-0029: a context that pins a PDK profile and its contract states the PDK, the
/// corners, the database unit and the guard values; the same inputs without a profile are the
/// IHP recipe.
#[test]
fn a_profile_bound_context_states_its_pdk() {
    let run = Run::new("profile");
    let write = |name: &str, text: String| -> PathBuf {
        let path = run.base.join(name);
        fs::write(&path, text).unwrap();
        path
    };
    let profile = |corners: [&str; 2]| {
        json!({"schema": "rusteda.native-pdk-profile.v1", "name": "gf180mcu", "pdk": "gf180mcuD",
            "library": "gf180mcu_fd_sc_mcu7t5v0",
            "macros": [{"id": "ram", "master": "RAM8", "instances": ["u_ram"]}],
            "corners": [
                {"name": corners[0], "rc": "nom", "liberty": {"std": "pdk:a.lib", "ram": "pdk:b.lib"}},
                {"name": corners[1], "rc": "nom", "liberty": {"std": "pdk:c.lib", "ram": "pdk:d.lib"}}]})
    };
    let contract = json!({"schema": "rusteda.native-pdk-contract.v1",
        "physical": {"filler_cells": 9, "tap": {"body_view": "gds", "lattices": [
            {"max_gap": 42000, "pattern": {"kind": "staggered", "phase": 21000, "unpaired": "cell_top"}}]}},
        "routing": {"min_routing_layer": "Metal2", "max_routing_layer": "TopMetal2"}});
    // The fixture's floorplan at 2000 database units per micron, and its LEF with the class
    // keywords as the GF180MCU library writes them.
    let floorplan = fs::read_to_string(run.fix.join("recipe/floorplan.json"))
        .unwrap()
        .replace("\"units_per_micron\": 1000", "\"units_per_micron\": 2000");
    let lef = fs::read_to_string(run.fix.join("pdk/cells.lef"))
        .unwrap()
        .replace("CLASS CORE ANTENNACELL ;", "CLASS core ANTENNACELL ;");
    assert!(floorplan.contains("2000") && lef.contains("CLASS core ANTENNACELL"));
    let floorplan = write("floorplan-2000.json", floorplan);
    let lef = write("cells-lower.lef", lef);
    let context_path = run.root.join("frontend/context.json");
    let legacy: Value = serde_json::from_str(&fs::read_to_string(&context_path).unwrap()).unwrap();
    let bind = |profile: &Value, contract: &Value| {
        let mut doc = legacy.clone();
        let files = doc["files"].as_object_mut().unwrap();
        for (key, path) in [
            ("profile", write("profile.json", profile.to_string())),
            ("contract", write("contract.json", contract.to_string())),
            ("floorplan", floorplan.clone()),
            ("std_lef", lef.clone()),
        ] {
            files.insert(key.into(), serde_json::from_str(&record(&path)).unwrap());
        }
        fs::write(&context_path, doc.to_string()).unwrap();
    };
    let read = |rel: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(run.root.join(rel)).unwrap()).unwrap()
    };
    let refused = |command: &str, workspace: &str, extra: &[&str]| -> String {
        let out = run.rusteda(command, workspace, extra);
        assert_eq!(out.status.code(), Some(2), "{command} {workspace}");
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "INPUT");
        error["error"].as_str().unwrap().to_string()
    };

    bind(&profile(["tt", "ss"]), &contract);
    run.ok("frontend-profiles", "frontend/profiles-pdk", &[]);
    let delay = read("frontend/profiles-pdk/delay.json");
    assert_eq!(delay["pdk"], "gf180mcuD");
    assert_eq!(delay["library"], "gf180mcu_fd_sc_mcu7t5v0");
    assert_eq!(delay["corners"][0]["name"], "tt");
    assert_eq!(delay["corners"][1]["name"], "ss");
    assert_eq!(delay["sources"][0]["role"], "liberty_tt");
    assert_eq!(delay["sources"][1]["role"], "liberty_ss");
    assert_eq!(
        delay["buffers"][0]["function_check"],
        "identity in original TT/SS Liberty"
    );
    // The lower-case `core ANTENNACELL` class is the port diode's class.
    assert_eq!(delay["antenna_cells"][0]["name"], "ANTENNA2");
    let sizing = read("frontend/profiles-pdk/sizing.json");
    assert_eq!(sizing["pdk"], "gf180mcuD");
    assert_eq!(sizing["dbu_per_um"], 2000);
    // The family is named by the common prefix of its cells; widths are in the floorplan's unit.
    let family = &sizing["families"][0];
    assert_eq!(family["id"], "BUFX");
    let widths: Vec<(&str, i64)> = family["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cell| {
            (
                cell["name"].as_str().unwrap(),
                cell["width_dbu"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(widths, [("BUFX1", 3360), ("BUFX2", 4200), ("BUFX4", 5880)]);
    // Everything that does not name the PDK, the unit or the changed inputs is the IHP run's.
    let expected: Value = serde_json::from_str(
        &fs::read_to_string(run.fix.join("expected/profiles/delay.json")).unwrap(),
    )
    .unwrap();
    for key in ["schema", "representative", "selection", "excluded"] {
        assert_eq!(delay[key], expected[key], "{key}");
    }
    assert_eq!(
        delay["buffers"][0]["delay_ns"],
        expected["buffers"][0]["delay_ns"]
    );
    for name in ["cell-policy.json", "cell-buses.json", "io.json", "cts.sdc"] {
        assert_eq!(
            fs::read_to_string(run.root.join("frontend/profiles-pdk").join(name)).unwrap(),
            fs::read_to_string(run.fix.join("expected/profiles").join(name)).unwrap(),
            "{name}"
        );
    }

    // The guards of `frontend-physical` take their values from the contract.
    let roots = run.path("frontend/bundle-reset/roots.json");
    run.ok(
        "frontend-physical",
        "frontend/physical-pdk",
        &["--roots", &roots],
    );
    for name in ["physical.json", "routing.json"] {
        assert_eq!(
            run.normalize(
                fs::read_to_string(run.root.join("frontend/physical-pdk").join(name))
                    .unwrap()
                    .replace("physical-pdk", "physical")
            ),
            fs::read_to_string(run.fix.join("expected/physical").join(name)).unwrap(),
            "{name}"
        );
    }
    type Change = fn(&mut Value);
    let cases: [(Change, &str); 5] = [
        (
            |c| c["physical"]["filler_cells"] = json!(8),
            "matched decap/tap template differs",
        ),
        (
            |c| c["physical"]["tap"]["body_view"] = json!("lef"),
            "matched decap/tap template differs",
        ),
        (
            |c| c["physical"]["tap"]["lattices"] = json!([{"max_gap": 15000}]),
            "matched decap/tap template differs",
        ),
        (
            |c| c["routing"]["max_routing_layer"] = json!("Metal5"),
            "matched routing layer policy differs",
        ),
        (
            |c| c["schema"] = json!("rusteda.other.v1"),
            "PDK contract schema",
        ),
    ];
    for (index, (change, message)) in cases.into_iter().enumerate() {
        let mut other = contract.clone();
        change(&mut other);
        bind(&profile(["tt", "ss"]), &other);
        let workspace = format!("frontend/physical-refused-{index}");
        assert_eq!(
            refused("frontend-physical", &workspace, &["--roots", &roots]),
            message,
            "case {index}"
        );
    }
    // The pinned Liberty files are those of the profile's corners.
    bind(&profile(["typ", "slow"]), &contract);
    assert_eq!(
        refused("frontend-profiles", "frontend/profiles-typ", &[]),
        "'std_typ'"
    );
    // A profile without its contract is refused.
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&context_path).unwrap()).unwrap();
    doc["files"].as_object_mut().unwrap().remove("contract");
    fs::write(&context_path, doc.to_string()).unwrap();
    assert_eq!(
        refused("frontend-profiles", "frontend/profiles-bare", &[]),
        "'contract'"
    );
}
