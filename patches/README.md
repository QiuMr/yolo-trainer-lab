# 上游补丁

`research/` 目录（montgomery 与 Burn 的本地副本）整个被 `.gitignore` 排除，所以对上游源码的
修改**不会自动进入本仓库**。但 `desktop/src-tauri/binaries/` 里的 `montgomery.exe` 是由打了补丁
的源码编译出来的——不留下补丁，下次重新构建就会得到一个行为不同的引擎，而且很难查。

构建前先应用：

```bash
git clone https://github.com/boquila/montgomery.git research/montgomery
cd research/montgomery && git checkout <构建镜像对应的 commit>
git apply ../../patches/*.patch
cargo build --release --features training,onnx
```

**注意 `--features training,onnx`**：默认 feature 是 `["pretrained", "onnx"]`，不含 `training`，
而 `main.rs` 里有只在 `training` 下编译的调用，默认构建会直接编译失败。

## 0002 · 分类塔宽度按真实类别数重算

`head.rs` 的注释写着 Ultralytics 的规则是 `max(ch[0], min(nc, 100))`，代码却写成
`p3_channels.max(DEFAULT_NUM_CLASSES.min(100))`——把 `nc` 写成了常量 80。而
`with_num_classes` 只改 `num_classes`、不重算 `cls_channels`，宽度在构造时就定死了。

后果：训练完全正常（多余通道闲置），只有导出时才暴露成 33 个 wrong-shape 张量，
且**只有 nc ≥ 80 的模型能导出**。改 yolov8 / yolo11 / yolo26 / yolov10 四个 head
（yolo12 复用 yolo11 的 head）。

**这个补丁改变模型结构，此前训的 checkpoint 不兼容。**

## 0003 · 导出时丢弃 yolo26 的辅助头

yolo26 的 `head.o2m_*` 是训练辅助头：`forward()` 只走 one2one 单头，只有
`forward_train_dual` 用它。写进快照会让审计报 102 个 unexpected 并中止导出，
而任何推理图都不可能消费它们。

用 `.filter(PathFilter::new().with_predicate(|path,_| !path.contains("o2m_")))` 剔除。
注意 `PathFilter::new()` 是"不匹配任何路径"，`match_all` 状态下谓词会被忽略，
必须显式构造新 filter。

## 0001 · 导出时从模型读真实类别数

`ExportSpec::for_model` 的 `num_classes` 是写死的（分类 1000 / 语义 19 / 深度 1 / 其余 80），
而 Python 侧用它建图。结果是在自定义数据集上训练的模型**一律导不出 ONNX**：分类表现为
`2 wrong-shape keys`，因为重建了一个 1000 宽的分类头去装 3 类的权重。

改为从 `artifact_metadata(weights).class_names` 读真实类别数（仅 detect / segment / classify，
语义分割与深度不做覆盖，它们的输出宽度不是类别数）。

实测：`Overriding model.yaml nc=1000 with nc=3`，3 类分类模型导出成功并通过 ONNX Runtime 校验。
详见 `docs/DECISIONS.md` D026。

## 0004 · 免 Python 的 ONNX 导出（`export --template`）

给 `OnnxExportOptions` 加 `template: Option<PathBuf>` 与 CLI 的 `--template <目录>`。给了这个参数
就走**离线路径**：不解析 Python、不找 ultralytics 源码、跑完不进 `validate_options`，直接把预生成
骨架填成完整模型。

- `src/export/pb.rs`：手写 protobuf 编解码。只重编码认识的字段，未知字段原样保留；写法保守
  （尺寸/字符串按长度前缀、整数按 varint），够用来改 `TensorProto`、`ValueInfoProto` 和
  `AttributeProto`。
- `src/export/template.rs`：读 `<model>.json` → 复用 `snapshot::write_snapshot` 拿到按 keymap
  改好名的权重 → 逐初始化器写 `raw_data` → 平移输出维度 → 改 `Constant` 值与整数属性。
  Conv+BN 融合、DFL 常量、`group` 取值都在这里处理，细节见 `docs/DECISIONS.md` D027。

**必须在 `--features training,onnx` 下编译。** 生成的骨架由 `tools/onnx_skeleton/generate_all.py`
产出，和这个补丁是配套的：骨架清单的字段（`source`/`class_dependent`/`scalars`/`attributes`）
就是这里读的契约。

## 0005 · 机器可读事件 + n 档分类塔宽允许重排

这个补丁包含两件事，必须一起说明。

### (a) `src/training/events.rs`：此前遗漏的补丁，本次补入

`events.rs`（JSONL 训练事件）和 `runtime.rs` 里的两处 `events::emit` 是更早一轮做 UI 实时曲线时
加进上游源码的，**但当时没有生成补丁**。后果是：按本 README 重新构建会得到一个不发事件的引擎，
而 `desktop` 侧完全依赖 `{"type":"start"|"epoch"|"end"}` 这几行渲染进度——UI 会一直停在
"等待第一轮"，且很难联想到是引擎少打了一个补丁。

教训：`research/` 被 gitignore，改完上游第一件事就是出补丁，否则等于没改。

### (b) `transfer_pretrained` 允许分类塔宽随类别数变化

`Detect` 的分类塔宽是 `max(ch[0], min(nc, 100))`。只有 `n` 档的 `ch[0] = 64` 低于 100，
所以只有 n 档会出现"塔宽跟着类别数走"：80 类的检查点塔宽 80，3 类模型塔宽 64，
`cls_0`/`cls_dw_0` 这些卷积**形状不同，结构上就搬不过去**。

原实现只允许 `.cls_out.`（投影层）被替换，于是 80 类 → 3 类的微调在 n 档直接失败：

```
pretrained transfer differed outside the documented class projections:
  ... 30 shape errors
```

改法：把落在分类塔内的 `ShapeMismatch` 从 `errors` 里摘出来，记为"新初始化"，
其余（区块、主干、任何其它形状不符）仍然致命。这与 Ultralytics 的 `intersect_dicts`
行为一致——形状不符的键直接丢弃。同时加了 `ReplacedProjection::tolerates_tower_resize()`，
只有 `Detector`/`Yolo26Segment` 允许；分类头的 `head.linear` 没有塔，YOLOX/YOLOv3 的类别分支
命名不同（`head.cls_preds.N`），都不放行。

**安全性**：拿错架构的权重会连主干一起不匹配，仍然报错，不会静默加载出一个半随机模型。

实测（yolov8n，60 张 3 类图，320 输入，30 轮）：
- 修复前：`--model yolov8n.bpk` 直接失败
- 修复后：`Initialized 291 tensors ... freshly initialized 6 class-projection tensors and 6
  classification-tower tensors`，mAP 3.22%，对照"从零"为 0.013%
