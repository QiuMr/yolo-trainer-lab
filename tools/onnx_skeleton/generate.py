"""Generate a weight-free ONNX skeleton for offline (Python-free) export.

Why this exists
---------------
Shipping ONNX export to users without Python needs the graph in advance. The graph is
architecture-dependent but *class-count independent* in topology, so one skeleton per
architecture serves every dataset. Two details make a naive "zero the weights" skeleton
unusable, and both are handled here:

1. **Conv+BN fusion.** Torch's ONNX exporter folds `conv` and `bn` into a single Conv node and
   names the fused weight `onnx::Conv_N`. Those values are `w * gamma / sqrt(var + eps)`, not the
   checkpoint's raw tensors, so a client cannot copy parameters in verbatim. The BN epsilon is a
   per-family constant (Ultralytics 1e-3, YOLOv8-cls 1e-5). We rename every fused initializer to
   `<module>.fused_weight` / `<module>.fused_bias` and record the fold recipe, epsilon included.
2. **Class-count-dependent shapes.** `nc` changes the width of the detection head's classification
   tower and the class output width. Freshly trained models have arbitrary `nc`, so the skeleton is
   generated at a canonical count and the class-dependent dimensions are recorded explicitly. The
   exporter then copies those dimensions from the real weights instead of from the skeleton.

Run from a machine with the pinned export environment; the result ships inside the installer.

  python tools/onnx_skeleton/generate.py \
      --ultralytics <../ultralytics> --contracts <../montgomery/tools/onnx> \
      --family yolov8 --task detect --cfg yolov8n.yaml --model-id yolov8n \
      --nc 80 --out desktop/src-tauri/resources/onnx
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
from pathlib import Path

import onnx
import torch
from onnx import numpy_helper


def import_contracts(contracts_dir: Path):
    sys.path.insert(0, str(contracts_dir))
    from contracts import UltralyticsPortable

    return UltralyticsPortable


def build(ultralytics_dir: Path, family: str, task: str, cfg: str, classes: int):
    sys.path.insert(0, str(ultralytics_dir))
    from ultralytics.nn.tasks import (
        ClassificationModel,
        DepthModel,
        DetectionModel,
        SegmentationModel,
        SemanticSegmentationModel,
    )

    ctor = {
        "classify": ClassificationModel,
        "segment": SegmentationModel,
        "semantic": SemanticSegmentationModel,
        "depth": DepthModel,
    }.get(task, DetectionModel)
    model = ctor(cfg=cfg, ch=3, nc=classes, verbose=False)
    if family in {"yolov8", "yolov10", "yolo11"} and task != "classify":
        # Released v8/v10/v11 checkpoints predate the SPPF.cv1 act=False refactor; the pinned
        # adapter restores the parameter-free SiLU so the graph matches the checkpoint.
        model.model[9].cv1.act = torch.nn.SiLU(inplace=True)
    decorrelate(model)
    return model.eval().float()


def decorrelate(model) -> None:
    """Make every parameter value distinct.

    A freshly built model reuses values: BatchNorm starts at identity (so every Conv+BN fold yields
    the same zero bias) and Ultralytics initializes head biases to a shared constant. Torch then
    folds the duplicates into ONE shared initializer plus Identity nodes, producing a topology that
    no trained checkpoint ever has — and a skeleton whose shared tensor cannot be filled per layer.
    Small per-parameter noise keeps the exported graph identical to the real export.
    """
    generator = torch.Generator().manual_seed(0)
    with torch.no_grad():
        for name, parameter in model.named_parameters():
            if name.endswith("dfl.conv.weight"):
                # The DFL projection is not a learned parameter: it is the bin index row
                # [0, 1, ..., reg_max-1], baked into the graph as a constant. Perturbing it would
                # shift every predicted distance by ~0.02 * stride pixels.
                parameter.copy_(torch.arange(parameter.numel(), dtype=parameter.dtype).view_as(parameter))
                continue
            parameter.add_(torch.randn(parameter.shape, generator=generator) * 1e-2)
        for module in model.modules():
            if isinstance(module, torch.nn.BatchNorm2d):
                module.num_batches_tracked.fill_(7)


def export(contracts_dir: Path, model, task: str, family: str, classes: int, path: Path):
    import_contracts(contracts_dir)
    from contracts import UltralyticsPortable, output_names

    wrapper = UltralyticsPortable(model, task, family, classes).eval()
    size = 224 if task == "classify" else 640
    sample = torch.zeros(1, 3, size, size, dtype=torch.float32)
    with torch.no_grad():
        wrapper(sample)
    torch.onnx.export(
        wrapper,
        (sample,),
        str(path),
        input_names=["images"],
        # Naming the outputs matters: without it torch inserts Identity nodes to preserve output
        # identity, which no longer matches the graph the shipping exporter produces.
        output_names=output_names(task, "portable", family),
        opset_version=17,
        export_params=True,
        do_constant_folding=True,
        training=torch.onnx.TrainingMode.EVAL,
        dynamic_axes=None,
        # The dynamo exporter needs onnxscript, which the locked environment omits on purpose.
        dynamo=False,
    )
    return onnx.load(str(path))


def module_path(node_name: str) -> str | None:
    """`/model/model.2/m.0/cv1/conv/Conv` -> `model.2.m.0.cv1.conv`."""
    if not node_name.startswith("/model/"):
        return None
    return node_name[len("/model/") :].rsplit("/", 1)[0].replace("/", ".")


def bn_hosts(model) -> list[tuple[str, list[int]]]:
    """Every module that carries a Conv2d plus a BatchNorm2d, with the conv's weight shape.

    Ultralytics' `Conv` is such a module (`.conv` + `.bn`), and its ONNX node path is traced as
    `/model/<module>/conv/Conv`. Node paths duplicate the trailing index (`cv2.0/cv2.0.0`) because
    the tracer names the Sequential child by its full attribute path, so the module is resolved by
    matching the traced tail against real module names and confirming the weight shape.
    """
    hosts = []
    for name, module in model.named_modules():
        conv = getattr(module, "conv", None)
        bn = getattr(module, "bn", None)
        if isinstance(conv, torch.nn.Conv2d) and isinstance(bn, torch.nn.BatchNorm2d):
            hosts.append((name, list(conv.weight.shape)))
    return hosts


def common_run(left: list[str], right: list[str]) -> int:
    """Longest contiguous run of segments shared by both paths."""
    best = 0
    for start in range(len(left)):
        for other in range(len(right)):
            length = 0
            while (
                start + length < len(left)
                and other + length < len(right)
                and left[start + length] == right[other + length]
            ):
                length += 1
            best = max(best, length)
    return best


def assign_hosts(anonymous: list[tuple[str, list[int]]], hosts: list[tuple[str, list[int]]]) -> dict[str, str]:
    """Pair every fused Conv with the module whose parameters produced it.

    Traced node paths repeat the trailing index (`model.22/cv2.0/cv2.0.0` for module
    `model.22.cv2.0.0`), so a plain suffix test fails. Paths are matched by their longest shared
    segment run, shape must agree exactly, and the result must be an injective mapping: a host used
    twice means the pairing went wrong, which is better to hear about than to ship.
    """
    scored = []
    for index, (traced, shape) in enumerate(anonymous):
        path = traced[:-5] if traced.endswith(".conv") else traced
        segments = path.split(".")
        for name, weight_shape in hosts:
            if weight_shape != shape:
                continue
            run = common_run(segments, name.split("."))
            # Longer shared runs first; then the more specific (deeper) module name.
            scored.append((run, len(name.split(".")), index, name))
    scored.sort(reverse=True)

    chosen: dict[int, str] = {}
    used: set[str] = set()
    for _run, _depth, index, name in scored:
        if index in chosen or name in used:
            continue
        chosen[index] = name
        used.add(name)
    missing = [anonymous[index][0] for index in range(len(anonymous)) if index not in chosen]
    if missing:
        raise SystemExit(f"unmatched fused Convs (no free module left): {missing}")
    return {anonymous[index][0]: name for index, name in chosen.items()}


def bn_epsilon(modules: dict[str, torch.nn.Module], path: str) -> float:
    """The epsilon the exporter folded with is the module's own, whatever the family uses."""
    module = modules.get(path)
    eps = getattr(module, "eps", None)
    if eps is None:
        raise SystemExit(f"no BatchNorm module at {path}; refusing to guess its epsilon")
    return float(eps)


def describe(graph, model, state_keys: set[str]) -> tuple[list[dict], list[dict]]:
    """Rename fused initializers and build the fill plan."""
    modules = dict(model.named_modules())
    initializers = {tensor.name: tensor for tensor in graph.initializer}
    consumer: dict[str, tuple[object, int]] = {}
    for node in graph.node:
        for slot, name in enumerate(node.input[1:3], start=1):
            if name in initializers:
                consumer[name] = (node, slot)

    anonymous = []
    for node in graph.node:
        if node.op_type != "Conv" or len(node.input) < 2:
            continue
        weight = node.input[1]
        if weight in initializers and weight.startswith("onnx::"):
            traced = module_path(node.name)
            if traced is None:
                raise SystemExit(f"cannot derive a module path from node {node.name!r}")
            anonymous.append((traced, list(initializers[weight].dims)))
    assignment = assign_hosts(anonymous, bn_hosts(model))

    renamed: dict[str, str] = {}
    entries: list[dict] = []
    for name, tensor in initializers.items():
        holder = consumer.get(name)
        # ONNX names carry the wrapper prefix (`model.model.22...`); the checkpoint snapshot does
        # not (`model.22...`), because it is named after the inner module tree.
        source = {"kind": "copy", "key": name.removeprefix("model.")}
        target = name
        if name.startswith("onnx::"):
            if holder is None:
                raise SystemExit(f"anonymous initializer {name} has no consuming node")
            node, slot = holder
            host = assignment[module_path(node.name)]
            suffix = "weight" if slot == 1 else "bias"
            # Keep the wrapper prefix so every initializer name in the skeleton follows one rule.
            target = f"model.{host}.conv.fused_{suffix}"
            source = {
                "kind": "conv_bn",
                "conv": f"{host}.conv",
                "bn": f"{host}.bn",
                "slot": suffix,
                "eps": bn_epsilon(modules, f"{host}.bn"),
            }
        if source["kind"] == "copy" and (
            source["key"] not in state_keys
            # The DFL projection is a fixed 1x1 of ones: it is a torch parameter, so it is in the
            # state dict, but the Burn graph represents it as a constant and never writes it into a
            # checkpoint (`ultralytics_adapter._allowed_missing` explicitly permits that). Nothing
            # will fill it, so its real bytes stay in the skeleton.
            or source["key"].endswith("dfl.conv.weight")
        ):
            # Some inference constants never appear in a checkpoint: the DFL projection is a fixed
            # 1x1 of ones, and the adapter's audit explicitly allows it to be missing. The skeleton
            # keeps such a tensor's real bytes instead of zeroing it, because nothing will fill it.
            source = {"kind": "constant"}
        renamed[name] = target
        entries.append(
            {
                "name": target,
                "dtype": str(tensor.data_type),
                "shape": list(tensor.dims),
                "source": source,
            }
        )

    for node in graph.node:
        node.input[:] = [renamed.get(name, name) for name in node.input]
    for tensor in graph.initializer:
        tensor.name = renamed[tensor.name]

    outputs = [
        {
            "name": out.name,
            "shape": [dim.dim_value if dim.HasField("dim_value") else None
                      for dim in out.type.tensor_type.shape.dim],
        }
        for out in graph.output
    ]
    return entries, outputs


def int_constants(graph) -> dict[str, list[int]]:
    """Every `Constant` node that carries an int64 vector, keyed by node name.

    The class count also leaks into the graph as a *value*, not a shape: the portable wrapper
    slices `decoded[:, 4 : 4 + nc]`, so the Slice bounds move with the dataset. Those constants must
    be rewritten at fill time, otherwise the graph still cuts 84 channels out of a 7-channel tensor.
    """
    found = {}
    for node in graph.node:
        if node.op_type != "Constant":
            continue
        for attribute in node.attribute:
            if attribute.name != "value" or attribute.t.data_type != 7:
                continue
            values = [int(value) for value in attribute.t.int64_data]
            if not values and attribute.t.raw_data:
                raw = attribute.t.raw_data
                values = [
                    int.from_bytes(raw[offset : offset + 8], "little", signed=True)
                    for offset in range(0, len(raw), 8)
                ]
            found[node.name] = values
    return found


def int_attributes(graph) -> dict[tuple[str, str], list[int]]:
    """Every integer node attribute, keyed by (node name, attribute name).

    A second way the class count leaks into the graph: YOLO11/26 classify towers are depthwise, so
    `Conv.group` equals the tower width — and the width follows the class count. That is an
    attribute, not a tensor, so no amount of weight filling would fix it.
    """
    found = {}
    for node in graph.node:
        for attribute in node.attribute:
            if attribute.type == 2:  # AttributeProto::INT
                found[(node.name, attribute.name)] = [int(attribute.i)]
            elif attribute.type == 7:  # AttributeProto::INTS
                found[(node.name, attribute.name)] = [int(value) for value in attribute.ints]
    return found


def class_attributes(full, narrow) -> list[dict]:
    wide = int_attributes(full.graph)
    tight = int_attributes(narrow.graph)
    changed = []
    for (node, name), values in wide.items():
        other = tight.get((node, name))
        if other is None or len(other) != len(values):
            continue
        mask = [left != right for left, right in zip(values, other)]
        if any(mask):
            # `group` on a depthwise convolution equals the tower width, which is
            # `max(p3, min(nc, 100))` — not the class count. It must come from the weight's own
            # output-channel count instead of being shifted by the class delta.
            rule = "weight_out_channels" if name == "group" else "delta"
            changed.append(
                {
                    "node": node,
                    "name": name,
                    "values": values,
                    "class_dependent": mask,
                    "rule": rule,
                }
            )
    return changed


def class_scalars(full, narrow) -> list[dict]:
    """Constants whose value follows the class count, element by element."""
    wide = int_constants(full.graph)
    tight = int_constants(narrow.graph)
    scalars = []
    for name, values in wide.items():
        other = tight.get(name)
        if other is None or len(other) != len(values):
            continue
        mask = [left != right for left, right in zip(values, other)]
        if any(mask):
            scalars.append({"node": name, "values": values, "class_dependent": mask})
    return scalars


def zero(tensor) -> None:
    """Keep name, dtype and dims; drop the payload.

    The skeleton ships inside the installer and is never executed: the fill step supplies every
    byte. Carrying zero payloads would make each skeleton as large as a real model (12 MB), so the
    bytes are dropped entirely and only the shape metadata remains.
    """
    tensor.raw_data = b""
    del tensor.float_data[:]
    del tensor.int32_data[:]
    del tensor.int64_data[:]
    del tensor.double_data[:]
    del tensor.string_data[:]
    del tensor.uint64_data[:]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ultralytics", required=True, type=Path)
    parser.add_argument("--contracts", required=True, type=Path)
    parser.add_argument("--family", required=True)
    parser.add_argument("--task", required=True)
    parser.add_argument("--cfg", required=True)
    parser.add_argument("--model-id", required=True)
    parser.add_argument("--nc", type=int, default=80)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    base = args.nc
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        model = build(args.ultralytics, args.family, args.task, args.cfg, base)
        modules = dict(model.named_modules())
        full = export(args.contracts, model, args.task, args.family, base, tmp / "base.onnx")
        # A one-class-smaller model isolates every dimension that follows the class count,
        # instead of trusting a hand-written list of head layers.
        narrow_model = build(args.ultralytics, args.family, args.task, args.cfg, base - 1)
        narrow = export(
            args.contracts,
            narrow_model,
            args.task,
            args.family,
            base - 1,
            tmp / "narrow.onnx",
        )
        narrow_entries, narrow_outputs = describe(
            narrow.graph, narrow_model, set(narrow_model.state_dict().keys())
        )

    entries, outputs = describe(full.graph, model, set(model.state_dict().keys()))

    narrow_shapes = {entry["name"]: entry["shape"] for entry in narrow_entries}
    for entry in entries:
        other = narrow_shapes.get(entry["name"])
        entry["class_dependent"] = other is not None and other != entry["shape"]

    # Output dimensions derive from the class count arithmetically (4+nc for detection), so the
    # skeleton records the base value; the exporter shifts it by the class delta.
    narrow_by_name = {out["name"]: out["shape"] for out in narrow_outputs}
    for out in outputs:
        other = narrow_by_name.get(out["name"])
        out["class_dims"] = [
            {"dim": index, "base": value, "offset": (value - base) if value is not None else None}
            for index, value in enumerate(out["shape"])
            if other is not None
            and index < len(other)
            and value is not None
            and other[index] is not None
            and value != other[index]
        ]

    args.out.mkdir(parents=True, exist_ok=True)
    skeleton = args.out / f"{args.model_id}.onnx"
    keep = {
        entry["name"]
        for entry in entries
        if entry["source"]["kind"] == "constant"
    }
    for tensor in full.graph.initializer:
        if tensor.name not in keep:
            zero(tensor)
    del full.graph.value_info[:]
    full.doc_string = "montgomery offline-export skeleton (weights stripped)"
    onnx.save(full, str(skeleton))

    manifest = {
        "schema": "montgomery-onnx-skeleton-v1",
        "model_id": args.model_id,
        "family": args.family,
        "task": args.task,
        "profile": "portable",
        "opset": 17,
        "base_classes": base,
        "input_shape": [1, 3, 224 if args.task == "classify" else 640,
                        224 if args.task == "classify" else 640],
        "onnx": skeleton.name,
        "tensors": entries,
        "outputs": outputs,
        "scalars": class_scalars(full, narrow),
        "attributes": class_attributes(full, narrow),
    }
    (args.out / f"{args.model_id}.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    print(f"{args.model_id}: {len(entries)} tensors -> {skeleton} "
          f"({skeleton.stat().st_size / 1024:.0f} KiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
