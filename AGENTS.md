# YOLO Trainer 开发指引（写给 agent 会话）

Windows 桌面应用。Tauri 2 壳 + React 19 前端 + 一个 Rust 引擎 sidecar。

引擎是上游 `boquila/montgomery`（AGPL-3.0），**源码不在本仓库内**：`research/montgomery/`
被 gitignore，对它的改动以 `patches/*.patch` 为唯一准绳。改引擎而不出补丁，等于没改。

## 仓库地图

| 路径 | 是什么 | 改它要连带改什么 |
| --- | --- | --- |
| `desktop/src/pages/` | 四个页面：数据集 / 训练 / 监控 / 设置 | — |
| `desktop/src/lib/models.ts` | **模型目录唯一来源**（族 × 尺度 × 任务） | 与引擎 `ModelId` 一一对应，改它要重核 `resolveSelection` |
| `desktop/src/lib/api.ts` | 前端 ↔ Tauri 命令的边界 | 新命令要在这里加包装和浏览器回退 |
| `desktop/src/lib/store.ts` | 运行态（事件折叠后的快照） | 新事件字段要同时改 `runstate.rs` |
| `desktop/src-tauri/src/runtime.rs` | 引擎子进程 + 事件折叠 + 状态机 | — |
| `desktop/src-tauri/src/control.rs` | 手写 HTTP 控制服务（`127.0.0.1:47821`） | **必须同步 `docs/API.md`**：UI 与 AI 客户端都照它写 |
| `desktop/src-tauri/src/engine.rs` | 引擎路径解析、spawn、设备探针 | — |
| `desktop/src-tauri/src/dataset.rs` | 数据集扫描与格式判定 | 文案在 `models.ts::taskProblem`，判定在这里 |
| `desktop/src-tauri/src/predict.rs` | 推理包装（强制隔离输出目录） | — |
| `desktop/src-tauri/src/updater.rs` / `version.rs` | 自动更新与版本注入 | — |
| `patches/` | 对引擎的全部补丁（0001–0006） | `patches/README.md` 记了应用顺序与已知缺陷 |
| `spikes/qa/` | 真机验证脚本（探针、截图、更新校验） | — |
| `docs/` | 五份必更文档，见「文档」一节 | — |

## 三条唯一来源，违反必出 bug

1. **版本号**只存在于 `desktop/src-tauri/tauri.conf.json`。`build.rs` 注入 Rust，CLI 排安装包名，
   `scripts/release.ps1` 打 tag。**不要用 `CARGO_PKG_VERSION`**——它读 `Cargo.toml`，两处会漂。
2. **模型名**只存在于 `desktop/src/lib/models.ts`。页面里不许出现 `"yolo26n"` 这种字面量，
   一律走 `architectureId()` / `resolveSelection()`。三级选择器存的是**原始选择**不是校验结果，
   失效组合由 `resolveSelection` 兜底，不要在页面里各自校验。
3. **训练传 `DatasetSummary.manifest`（yaml 绝对路径），不要传 `root`**。`path:` 指向别处时
   根目录下没有 yaml，引擎会找不到数据集。

另外：任务与数据集格式是**一对一强制匹配**（检测/分割 → `yolo`，分类 → `imagenet-folders`），
判定在 `dataset.rs`，不匹配时禁用启动并说明该换哪一个。

## 改哪里跑什么检查

```console
# 前端（任意 desktop/src 改动）
cd desktop && npx tsc --noEmit && npm run build

# 壳（桌面端 Rust）
cd desktop/src-tauri && cargo test --target-dir ../../artifacts/desktop-target

# 引擎（research/montgomery）
cd research/montgomery && cargo test --features training --lib
```

**壳的测试必须带 `--target-dir`。** 本机继承了 `CARGO_TARGET_DIR=D:\rust-target` 这个旧环境变量
（用户级变量已删，但运行中的会话仍继承），不带就会把产物写到 D 盘、和 `binaries/` 里的 sidecar
对不上，构建看着成功、跑的却是旧引擎。

引擎改完要重建并同步 sidecar：

```console
cd research/montgomery && cargo build --release --features training --bin montgomery
cd ../.. && ./scripts/sync-engine.ps1     # 或用 sidecar 的固定名手动拷贝
```

发版一律走脚本，不手工改版本号、不手工传资产：

```console
powershell -File scripts/release.ps1 -Notes "..."
python spikes/qa/probe_update.py          # 发完立刻跑，12 项断言
```

## 项目特有的坑（都是踩过的，不是理论风险）

- **打包会假失败**：`safe-delete` 拦截「一次删 >50 个文件」，而 vite 清 `dist/assets` 要删 110 个。
  构建前先硬删 `desktop/dist`（`[System.IO.Directory]::Delete($p,$true)`），否则构建中断但退出码未必明显。
- **`.ps1` 必须 UTF-8 带 BOM + CRLF**。PowerShell 5.1 对无 BOM 的脚本按 GBK 解码，中文注释被撕碎、
  引号错位，报一堆「意外的标记」。用 Edit 工具改过 `.ps1` 之后要重新转一次编码。
- **`cmd 2>&1 | tail` 会吞掉真实退出码**。曾把 segfault（exit 139）看成 exit 0。判断成败要把输出
  重定向到文件再读 `$?`，不要接管道。
- **这台开发机不能训练 GPU**：Ryzen 5 5560U / Vega 6 核显，Vulkan 下设备探针就失败，DX12 下能过探针
  但训练在 cubecl autotune 层段错误退出。所以任何「GPU 能跑 / 速度快」的结论**必须换一台 N 卡机器
  实测**，不能在本机外推。`--device auto` 的探针回退是对的，不要改。
- **设备探针的 `backend` 在未指定时是 `auto`，不要从指纹猜**。`spirv_4098_5688` 在 Vulkan 和 DX12 的
  SPIR-V 路径下都会出现，本机 DX12 探针输出的正是它。
- **`patches/*.patch` 目前不能一次 apply**：0001 与 0004 都在 `ExportSpec::for_model` 附近落 hunk，
  普通 `git apply` 会失败，只能用 `--3way` 且会在 `export/mod.rs` 留冲突待人工合并。详见 `patches/README.md`。
- **MSYS 的 `/tmp` 与 Windows 原生 git 的 `/tmp` 不是同一个目录**。用 `git diff --no-index` 造补丁时，
  两边路径必须统一，否则 git 报「文件不存在」。
- **`.workbuddy/memory/` 是项目数据，不是缓存**，不要删。

## 许可边界

引擎是 AGPL-3.0，UI 通过**子进程**调用它（sidecar），**不静态链接**——这样 UI 可闭源，且引擎崩溃
不牵连界面。任何「把引擎编进主程序」的方案都会破坏这条边界，不要提。

## 文档是硬性要求

改完代码必须更新文档，否则下一个会话只能重新读代码。

| 文件 | 写什么 | 什么时候必须更新 |
| --- | --- | --- |
| `docs/STATUS.md` | 当前进度、已验证/未验证、实测数据 | 每次跑通或推翻一个结论 |
| `docs/DECISIONS.md` | ADR 风格决策，**含被否决的选项和原因** | 每做一个不可逆选择 |
| `docs/API.md` | CLI 参数与 HTTP 契约，精确到字段类型与默认值 | 任何参数/输出变更 |
| `docs/ARCHITECTURE.md` | 模块职责与数据流 | 新增或拆分模块 |
| `docs/CHANGELOG.md` | 日期 + 改了什么 + 为什么 | 每次有实质改动 |

写作要求：记「为什么」不记「是什么」；实测数据必须标注环境（机器、后端、规模）；不要记过程性
调试信息；计划过但决定不做的项**必须记下来并写明原因**，不能默默丢掉。
