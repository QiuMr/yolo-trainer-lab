"""Skeleton probe: is an ONNX graph independent of the weights inside it?

This is the go/no-go test for shipping ONNX export without Python. The plan is to pre-export a
weightless "skeleton" per architecture and let the app pour `.bpk` weights into it, so three things
have to be true:

1. Zeroing every initializer leaves the graph structure byte-for-byte identical (op sequence,
   tensor names, shapes, dtypes). If it does not, the skeleton is a different graph.
2. Refilling the original weights reproduces the original outputs exactly.
3. The initializer list is stable and ordered, so a name->offset table can survive a weight swap.

Usage:
    python probe_onnx_skeleton.py <model.onnx> [--input-shape 1,3,224,224]

Exits non-zero if any check fails.
"""

from __future__ import annotations

import argparse
import hashlib
import sys

import numpy as np
import onnx
import onnxruntime as ort
from onnx import numpy_helper


def fingerprint(graph: onnx.GraphProto) -> dict:
    """Everything a skeleton must preserve. Values are deliberately excluded."""
    ops = [node.op_type for node in graph.node]
    inits = [(t.name, tuple(t.dims), t.data_type) for t in graph.initializer]
    return {
        "nodes": len(graph.node),
        "inputs": [(i.name, [d.dim_value or d.dim_param for d in i.type.tensor_type.shape.dim]) for i in graph.input],
        "outputs": [o.name for o in graph.output],
        "op_digest": hashlib.sha256("\n".join(ops).encode()).hexdigest()[:16],
        "init_digest": hashlib.sha256(
            "\n".join(f"{name}|{dims}|{dtype}" for name, dims, dtype in inits).encode()
        ).hexdigest()[:16],
        "init_count": len(inits),
        "total_params": int(sum(int(np.prod(t.dims)) for t in graph.initializer)),
    }


def zero_weights(model: onnx.ModelProto) -> dict[str, np.ndarray]:
    """Replace every initializer's data with zeros, keeping name/shape/dtype. Returns the originals."""
    originals: dict[str, np.ndarray] = {}
    for tensor in model.graph.initializer:
        original = numpy_helper.to_array(tensor)
        originals[tensor.name] = original
        replacement = numpy_helper.from_array(np.zeros_like(original), tensor.name)
        tensor.CopyFrom(replacement)
    return originals


def refill(model: onnx.ModelProto, originals: dict[str, np.ndarray]) -> None:
    for tensor in model.graph.initializer:
        tensor.CopyFrom(numpy_helper.from_array(originals[tensor.name], tensor.name))


def run(path: str, shape: list[int]) -> list[np.ndarray]:
    session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    name = session.get_inputs()[0].name
    # Deterministic input: a fixed seed beats "random" for a comparison that must be repeatable.
    rng = np.random.default_rng(20261111)
    data = rng.random(shape, dtype=np.float32)
    return session.run(None, {name: data})


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("model")
    parser.add_argument("--input-shape", default="1,3,224,224")
    parser.add_argument("--tolerance", type=float, default=1e-6)
    args = parser.parse_args()
    shape = [int(part) for part in args.input_shape.split(",")]

    model = onnx.load(args.model)
    before = fingerprint(model.graph)
    print("original:", before)

    originals = zero_weights(model)
    zeroed = fingerprint(model.graph)
    print("zeroed  :", zeroed)

    zeroed_path = args.model.replace(".onnx", ".skeleton.onnx")
    onnx.save(model, zeroed_path)
    try:
        onnx.checker.check_model(zeroed_path)
        checker = "ok"
    except Exception as error:  # noqa: BLE001 - any failure is the answer
        checker = f"FAILED: {error}"
    print(f"skeleton saved: {zeroed_path} (checker: {checker})")

    structure_ok = before["op_digest"] == zeroed["op_digest"] and before["init_digest"] == zeroed["init_digest"]

    refill(model, originals)
    after = fingerprint(model.graph)
    restored_path = args.model.replace(".onnx", ".restored.onnx")
    onnx.save(model, restored_path)
    print("restored:", after)

    base_out = run(args.model, shape)
    restored_out = run(restored_path, shape)
    diffs = [float(np.max(np.abs(a - b))) for a, b in zip(base_out, restored_out)]
    numeric_ok = all(diff <= args.tolerance for diff in diffs)

    print()
    print(f"[{'PASS' if structure_ok else 'FAIL'}] graph structure survives zeroing")
    print(f"[{'PASS' if numeric_ok else 'FAIL'}] refilled weights reproduce outputs (max|diff|={max(diffs):.3e})")
    print(f"[{'PASS' if before == after else 'FAIL'}] restore is lossless")

    # Every initializer name must be unique, otherwise a name->offset table cannot address them.
    names = [t.name for t in onnx.load(args.model).graph.initializer]
    unique_ok = len(names) == len(set(names))
    print(f"[{'PASS' if unique_ok else 'FAIL'}] initializer names are unique ({len(names)} tensors)")

    return 0 if structure_ok and numeric_ok and unique_ok else 1


if __name__ == "__main__":
    sys.exit(main())
