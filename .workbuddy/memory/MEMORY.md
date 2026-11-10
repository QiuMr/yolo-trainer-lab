# yolo-trainer-lab 项目约定

## 文档是硬性要求（用户明确强调）

**改完代码必须更新文档，否则下次只能重新读代码。**

### 目录约定
| 文件 | 内容 | 什么时候必须更新 |
| --- | --- | --- |
| `docs/STATUS.md` | 当前进度、已验证/未验证、实测数据 | 每次跑通或推翻一个结论 |
| `docs/DECISIONS.md` | 选型与架构决策（ADR 风格，含放弃的方案） | 每做一个不可逆选择 |
| `docs/API.md` | CLI 参数与输出格式契约 | 任何参数/输出变更（UI 依赖它） |
| `docs/ARCHITECTURE.md` | 模块职责、数据流 | 新增或拆分模块 |
| `docs/CHANGELOG.md` | 变更记录（日期 + 改了什么 + 为什么） | 每次有实质改动 |

### 写作规则
1. **记「为什么」，不记「是什么」** —— 代码本身能看出是什么，看不出为什么。
2. **决策必须记被否决的选项和原因**（例：「评估过 DX12，但 burn-wgpu 无此 feature，实测挂起，放弃」）。
3. **接口契约必须精确**：参数名、类型、默认值、输出格式，UI 会照着它写。
4. **实测数据标注环境**（机器型号、后端、参数规模），否则数字会误导后续判断。
5. 不要记录过程性调试信息。

## 已知环境事实

- 本机：MECHREVO WUJIE 14 / Ryzen 5 5560U / Vega 6 核显（共享内存）/ 16 GB / Win11。
- **本机 GPU 不能用于验证**：Vega 6 上 `device lost`，训练 240s 跑不完一个 dry-run。
- 增量编译必须用 `--target-dir artifacts/target-dist` 复用依赖（改 montgomery 本体 2 分钟，改 cubecl-wgpu 约 8 分钟）。
- 工具链：Node 22.22.2 / pnpm 12.14.0 / rustc 1.99.1 / WebView2 已装。

## 代码约定

- **模型名只有一个来源**：`desktop/src/lib/models.ts`。页面里不许再出现 `"yolo26n"` 这类字面量，
  一律走 `architectureId()` / `resolveSelection()`。引擎侧 `ModelId` 是真理，这份目录与它一一对应
  （73 个 ID 里 63 个可训练，sem/depth 各 5 个只能推理）。
- 三级选择器存的是**原始选择**不是校验结果，失效组合由 `resolveSelection` 兜底。
- **任务与数据集格式是一对一强制匹配**（检测/分割 → `yolo`，分类 → `imagenet-folders`），
  判定在 `dataset.rs`，文案在 `models.ts::taskProblem`。不匹配时禁用启动并说明该换哪一个。
- 训练传 `DatasetSummary.manifest`（yaml 绝对路径），**不要传 `root`**：`path:` 指向别处时
  根目录下没有 yaml。
- 推理一律走 `predict.rs`，它强制把标注图写到 `<输出目录>/predictions/`。引擎默认写在源图旁边，
  会污染数据集。

- **版本号只有一个来源**：`desktop/src-tauri/tauri.conf.json` 的 `version`。`build.rs` 注入 Rust，
  CLI 命名安装包，`scripts/release.ps1` 打 tag。不要用 `CARGO_PKG_VERSION`（读 Cargo.toml，会漂）。
- **发版签名私钥**在 `%USERPROFILE%\.tauri\yolo-trainer.key`，不入库；公钥写在 `tauri.conf.json`。
- **NSIS 更新产物是 `<setup>.exe` + `<setup>.exe.sig`，没有 `.nsis.zip`**；签名内容嵌进
  `latest.json`，Release 只传 2 个资产。`.sig` 不带版本 → 别开 `requireSignedVersion`。
- `scripts/*.ps1` 必须 UTF-8 BOM + CRLF（PowerShell 5.1 按 GBK 解无 BOM 文件）。

## 当前架构决策（详见 docs/DECISIONS.md）

- 主引擎用 **montgomery**（AGPL-3.0，许可与体积用户已接受）。
- **UI 通过子进程调用 montgomery.exe（sidecar），不静态链接** —— 保持 AGPL 隔离，UI 可闭源，且 GPU 崩溃不牵连 UI。
- 前端：**Tauri 2 + React 19 + TypeScript + Vite**。
- CPU 是**正式支持的降级路径**，不是临时调试手段。
- 自动更新用 **tauri-plugin-updater + NSIS 安装器**（不照搬 OCR-Studio 的 zip 覆盖：注册表、
  卸载项、externalBin 占用三者都不允许）。GitHub 仓库必须 **public**，否则客户端匿名下载不到
  Release 资产（D020/D021）。
- 发版一律走 `scripts/release.ps1`，不手工改版本号、不手工传资产。
