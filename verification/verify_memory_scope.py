#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# SPDX-FileCopyrightText: 2026 RustEDA contributors
"""Bounded black-box check of SDF/retention metadata in ordinary OpenSTA analysis.

Run under an OS memory limit, for example MemoryMax=2G and timeout 60s. Five
two-pin cases run sequentially; each command has a 10-second deadline and its
own GNU time record. Missing reference tools produce an unverified result.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def identity(path):
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return dict(path=str(path), sha256=digest, bytes=path.stat().st_size)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--opensta", type=Path, required=True)
    parser.add_argument("--gnu-time", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    args = parser.parse_args()
    work = args.workspace.resolve()
    if work.exists():
        raise SystemExit("REVISION_CONFLICT: fresh verification directory required")
    work.mkdir(parents=True)
    result = dict(status="unverified", cases=[],
                  scope="ordinary NLDM STA only; no output-retention waveform or SDF verification",
                  budgets=dict(max_cases=5, pins_per_case=2, per_case_seconds=10, memory_estimate_mib=128))
    try:
        sta, timer = args.opensta.resolve(strict=True), args.gnu_time.resolve(strict=True)
        result["tools"] = dict(opensta=identity(sta), gnu_time=identity(timer))
        result["version"] = subprocess.check_output([str(sta), "-version"], text=True, timeout=5).strip()
        fixture = Path(__file__).resolve().parent / "fixtures/synthetic/liberty/retention_scope.lib"
        source = fixture.read_text()
        assert len(source) < 8192
        assert source.count('values ( "0.01" )') == 2
        assert source.count("sdf_edges : both_edges;") == 1
        result["fixture"] = identity(fixture)
        (work / "top.v").write_text("module top(input CLK, output Q); MEM mem(.CLK(CLK), .Q(Q)); endmodule\n")
        baseline = None
        cases = [("baseline", "both_edges", "0.01"), ("long_retention", "both_edges", "90.0"),
                 ("noedge", "noedge", "0.01"), ("start", "start_edge", "0.01"),
                 ("end", "end_edge", "0.01")]
        for name, edges, value in cases:
            directory = work / name
            directory.mkdir()
            library = directory / "memory.lib"
            library.write_text(source.replace('values ( "0.01" )', f'values ( "{value}" )')
                              .replace("sdf_edges : both_edges;", f"sdf_edges : {edges};"))
            tcl = directory / "run.tcl"
            tcl.write_text(f"read_liberty {{{library}}}\nread_verilog {{{work / 'top.v'}}}\n"
                           "link_design top\ncreate_clock -name clk -period 100 [get_ports CLK]\n"
                           "set_output_delay 10 -clock clk [get_ports Q]\nset_load 0.01 [get_ports Q]\n"
                           "report_checks -path_delay min_max -digits 9\nexit\n")
            command = [str(timer), "-v", "-o", str(directory / "time.txt"), str(sta), "-exit", str(tcl)]
            with (directory / "stdout").open("w") as out, (directory / "stderr").open("w") as err:
                process = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=out, stderr=err, timeout=10)
            if (directory / "stdout").stat().st_size + (directory / "stderr").stat().st_size > 65536:
                raise ValueError("reference output budget exceeded")
            output, error = (directory / "stdout").read_text(), (directory / "stderr").read_text()
            if process.returncode or re.search(r"\b(?:Error|Warning):", output + error):
                raise ValueError(f"reference could not analyze {name}")
            if "Startpoint:" not in output or "slack (MET)" not in output:
                raise ValueError("reference report is incomplete")
            baseline = output if baseline is None else baseline
            matched = output == baseline
            result["cases"].append(dict(name=name, source=identity(library), command=command,
                                        ordinary_sta_byte_identical=matched))
            if not matched:
                result["status"] = "failed"
                raise ValueError(f"ordinary STA changed for {name}")
        result["status"] = "passed"
    except (OSError, ValueError, AssertionError, subprocess.SubprocessError) as error:
        result["error"] = str(error)
    (work / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
