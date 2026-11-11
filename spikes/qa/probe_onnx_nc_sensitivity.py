"""Measure exactly what `num_classes` changes inside an exported ONNX graph.

The offline exporter must serve models trained on the user's own dataset, so the class count is
arbitrary. If a skeleton baked at one class count can be rewired to another by touching only a
handful of initializer dimensions, one skeleton per architecture suffices. If node topology or
constants change, skeletons would have to be generated per class count, which is not shippable.

Run:  python spikes/qa/probe_onnx_nc_sensitivity.py <ultralytics-src> <family> <task> <cfg> <a> <b>
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

import onnx
import torch


def build(ultralytics_src: Path, family: str, task: str, cfg: str, classes: int):
    sys.path.insert(0, str(ultralytics_src))
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
        # Same checkpoint-era SiLU restoration the pinned adapter performs.
        model.model[9].cv1.act = torch.nn.SiLU(inplace=True)
    return model.eval().float()


def wrap(model, task: str, family: str, classes: int):
    from contracts import UltralyticsPortable

    return UltralyticsPortable(model, task, family, classes).eval()


def export(model, path: Path, size: int) -> None:
    dummy = torch.zeros(1, 3, size, size)
    torch.onnx.export(
        model,
        dummy,
        str(path),
        opset_version=17,
        input_names=["images"],
        do_constant_folding=True,
        training=torch.onnx.TrainingMode.EVAL,
        dynamic_axes=None,
        # Same exporter the pinned adapter uses: the dynamo path needs onnxscript, which the
        # locked environment deliberately does not install.
        dynamo=False,
    )


def topology(graph) -> list[str]:
    """Node sequence without names-as-identity: op type plus attribute payload."""
    return [f"{node.op_type}|{sorted((a.name, a.type) for a in node.attribute)}" for node in graph.node]


def main() -> int:
    if len(sys.argv) != 7:
        print(__doc__)
        return 2
    src = Path(sys.argv[1]).resolve()
    family, task, cfg = sys.argv[2], sys.argv[3], sys.argv[4]
    a, b = int(sys.argv[5]), int(sys.argv[6])
    size = 224 if task == "classify" else 640
    # `contracts.py` lives beside `export.py` in the pinned repository, not in this spike directory.
    contracts_dir = src.parent / "montgomery" / "tools" / "onnx"
    sys.path.insert(0, str(contracts_dir))

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        graphs = {}
        for classes in (a, b):
            model = build(src, family, task, cfg, classes)
            path = tmp / f"nc{classes}.onnx"
            export(wrap(model, task, family, classes), path, size)
            graphs[classes] = onnx.load(str(path))

    ga, gb = graphs[a].graph, graphs[b].graph

    print(f"== {family} / {task} / {cfg} : nc {a} vs {b}")
    ta, tb = topology(ga), topology(gb)
    print(f"nodes      {len(ta):>5} vs {len(tb):>5}   topology identical: {ta == tb}")
    if ta != tb:
        for index, (x, y) in enumerate(zip(ta, tb)):
            if x != y:
                print(f"  first divergence at node {index}: {x}  !=  {y}")
                break

    ia = {t.name: list(t.dims) for t in ga.initializer}
    ib = {t.name: list(t.dims) for t in gb.initializer}
    only_a, only_b = sorted(set(ia) - set(ib)), sorted(set(ib) - set(ia))
    print(f"initializers {len(ia):>4} vs {len(ib):>4}   only-in-{a}: {only_a}   only-in-{b}: {only_b}")
    changed = [(name, ia[name], ib[name]) for name in sorted(set(ia) & set(ib)) if ia[name] != ib[name]]
    print(f"initializers whose shape changed: {len(changed)} / {len(ia)}")
    for name, sa, sb in changed[:20]:
        dims = [i for i, (x, y) in enumerate(zip(sa, sb)) if x != y]
        print(f"  {name:<44} {sa} -> {sb}   differing dims {dims}")

    outs = [(o.name, [d.dim_value or d.dim_param for d in o.type.tensor_type.shape.dim]) for o in gb.output]
    print("outputs:", outs)
    vis = len(ga.value_info)
    print(f"value_info entries: {vis} (kept in skeleton or stripped is a size choice only)")

    attrs = sorted(
        (node.op_type, tuple(sorted(a.name for a in node.attribute)))
        for node in ga.node
        if node.attribute
    )
    print(f"nodes carrying attributes: {len(attrs)}")

    meta = {
        "family": family,
        "task": task,
        "cfg": cfg,
        "nc": [a, b],
        "topology_identical": ta == tb,
        "changed_initializers": [{"name": n, "from": x, "to": y} for n, x, y in changed],
        "outputs": outs,
        "value_info": vis,
    }
    print("SUMMARY " + json.dumps(meta))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
