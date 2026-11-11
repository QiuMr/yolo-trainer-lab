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

## 0001 · 导出时从模型读真实类别数

`ExportSpec::for_model` 的 `num_classes` 是写死的（分类 1000 / 语义 19 / 深度 1 / 其余 80），
而 Python 侧用它建图。结果是在自定义数据集上训练的模型**一律导不出 ONNX**：分类表现为
`2 wrong-shape keys`，因为重建了一个 1000 宽的分类头去装 3 类的权重。

改为从 `artifact_metadata(weights).class_names` 读真实类别数（仅 detect / segment / classify，
语义分割与深度不做覆盖，它们的输出宽度不是类别数）。

实测：`Overriding model.yaml nc=1000 with nc=3`，3 类分类模型导出成功并通过 ONNX Runtime 校验。
详见 `docs/DECISIONS.md` D026。
