# YOLO Trainer

在本机训练 YOLO 模型的桌面应用：**单个安装包、不装 Python、不配 CUDA，任意显卡都能跑**。

界面是 Tauri 2 + React，训练引擎是独立进程里的 [montgomery](https://github.com/boquila/montgomery)（Rust + Burn）。两者只通过 stdout 的 NDJSON 通信。

![状态](https://img.shields.io/badge/平台-Windows%20x64-111) ![状态](https://img.shields.io/badge/安装包-~29%20MiB-111)

---

## 能做什么

| 能力 | 范围 |
| --- | --- |
| 任务 | 目标检测、实例分割、图像分类 |
| 模型 | 7 个家族 × 规模档，共 **63 个可训练模型**（另有 10 个 ID 只能推理） |
| 数据集 | YOLO `dataset.yaml` + `images/`/`labels/`；ImageNet 风格 `train/<类名>/`（分类） |
| 设备 | 自动探测显卡，失败/强制时回退 CPU；CPU 是正式支持的路径 |
| 产物 | `exports/best.bpk`、`exports/last.bpk`、`results.csv`、验证指标 |
| 验证 | 训练完选一张图直接推理，看标注图与检出列表 |
| 自动更新 | GitHub Releases + minisign 签名校验 + NSIS 静默升级 |
| AI 操控 | 本地 HTTP 服务（`127.0.0.1:47821`），带令牌，接口契约见 `docs/API.md` |

模型选择器把 63 个模型拆成「任务 → 家族 → 规模」三层，不需要背模型 ID。

## 安装与使用

从 [Releases](https://github.com/QiuMr/yolo-trainer-lab/releases/latest) 下载 `..._x64-setup.exe`，双击安装即可。装好后：

1. **数据集**页选择目录 → 自动扫描类别、图片数、标注覆盖率（分类数据集会显示各类分布）。
2. **训练**页选任务/家族/规模，设置轮数、batch、分辨率、设备 → 开始训练。
3. **监控**页看 loss 曲线与日志；结束后用「验证模型」跑一张图。
4. **设置**页可换引擎、改设备、开关 AI 服务、检查更新。

关掉窗口不会退出程序（缩到托盘），从托盘菜单退出。

## 从源码构建

前置：Windows x64、Node 22+、pnpm、Rust stable、WebView2（Win11 自带）、NSIS（Tauri 打包时自动下载）。

### 1. 取得引擎二进制

桌面壳把 `montgomery.exe` 作为 **externalBin** 随包分发，这个 137 MiB 的文件不在本仓库里（见 `.gitignore`）。

```bash
git clone https://github.com/boquila/montgomery.git
cd montgomery
cargo build --release --features training --target-dir <某处>/target-dist
cp <某处>/target-dist/release/montgomery.exe \
   <本仓库>/desktop/src-tauri/binaries/montgomery-x86_64-pc-windows-msvc.exe
```

本项目验证过的上游提交：`0b17bf9fa5be6fe0af7e2f99a15892c7d98e97be`（montgomery 0.1.0）。

### 2. 构建桌面壳

```bash
cd desktop
pnpm install
pnpm tauri build
```

调试时 `pnpm tauri dev`；只做前端可用 `pnpm dev`（`?demo=1` 提供假训练数据，`?page=xxx` 直达指定页）。

> **注意**：`cargo build --release` 产出的 `desktop.exe` 不是可分发版本——它没有嵌入前端资源，启动会去连 `localhost:1420`。要分发必须用 `pnpm tauri build`。

### 3. 发版

```powershell
powershell -File scripts/release.ps1 -Notes "修了分类数据集扫描"
```

脚本会校验工作区干净、tag 未占用、私钥与 externalBin 齐全，然后构建、生成 `release/latest.json`、push、建 Release、核对资产并补传。版本号**只**从 `desktop/src-tauri/tauri.conf.json` 读，程序内显示的版本由 `build.rs` 注入同一个值。

首次发版需要一对签名密钥（私钥不入库）：

```bash
npx tauri signer generate --ci --password "" -w "$USERPROFILE/.tauri/yolo-trainer.key"
# 把打印出来的公钥填进 tauri.conf.json 的 plugins.updater.pubkey
```

## 自动更新怎么工作

```
应用启动 → GET https://github.com/QiuMr/yolo-trainer-lab/releases/latest/download/latest.json
        → 比对版本号 → 下载 setup.exe → minisign 验签 → NSIS /P 静默安装 → 自动重启
```

- feed 是**固定资产路径**，发新版不用改客户端任何代码。
- `latest.json` 由发版脚本生成，内含 `version / notes / pub_date / platforms.windows-x86_64.{url,signature}`。
- 安装包装的是 `currentUser` 模式；训练产物、数据集与设置都在 `%APPDATA%`，升级不覆盖。

发完版先跑一遍自检（不用启动应用）：

```bash
python spikes/qa/probe_update.py        # 12 项断言：feed、签名版本一致性、资产可下载
```

**为什么用官方 `tauri-plugin-updater` 而不是自己解压覆盖**：本项目发的是 NSIS 安装包，要维护注册表与卸载项；直接覆盖文件会留下指向旧版本的卸载信息，且 `externalBin` 引擎可能被占用。官方插件让真正的安装器接手，省掉了「更新助手进程 + 等 DLL 句柄释放」这一整类问题。

## 架构

```
montgomery.exe (AGPL-3.0, 独立进程)
      │ stdout NDJSON
      ▼
  engine.rs Hub ──┬──▶ RunState ──▶ HTTP 控制服务（AI 用）
                  └──▶ Tauri 事件 ──▶ React 界面（人用）
```

引擎**不是链接进来的库**：既保持 AGPL 隔离，也让显卡驱动崩溃只杀死子进程。两个前端共用同一个 `Runtime`，不会看到互相矛盾的状态。

细节见 `docs/ARCHITECTURE.md` 与 `docs/DECISIONS.md`。

## 许可与第三方组件

- **本仓库的桌面壳代码**：公开可见，但尚未指定开源许可证，默认保留所有权利。对外分发前需要明确这一点（见 `docs/STATUS.md` 待办）。
- **montgomery.exe**：**AGPL-3.0**，未经修改，源码与完整许可文本在 <https://github.com/boquila/montgomery>（LICENSE 文件）。随安装包分发该二进制时，用户可依上述地址取得对应源码；本项目验证的版本对应提交 `0b17bf9`。
- **Inter / Noto Sans SC 字体**：本地打包的 Google Fonts 子集，SIL Open Font License 1.1。
- 训练引擎内部依赖（Burn、cubecl 等）的许可由上游仓库声明。

## 已知边界

- 本机核显（Vega 6）上 GPU 后端不可靠，**GPU 训练尚未在独显机器上验证**；CPU 全链路已实测通过。
- 不能直接读 Ultralytics 的 `.pt`；从零训练是零配置的，用官方预训练权重需要 Python 侧转换。
- COCO JSON 数据集引擎支持，但扫描器未接入，界面里不能用。
- 分割与检测共用 `*.txt` 标签，扫描器不解析内容，格式不匹配由引擎报错提示。
- 无 AMP/FP16、无多卡，长程大规模训练不是这个引擎的赛道。

## 文档索引

| 文件 | 内容 |
| --- | --- |
| `docs/STATUS.md` | 当前进度、实测数据、复现命令、待办 |
| `docs/DECISIONS.md` | 架构决策（ADR，含被否决的方案） |
| `docs/API.md` | CLI 参数、控制服务与更新接口的契约 |
| `docs/ARCHITECTURE.md` | 模块职责、数据流、引擎能力边界 |
| `docs/CHANGELOG.md` | 变更记录 |
