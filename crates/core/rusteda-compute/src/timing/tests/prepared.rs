// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2026 RustEDA contributors

//! COMPUTE-REQ-0031 and the statements it keeps of the superseded COMPUTE-REQ-0030: the
//! three-node prepared-arc ABI, frozen loads, classes and starts, and records patched in an open
//! session, which a cone propagates equal to a fresh full run.
use super::cpu::differing_rows;
use super::random::{self, Owned, Rng, Shape};
use super::{assert_same, pool};
use crate::timing::*;
use crate::{BackendKind, ComputeError, Precision};
use std::borrow::Cow;

fn run(kind: BackendKind) -> TimingValues {
    let in_ptr = vec![0, 0, 1, 2];
    let arcs = vec![
        InArc {
            src: 0,
            class: 0,
            map: NONE,
        },
        InArc {
            src: 1,
            class: 1,
            map: NONE,
        },
    ];
    let (out_ptr, out_dst) = out_adjacency(3, &in_ptr, &arcs);
    let graph = LevelizedTimingCsr {
        level_ptr: Cow::Owned(vec![0, 1, 2, 3]),
        in_ptr: Cow::Owned(in_ptr),
        in_arcs: Cow::Owned(arcs),
        out_ptr: Cow::Owned(out_ptr),
        out_dst: Cow::Owned(out_dst),
        map_ptr: Cow::Borrowed(&[0]),
        maps: Cow::Borrowed(&[]),
    };
    let bank = CompiledLutBank {
        luts: Cow::Owned(vec![LutDesc::scalar(0), LutDesc::scalar(1)]),
        data: Cow::Owned(vec![100.0, 100.0]),
        classes: Cow::Owned(vec![
            ArcClass::cell(POSITIVE_UNATE, [0; 2], [1; 2]),
            ArcClass::wire(),
        ]),
    };
    let mut starts = StartRows::default();
    starts.push(0, &[0.0; 4], &[0.1; 4]);
    let values = [
        ArcValues {
            delay: [0.2; 8],
            slew: [0.15; 8],
        },
        ArcValues {
            delay: [0.03; 8],
            slew: [0.17; 8],
        },
    ];
    let loads = [[0.0; 4]; 3];
    let problem = TimingProblem {
        graph: &graph,
        luts: &bank,
        ntags: 1,
        load: &loads,
        starts: &starts,
        wires: &[],
        arc_values: &values,
    };
    problem.validate().unwrap();
    let backend = timing_backend(kind, Precision::F64).unwrap();
    let mut session = backend.open(&problem).unwrap();
    session.propagate(&Request::all()).unwrap();
    assert!(session.set_loads(&[(1, [1.0; 4])]).is_err());
    assert!(session.set_starts(&starts).is_err());
    assert!(session.set_arc_classes(&[(0, 0)]).is_err());
    let result = session.into_values();
    assert!((result.arrival(2, 0, 0, 1) - 0.23).abs() < 1e-15);
    assert_eq!(result.transition(2, 0, 1), 0.17);
    assert!(
        TimingProblem {
            arc_values: &values[..1],
            ..problem
        }
        .validate()
        .is_err()
    );
    let mut invalid = values;
    invalid[0].slew[0] = f64::NAN;
    assert!(
        TimingProblem {
            arc_values: &invalid,
            ..problem
        }
        .validate()
        .is_err()
    );
    result
}

#[test]
fn prepared_values_and_frozen_inputs() {
    run(BackendKind::Cpu);
}

#[test]
#[cfg(feature = "cuda")]
#[ignore = "requires native CUDA, execute in a physical-memory cgroup"]
fn prepared_cpu_cuda_agree() {
    assert_same(
        &run(BackendKind::Cpu),
        &run(BackendKind::Cuda),
        "prepared RC arc f64",
    );
}

/// A random prepared record: finite delays of either sign, non-negative transitions.
fn record(r: &mut Rng) -> ArcValues {
    let mut rec = ArcValues::default();
    for j in 0..8 {
        rec.delay[j] = r.range(-0.05, 0.4);
        rec.slew[j] = r.range(0.0, 0.3);
    }
    rec
}

/// A random problem of `seed` wide enough for parallel levels, with one record per arc.
fn prepared(seed: u64) -> (Owned, Vec<ArcValues>) {
    let mut shape = Shape::from_seed(seed);
    shape.width = 80 + (seed as usize % 5) * 40;
    let o = random::problem(seed, shape);
    assert!(o.graph.arcs() > 0, "seed {seed} has no arcs");
    let mut r = Rng::new(seed ^ 0xA2C);
    let recs = (0..o.graph.arcs()).map(|_| record(&mut r)).collect();
    (o, recs)
}

/// The destination node of arc `k` in CSR order.
fn destination(g: &LevelizedTimingCsr, k: usize) -> usize {
    g.in_ptr.partition_point(|&p| p as usize <= k) - 1
}

/// The first arc whose destination's forward cone holds a node with two incoming arcs from the
/// cone, so that the patch reaches that node along two paths.
fn reconvergent_arc(g: &LevelizedTimingCsr) -> Option<usize> {
    (0..g.arcs()).find(|&k| {
        let mut reach = vec![false; g.nodes()];
        let mut stack = vec![destination(g, k)];
        reach[stack[0]] = true;
        while let Some(u) = stack.pop() {
            for &d in &g.out_dst[g.out_ptr[u] as usize..g.out_ptr[u + 1] as usize] {
                if !reach[d as usize] {
                    reach[d as usize] = true;
                    stack.push(d as usize);
                }
            }
        }
        (0..g.nodes()).any(|w| {
            reach[w]
                && g.incoming(w)
                    .iter()
                    .filter(|a| reach[a.src as usize])
                    .count()
                    >= 2
        })
    })
}

/// Which records a case replaces.
#[derive(Debug, Clone, Copy)]
enum PatchSet {
    SingleArc,
    /// A quarter of the arcs, with repeats: the last edit of an arc wins.
    ManyArcs,
    Reconvergent,
    /// Records replaced by themselves: nothing changes.
    Identical,
}

fn edits(set: PatchSet, o: &Owned, recs: &[ArcValues], r: &mut Rng) -> Vec<(u32, ArcValues)> {
    let na = recs.len();
    match set {
        PatchSet::SingleArc => vec![(r.below(na) as u32, record(r))],
        PatchSet::ManyArcs => (0..na.div_ceil(4))
            .map(|_| (r.below(na) as u32, record(r)))
            .collect(),
        PatchSet::Reconvergent => {
            let k = reconvergent_arc(&o.graph).expect("a problem with a reconvergent arc");
            vec![(k as u32, record(r))]
        }
        PatchSet::Identical => (0..4)
            .map(|_| r.below(na))
            .map(|k| (k as u32, recs[k]))
            .collect(),
    }
}

/// Opens a session on `recs`, propagates every node, replaces `edits` and propagates `scope`;
/// returns the values before and after and the changed rows.
fn patched_run(
    backend: &dyn TimingPropagationBackend,
    o: &Owned,
    recs: &[ArcValues],
    edits: &[(u32, ArcValues)],
    scope: Scope<'_>,
) -> (TimingValues, TimingValues, Option<Vec<u32>>) {
    let p = TimingProblem {
        arc_values: recs,
        ..o.problem()
    };
    let mut s = backend.open(&p).unwrap_or_else(|e| panic!("open: {e}"));
    s.propagate(&Request::all())
        .unwrap_or_else(|e| panic!("propagate: {e}"));
    let before = s.values().clone();
    s.set_arc_values(edits)
        .unwrap_or_else(|e| panic!("set_arc_values: {e}"));
    s.propagate(&Request::all().with_scope(scope))
        .unwrap_or_else(|e| panic!("propagate: {e}"));
    let changed = s.changed().map(<[u32]>::to_vec);
    (before, s.into_values(), changed)
}

/// The values of a fresh session on `recs` with `edits` applied in order, every node.
fn fresh_run(o: &Owned, recs: &[ArcValues], edits: &[(u32, ArcValues)]) -> TimingValues {
    let mut recs = recs.to_vec();
    for &(k, rec) in edits {
        recs[k as usize] = rec;
    }
    let p = TimingProblem {
        arc_values: &recs,
        ..o.problem()
    };
    let mut s = CPU.open(&p).unwrap_or_else(|e| panic!("open: {e}"));
    s.propagate(&Request::all())
        .unwrap_or_else(|e| panic!("propagate: {e}"));
    s.into_values()
}

const CASES: [(u64, PatchSet); 8] = [
    (500, PatchSet::SingleArc),
    (501, PatchSet::SingleArc),
    (502, PatchSet::ManyArcs),
    (503, PatchSet::ManyArcs),
    (504, PatchSet::Reconvergent),
    (505, PatchSet::Reconvergent),
    (506, PatchSet::Identical),
    (507, PatchSet::Identical),
];

// COMPUTE-REQ-0031, COMPUTE-REQ-0019, COMPUTE-REQ-0021: records replaced in an open session and
// propagated as a cone (or from the lowest patched level) equal a fresh session on the replaced
// records propagated in full, bit for bit, on 1, 3 and 8 threads; the cone lists exactly the rows
// that changed, none for records replaced by themselves.
#[test]
fn patched_arc_values_equal_a_fresh_run() {
    let pools: Vec<(usize, rayon::ThreadPool)> =
        [1, 3, 8].into_iter().map(|t| (t, pool(t))).collect();
    for (seed, set) in CASES {
        let (o, recs) = prepared(seed);
        let e = edits(set, &o, &recs, &mut Rng::new(seed ^ 0xED17));
        let fresh = pools[0].1.install(|| fresh_run(&o, &recs, &e));
        for (t, p) in &pools {
            for scope in [Scope::Cone(&[]), Scope::FromLevel(o.graph.levels() as u32)] {
                let what = format!("seed {seed}, {set:?}, {scope:?}, {t} threads");
                let (before, after, changed) =
                    p.install(|| patched_run(&CPU, &o, &recs, &e, scope));
                assert_same(&after, &fresh, &what);
                if let Scope::Cone(_) = scope {
                    let want = differing_rows(&before, &fresh);
                    assert_eq!(changed.as_deref(), Some(&want[..]), "{what}");
                    let identical = matches!(set, PatchSet::Identical);
                    assert_eq!(want.is_empty(), identical, "{what}: rows changed");
                }
            }
        }
    }
}

// COMPUTE-REQ-0031, COMPUTE-REQ-0020: set_arc_values is rejected in a session opened without
// records, for an arc out of range and for a record that is not finite or has a negative
// transition; a rejected patch applies none of its edits.
#[test]
fn arc_value_patches_are_validated() {
    let (o, recs) = prepared(510);
    let na = recs.len() as u32;
    let good = recs[0];
    let with = |f: fn(&mut ArcValues)| {
        let mut rec = good;
        f(&mut rec);
        rec
    };
    let rejected: [(&str, Vec<(u32, ArcValues)>); 6] = [
        ("arc out of range", vec![(0, good), (na, good)]),
        (
            "NaN delay",
            vec![(0, good), (1 % na, with(|r| r.delay[3] = f64::NAN))],
        ),
        (
            "infinite delay",
            vec![(0, with(|r| r.delay[0] = f64::INFINITY))],
        ),
        (
            "-inf delay",
            vec![(0, with(|r| r.delay[7] = f64::NEG_INFINITY))],
        ),
        (
            "infinite transition",
            vec![(0, with(|r| r.slew[2] = f64::INFINITY))],
        ),
        (
            "negative transition",
            vec![(0, with(|r| r.slew[5] = -1e-3))],
        ),
    ];
    let p = TimingProblem {
        arc_values: &recs,
        ..o.problem()
    };
    let mut s = CPU.open(&p).unwrap_or_else(|e| panic!("{e}"));
    s.propagate(&Request::all())
        .unwrap_or_else(|e| panic!("{e}"));
    for (what, e) in &rejected {
        assert!(
            matches!(s.set_arc_values(e), Err(ComputeError::Invalid(_))),
            "{what}"
        );
        s.propagate(&Request::cone(&[]))
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(s.changed(), Some(&[][..]), "{what}: nothing applied");
    }
    let mut plain = CPU.open(&o.problem()).unwrap_or_else(|e| panic!("{e}"));
    assert!(matches!(
        plain.set_arc_values(&[(0, good)]),
        Err(ComputeError::Invalid(_))
    ));
}

// COMPUTE-REQ-0031, COMPUTE-REQ-0019: a CUDA session patched with set_arc_values and propagated as
// a cone equals the CPU's fresh full run bit for bit in f64.
#[test]
#[cfg(feature = "cuda")]
#[ignore = "needs a CUDA GPU"]
fn cuda_patched_arc_values_equal_the_cpu() {
    let cuda = timing_backend(BackendKind::Cuda, Precision::F64).unwrap_or_else(|e| panic!("{e}"));
    for (seed, set) in CASES {
        let (o, recs) = prepared(seed);
        let e = edits(set, &o, &recs, &mut Rng::new(seed ^ 0xED17));
        let fresh = fresh_run(&o, &recs, &e);
        let p = TimingProblem {
            arc_values: &recs,
            ..o.problem()
        };
        let mut s = cuda.open(&p).unwrap_or_else(|e| panic!("open: {e}"));
        s.propagate(&Request::all())
            .unwrap_or_else(|e| panic!("propagate: {e}"));
        s.set_arc_values(&e)
            .unwrap_or_else(|e| panic!("set_arc_values: {e}"));
        s.propagate(&Request::cone(&[]).with_readback(Readback::All))
            .unwrap_or_else(|e| panic!("propagate: {e}"));
        assert_same(s.values(), &fresh, &format!("seed {seed}, {set:?}, cuda"));
    }
}
