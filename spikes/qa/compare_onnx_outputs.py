"""Compare two ONNX files on identical inputs.

Used to prove the offline fill produces the same model as the Python export: same weights, same
graph, therefore the same numbers. Reports per-output max absolute difference and relative scale.
"""

from __future__ import annotations

import sys

import numpy as np
import onnxruntime as ort


def session(path: str) -> ort.InferenceSession:
    return ort.InferenceSession(path, providers=["CPUExecutionProvider"])


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    reference, candidate = sys.argv[1], sys.argv[2]
    a, b = session(reference), session(candidate)

    shape = [dim or 1 for dim in a.get_inputs()[0].shape]
    rng = np.random.default_rng(7)
    feeds = {a.get_inputs()[0].name: rng.random(shape, dtype=np.float32)}

    outputs_a = a.run(None, feeds)
    feeds_b = {b.get_inputs()[0].name: feeds[a.get_inputs()[0].name]}
    outputs_b = b.run(None, feeds_b)

    if len(outputs_a) != len(outputs_b):
        print(f"FAIL output count {len(outputs_a)} vs {len(outputs_b)}")
        return 1

    failures = 0
    for index, (left, right) in enumerate(zip(outputs_a, outputs_b)):
        if left.shape != right.shape:
            print(f"FAIL output {index} shape {left.shape} vs {right.shape}")
            failures += 1
            continue
        difference = float(np.max(np.abs(left.astype(np.float64) - right.astype(np.float64))))
        scale = float(np.max(np.abs(left)))
        print(
            f"output {index}: shape {left.shape}  max|diff| {difference:.3e}  "
            f"scale {scale:.3e}  {'OK' if difference == 0.0 else 'DIFFERS'}"
        )
        if difference != 0.0:
            failures += 1
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
