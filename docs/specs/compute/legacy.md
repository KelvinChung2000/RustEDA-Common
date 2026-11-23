# rusteda-compute: superseded requirements

A changed rule of this crate gets a new ID, and its old text moves here verbatim. The tests that
verify statements a successor kept cite the old ID next to the new one.

## Prepared arc values without patches (replaced by COMPUTE-REQ-0031)

COMPUTE-REQ-0031 in [timing.md](timing.md) keeps the record layout, the validation, the CPU and
CUDA agreement and the memory budgets below. It drops the immutability of the records: an open
session now replaces them with `set_arc_values`.

**COMPUTE-REQ-0030**: TimingProblem may carry one 128-byte ArcValues per incoming
arc, with f64 delay/slew indexed (input transition, output transition, bound).
Prepared values override cell lookup and wire degradation. They are immutable
snapshots of loads, starts and classes: session patch methods must reject edits
until the caller re-prepares the model. Validate size, finite values and
nonnegative transitions. CPU/CUDA use identical data and agree bitwise in f64.
Include these buffers in host/device budgets; test a three-node case plus
extracted-engine propagation without enumerating timing paths.
