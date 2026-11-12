# 架构

## 进程结构

```
┌────────────────────────────────────────────┐
│  Tauri 应用 (desktop/)                     │
│                                            │
│  React UI ──invoke──▶ #[tauri::command] ─┐ │
│                                          │ │
│  AI / 脚本 ──HTTP──▶ control (127.0.0.1) ┤ │
│                                          ▼ │
│                                   Runtime  │
│                    （settings / engine /   │
│                      RunState / control）  │
│                          │                 │
└──────────────────────────┼─────────────────┘
                           │ spawn + stdout/stderr 管道
                           ▼
                  montgomery.exe（独立进程，AGPL-3.0）
                           │
                    ┌──────┴──────┐
                    ▼             ▼
              stdout: NDJSON   stderr: 人类日志
                    │             │
                    └──▶ hub ◀────┘
                          │
          ┌───────────────┴───────────────┐
          ▼                               ▼
   RunState（服务读）            webview 事件（界面读）
```

**两个前端，一份实现**：窗口和 HTTP 服务都只调 `Runtime` 的方法，谁都不直接持有引擎。
所以"界面显示第 7 轮"和"`/state` 返回第 7 轮"是同一份数据，不可能不一致（D011、D017）。

**为什么引擎是独立进程**：AGPL 传染（详见 DECISIONS.md D002）＋ 崩溃隔离。
GPU 驱动出问题只会杀掉引擎进程，壳活着并可以提示或重试。

---

## 前端（`desktop/src`）

| 文件 | 职责 |
| --- | --- |
| `index.css` | 设计 token：白/黑配色、Inter + Noto Sans SC（本地自带）、三级圆角、粗字重、三档动效时长 |
| `fonts.css` + `assets/fonts/` | 本地字体（237 个 woff2 子集），由 `spikes/fonts/fetch_fonts.py` 生成，不依赖任何 CDN |
| `components/TitleBar.tsx` | 自绘标题栏与窗口控制（无边框窗口，见 D005）。不含任何文字；左上角是侧栏收起按钮，其列宽跟随侧栏宽度动画；右上角除窗口控制外还有命令面板入口 |
| `components/Sidebar.tsx` | 导航 + 右边缘拖拽调宽，`useSidebar()` 承载其状态与持久化。导航项有 `pinned` 之分，设置固定在底部；**没有品牌块**，收起按钮在标题栏里。选中态是一块**移动的墨色药丸**，位置由「行与整列的 client rect 之差」算出——**不能用 `offsetTop`**，它相对最近的 positioned 祖先（每组的 `nav` / 底部容器）重新计数，会让药丸整体偏移并在选中底部项时飞回顶部 |
| `components/CommandPalette.tsx` | Ctrl+K 命令面板。所有页面与全局动作用一个数组描述（`PaletteAction`），将来接 AI 时同一份列表就是"可调用命令表" |
| `components/NumberField.tsx` | 训练参数输入：标签可左右拖动调值，失焦时按范围与步长归一化（D016） |
| `components/Motion.tsx` | `AnimatedNumber` / `Progress` / `Reveal` / `LiveDot`，只做"动效能表达信息"的那几件事 |
| `components/Toaster.tsx` | toast 栈（下拉或点 × 关闭），用于没有页面归属的回执 |
| `components/ui.tsx` | 基础控件：Button（必带图标）、Card、Field、Badge、Stat、SectionTitle、Kbd |
| `lib/api.ts` | 引擎桥接。**所有字段以 docs/API.md 为准**；浏览器环境返回样例数据以便预览 |
| `lib/store.ts` | 极简 pub/sub store（20 行），承载跨页面的训练状态与日志。日志行带单调 `id`，避免上限裁剪时整列表重挂载 |
| `lib/motion.ts` · `lib/gestures.ts` · `lib/dnd.ts` · `lib/toast.ts` · `lib/clipboard.ts` | 动效补间、指针手势、窗口级拖放、toast、复制（带 WebView2 兜底，见 D013/D014） |
| `lib/motion.ts` | `useTween`（数字补间）、`usePresence`（退出动画期间保持挂载）、`useReducedMotion` |
| `lib/gestures.ts` | `useSwipe` / `useLongPress` / `useScrub` / `useAutoFollow`，全部在 useEffect 内成对增删监听 |
| `lib/dnd.ts` | 窗口级文件夹拖放（挂在 App 层，见 D015） |
| `lib/toast.ts` | toast 的 pub/sub 状态 |
| `lib/demo.ts` | 浏览器预览用的假训练数据（`?demo=1`），保证监控页无引擎也能审阅 |
| `lib/models.ts` | 模型目录：3 个家族 × 3 个任务 × 5 个规模档（45 个），`architectureId()` 按引擎的 `ModelId` 规则拼 ID。**这是模型名字的唯一来源**，页面不允许再手写 `"yolo26n"` 字面量 |
| `components/PredictCard.tsx` | 训练完成后用一张图验证产物：调 `predict_image`，左侧标注图、右侧检出列表 |
| `components/UpdateCard.tsx` | 设置页「版本与更新」：挂载时静默检查 + 6 h 缓存、进度事件、一键下载安装；网络失败只显示浅色句子 |
| `App.tsx` | 外壳：标题栏 + 侧边栏 + 页面区；全局事件订阅、手势与快捷键、页面方向转场都在这一层 |
| `pages/DatasetPage.tsx` | 选目录 → 扫描 → 展示类别/图片数/标注覆盖率 → 下一步引导 |
| `pages/TrainPage.tsx` | 任务/家族/规模三级选择模型 → 参数 → 启动训练 → 跳转监控 |
| `pages/MonitorPage.tsx` | 实时 loss 曲线（ECharts）+ 指标卡 + 日志 + 错误码文案 |

**关键约定**（前端部分）：
- 状态放 store，不放页面。训练事件从 App 层订阅一次，任何页面都能读到。
- 组件不直接碰 `invoke`，一律走 `lib/api.ts`。
- 动效只用 `index.css` 里那三档时长（120/180/280），不允许出现第四个数字（DECISIONS D006）；
  唯一的例外是 `--ease-spring`，只给"位移物体"用（药丸、toast 回弹）。
- **每个手势都要有一个显式等价控件**（DECISIONS D014）。加手势时先问"没有触控板的人怎么办"。
- 手势 hook 里的监听一律在同一个 `useEffect` 内成对增删；不挂 `document` 上长期存活的匿名监听。
- `?page=xxx` 可直接打开指定页，`?sidebar=collapsed` 强制收起态，`?demo=1` 填假训练数据，
  `?demo=done` 填一份**已完成**的训练（唯一能看到「验证模型」卡片的状态），
  `?demo=classify` 让数据集页显示分类布局（两种布局渲染的不是同一张卡）。
  这些都仅用于截图与人工 QA，`demo` 在真实壳里被 `isTauri` 守卫掉。

### 动效分层

| 层 | 手段 | 例子 |
| --- | --- | --- |
| 入场 | CSS 关键帧 + `animation-delay` 错峰 | 页面转场、卡片、日志行、命令面板 |
| 状态变化 | CSS transition（120/180ms） | hover、按下、颜色、侧栏宽度 |
| 位移物体 | CSS transition + `--ease-spring` | 导航药丸、toast 回弹、勾选图标 |
| 数值 | `useTween` + rAF | loss、进度、耗时、剩余时间 |
| 进出场保留 | `usePresence` | 命令面板退出动画 |

原则：**能用 CSS 表达的不写 JS**（D013）。JS 只处理 CSS 表达不了的（数字）或需要延迟卸载的（退出）。

---

## 后端（`desktop/src-tauri/src/`）

模块边界是刻意的：**只有 `lib.rs`、`tray.rs` 与 `updater.rs` 碰 Tauri**，其余全是纯逻辑，
窗口与 HTTP 服务直接复用它们（DECISIONS D011）。`updater.rs` 是例外：它必须拿 `AppHandle`
才能发进度事件，且本身就是窗口专属能力。

| 模块 | 职责 | 依赖 Tauri |
| --- | --- | --- |
| `dataset.rs` | 读 `dataset.yaml`，判断布局（YOLO / 分类文件夹），统计图片与标注覆盖率 | 否 |
| `engine.rs` | 定位 exe、spawn、事件 fan-out（`Hub`）、退码回收 | 否 |
| `predict.rs` | 单图推理：spawn `montgomery predict`、从混杂输出里取 JSON、产物落到应用目录 | 否 |
| `settings.rs` | `settings.json` 读写与归一化（含控制服务的开关与端口） | 否 |
| `runstate.rs` | 把 hub 事件折叠成一份快照（status/epoch/loss/fitness/产物/错误），并给出增量事件与 `reset`/`truncated` 标记 | 否 |
| `runs.rs` | 扫 `runs/<task>/<run>/` 报告历史运行与可用产物（`/runs` 的实现；`/state` 重启即失忆，磁盘不会） | 否 |
| `runtime.rs` | 两边共享的状态：设置、引擎句柄、快照、控制服务句柄。**折叠线程在 `new` 里启动**：hub 是扇出结构，快照在这里更新，窗口与 AI 客户端因此看到的永远是同一份进度，而不依赖 UI 层线程是否存活 | 否 |
| `control.rs` | 本地 HTTP 服务：路由、鉴权、自描述文档 | 否 |
| `version.rs` | 版本号（`build.rs` 从 `tauri.conf.json` 注入）+ 数值化版本比较 | 否 |
| `pretrained.rs` | 预训练权重清单（编译进二进制）、按需下载、sha256 校验、缓存到 `%LOCALAPPDATA%`、发进度事件 | 部分（只用到 `app_local_data_dir` 与事件） |
| `updater.rs` | 查 GitHub feed、下载验签、发进度事件、拉起 NSIS 安装器 | 是 |
| `tray.rs` | 托盘图标、显示/隐藏、退出 | 是 |
| `lib.rs` | `#[tauri::command]` 薄壳、hub 订阅线程、进程生命周期 | 是 |

| 命令 | 作用 |
| --- | --- |
| `resolve_engine` | 按设置 → `MONTGOMERY_ENGINE` → exe 同目录 → 仓库构建产物 依次查找 |
| `scan_dataset` | 手写 YAML 解析 + 目录统计，返回格式/类别/图片数/缺标注数/警告 |
| `start_training` | 按设置取引擎路径与产物目录，spawn 引擎；输出经 `Hub` 分发 |
| `stop_training` | kill 子进程；`exit` 事件由 reaper 线程统一发出，停止与自然结束只有一条路径 |
| `predict_image` | 调 `predict.rs`，用当前 `RunState.best` 或显式指定的 bpk 推理一张图 |
| `get_settings` | 返回**已解析**的视图（引擎绝对路径、产物绝对路径、错误原因） |
| `save_settings` | 归一化 → 落盘 → 更新内存缓存 → （控制参数变了就重启监听）→ 回显视图 |
| `app_version` | 返回编译进去的版本号，由 `build.rs` 注入，前端不硬编码 |
| `pretrained_catalog` | 列出可下载的权重与本地缓存状态，**不联网** |
| `download_pretrained` | 取一个权重到本地（必要时下载+校验）；下载在独立线程，因为 reqwest blocking 不能在 async 运行时里驱动 |
| `check_update` | 查 GitHub feed；**永远成功**，网络/解析失败收敛进 `UpdateInfo.error` |
| `install_update` | 下载 → 验签 → NSIS `/P` 安装；Windows 上成功即退出进程 |

`Runtime` 是所有状态的唯一归属：`engine` 句柄、当前设置、`RunState`、`control::Handle`。
命令层只做「解参数 → 调方法 → 序列化结果」。这也是控制服务能直接复用同一套逻辑的原因——
它调的是同一批方法，不是另一份实现。

### 控制服务（`control.rs`）

| 关注点 | 做法 | 为什么 |
| --- | --- | --- |
| 并发模型 | 每个连接一个 `std::thread`，`Connection: close` | 客户端只有一个，异步运行时是纯负担（D017） |
| HTTP 解析 | 手写请求行 / 头部 / `Content-Length` | 端点少；14 KB 头的上限和一个 1 MB 的 body 上限就是全部防御 |
| 停止 | `AtomicBool` + 非阻塞 `accept` 轮询（120 ms） | 阻塞 `accept` 要停止得让服务自己连自己，零件更多 |
| 鉴权 | `Authorization: Bearer`，常量时间比较 | token 存在配置目录，重启不变，避免用户粘过的提示词失效 |
| 端口 | 默认 47821，占用时回退系统分配 | "端口被占"变成"换一个"，而不是功能悄悄失效 |

**事件流**：`engine.rs` 的 `Hub` 是唯一出口。`lib.rs` 在 `setup` 里订阅一次并转发成
`engine-event` / `engine-log` 两个 webview 通道。新增消费者（如本地服务）**必须**同样走
`engine.subscribe()`，不要再往 handler 里塞 `app.emit`。

**事件转发原则**：stdout 的 JSON 行**原样转发**，不重新解释字段。壳只额外加一个 `exit` 事件——
进程无法报告自己的退出码，这个信息只能由父进程补。

**生命周期责任**：引擎是独立进程，宿主必须替它收尸。应用退出时 `RunEvent::Exit` 会调用
`kill_engine`（`kill()` + `wait()`）。不做这一步，用户关掉界面后训练会继续在后台跑满 CPU，
而且再没有任何入口能停掉它——这是进程隔离架构必须自己付的成本。

注意"退出"指的是**进程退出**，不是关窗口：× 只隐藏窗口（D008），训练继续；只有托盘菜单的
「退出」才会走到 `kill_engine`。

### 窗口与进程（`src-tauri/src/tray.rs` + `run()`）

| 部件 | 行为 |
| --- | --- |
| 托盘图标 | 左键单击 → `show + unminimize + set_focus`；右键菜单「显示主窗口 / 退出」 |
| 关闭按钮 | `CloseRequested` → `api.prevent_close()` + `hide()`，进程存活（D008） |
| 单实例 | 重复启动只把已有窗口抬起来；插件**第一个注册**，否则新进程会晚退出而产生两个引擎 |
| 窗口状态 | `POSITION \| SIZE \| MAXIMIZED`，**不含 `VISIBLE`**（D009），由插件在 `RunEvent::Exit` 落盘 |

---

## 打包（引擎如何到达用户机器）

引擎作为 Tauri **sidecar** 随安装包分发：

```
desktop/src-tauri/binaries/montgomery-x86_64-pc-windows-msvc.exe   ← 源（构建时拷入）
        ↓  tauri build（externalBin: ["binaries/montgomery"]）
<安装目录>/YOLO Trainer.exe
<安装目录>/montgomery.exe        ← 去掉三元组，落在应用 exe 旁
```

这正好命中 `engine_candidates()` 里 `current_exe().parent().join("montgomery.exe")`，
所以**装完就能用，不需要用户配置路径**。开发态下 exe 同目录没有引擎，会继续往下走
仓库相对路径 `artifacts/target-dist/release/montgomery.exe` 兜底。

体积构成：引擎 137.4 MB + Tauri 壳约 10 MB（WebView2 复用系统自带，不计入）。
NSIS 压缩后安装包约 29 MiB。

---

## 自动更新

```
scripts/release.ps1                    GitHub Release
  ├─ 读 tauri.conf.json 的 version ─────▶ tag vX.Y.Z
  ├─ npx tauri build（带私钥）          ├─ YOLO Trainer_X.Y.Z_x64-setup.exe
  └─ release/latest.json ◀─────────────┴─ latest.json（含 .exe.sig 的内容）
                                              │
                          固定 URL：releases/latest/download/latest.json
                                              │ 匿名 GET
                                              ▼
                                    updater.rs / tauri-plugin-updater
                                      版本比较 → 下载 → minisign 验签
                                              │
                                      NSIS /P → /R 重启
```

| 层 | 归属 | 说明 |
| --- | --- | --- |
| feed URL / 公钥 | `tauri.conf.json` 的 `plugins.updater` | 一次写定，发版不改客户端 |
| 版本号 | `tauri.conf.json` | `build.rs` 注入 Rust，CLI 命名安装包，脚本打 tag（D022） |
| 签名私钥 | `%USERPROFILE%\.tauri\`，**不入库** | 泄漏等于任何人能发"通过校验"的假更新（D023） |
| 安装模式 | `plugins.updater.windows.installMode = quiet` | NSIS `/S` 完全静默；`passive`（`/P`）的进度窗口在受限会话里无法完成（D024） |
| 发版脚本 | `scripts/release.ps1` | 校验工作区/tag/产物 → 构建 → push → 建 Release → **核对资产并补传** |
| 客户端 | `updater.rs` + `UpdateCard.tsx` | 检查、进度、安装；失败一律收敛成可读句子 |

**信任边界**：feed 走 HTTPS 但本身不签名，签名只覆盖下载的安装包。所以插件先验签再运行安装器，
`install_update` 另做一次"远端版本必须比当前新"的显式判断，防止把旧 Release 的合法签名
配上一个虚高的版本号实现降级。`requireSignedVersion` **已启用**：CLI 2.12.4 的签名带
`version:` 字段，不带它的签名会被直接拒绝（D023）。

**为什么不是 zip 覆盖**：NSIS 安装写注册表与卸载项，且 `externalBin` 与 WebView2 DLL 在主进程
运行时被占用。详见 D020。

---

## 引擎能力边界（训练器视角）

上游 `research/montgomery` 的 `ModelId` 有 **73 个条目 / 7 个家族**，但"能推理"≠"能训练"。
下列结论来自 `src/training/dispatch.rs::recipe_for` 与 CLI `TrainArgs`，不是猜测。

### 可训练：63 个，其中产品提供 45 个

"引擎能训"和"产品提供"是两件事。D028 把界面和 ONNX 骨架一起收窄到 45 个，
引擎侧的 CLI 仍然认全部 63 个（没有改动，也不花体积）。

| 家族 | 规模档 | 检测 | 实例分割 | 分类 | 产品提供 |
| --- | --- | --- | --- | --- | --- |
| YOLO26 | n/s/m/l/x | 5 | 5 | 5 | ✅ 15 |
| YOLO11 | n/s/m/l/x | 5 | 5 | 5 | ✅ 15 |
| YOLOv8 | n/s/m/l/x | 5 | 5 | 5 | ✅ 15 |
| YOLOv10 | n/s/m/b/l/x | 6 | — | — | ❌ 免 NMS 已被 YOLO26 覆盖 |
| YOLO12 | n/s/m/l/x | 5 | — | — | ❌ 注意力架构，社区零采用 |
| YOLOX | nano/tiny/s/m/l/x | 6 | — | — | ❌ 异形档位 + 非 ultralytics 血统 |
| YOLOv3 | tinyu | 1 | — | — | ❌ 上游标注为"对照基线" |

### 只能推理：10 个

`yolo26{n,s,m,l,x}-sem`（语义分割）与 `yolo26{n,s,m,l,x}-depth`（单目深度）在
`recipe_for` 里保留了真实几何参数，但注释写明 **inference-only，native trainer 会在建损失前拒绝**。
原因是没有对应的损失与标注管线（语义掩膜/深度图的训练数据组织方式与检测/分割/分类都不同）。

### 三种数据集格式（`DatasetFormat`）

| 格式 | 约定 | 适用任务 |
| --- | --- | --- |
| `yolo` | `dataset.yaml` 的 `train`/`val` 指向图片目录，标签在旁的 `labels/*.txt`；缺失标签文件 = 合法背景图 | 检测 / 实例分割（polyline 标签） |
| `coco` | `train_annotations` 等字段指向 COCO JSON；mask 支持 polygon 与 RLE 两种 | 检测 / 实例分割 |
| `classification-folders` | ImageNet 风格 `train/<类名>/*.jpg`，`names` 可省略（从目录名推断） | 分类 |

`format` 省略时引擎会嗅探，COCO 必须显式给每段 split 的标注文件。
**本项目 UI 目前只产出/扫描 `yolo` 格式**（`src-tauri/src/dataset.rs` 里 `format: "yolo"` 写死），
COCO 与分类文件夹是引擎已有、产品未接的能力。

### UI 的开放范围

选择器把 **45 个**模型开放（引擎能训 63 个，D028 砍掉了 18 个），组织方式见
`desktop/src/lib/models.ts`：

| 任务 | 家族数 | 模型数 | 数据集格式 | 链路 |
| --- | --- | --- | --- | --- |
| 检测 | 3（YOLO26 / YOLO11 / YOLOv8） | 15 | `yolo` | 通 |
| 实例分割 | 3（YOLO26 / YOLO11 / YOLOv8） | 15 | `yolo` | 通（同一套 `labels/*.txt`，多边形标签由引擎解析） |
| 图像分类 | 3（YOLO26 / YOLO11 / YOLOv8） | 15 | `imagenet-folders` | 通（`train/<类名>/`，类名即标签） |

**任务与格式是一对一强制匹配**，不是建议：`taskProblem()` 在两者不匹配时给出具体原因（该换
任务还是该换数据集），并禁用启动。三种格式的组合由 `dataset.rs` 判定，判定依据是磁盘内容
而不是 yaml 里写了什么——yaml 可以省略 `format`。

分类的类名可以省略 `names`（从 `train/` 子目录名推断），但**写了就必须与目录名完全一致**，
因为引擎按 `split.join(name)` 找目录。扫描器提前校验这一点，把一次跑到一半的失败变成页面上
的一句话。

分割与检测共用标签文件，扫描器无法从文件内容区分 bbox 与多边形；**标注格式是否匹配交给引擎校验**，
不匹配时引擎的报错自带 hint，会显示在页面的错误区。

### 产物与格式

| 用途 | 格式 | 说明 |
| --- | --- | --- |
| 训练初始化 | `--architecture <id>` | 从零开始，无外部文件 |
| 训练初始化 | `--model *.bpk` | Montgomery 原生 Burnpack（半精度存储） |
| 断点续训 | `--resume checkpoints/last` | 完整原生检查点，**不等于** bpk；含优化器与数据配置 |
| 导出 | `exports/best.bpk`、`exports/last.bpk` | 每次 run 自动产出 |
| 导出 | `.onnx`（`export` 子命令） | 需要单独编译 `onnx` feature，且依赖 ONNX Runtime 校验 |
| 追加产物 | `results.csv`、`results.svg`、`validation.jsonl` | 每次 run 都有 |

### 关键断点：`.pt` 不能直接吃

Ultralytics 的 `.pt` 必须先经 `tools/export_checkpoint_state.py`（Python + PyTorch + ultralytics）
导成 tensor-only state，再用 `montgomery pack-weights --architecture <id> --state xxx.pt` 打包。
**这是"零配置"承诺唯一的破口**：从零训练完全零配置，但"用官方预训练权重起步"需要一次 Python 转换。
产品上要么避开这个卖点，要么后续做一个转换引导页。


---

## 设计系统（用户明确要求）

| 项 | 约定 |
| --- | --- |
| 主色 | 纯白 `#FFFFFF`，黑色只作强调与选中态 |
| 文字 | `#0A0A0A`，不用纯黑（白底纯黑会发颤） |
| 字体 | Inter（拉丁）+ Noto Sans SC（中文），来自 Google Fonts |
| 字重 | 标题 700–800，正文 500–600。层级靠字重，不靠字号 |
| 圆角 | 卡片 14px，控件 10px，标签 8px。只用这三档 |
| 图标 | lucide-react，`strokeWidth` 2.3–2.6（配合粗字重）。**每个按钮都必须有图标** |
| 状态色 | 只在成功/警告/错误时出现，不做装饰 |

---

## 已知风险

1. **推理输出会污染数据集目录**：`predict` 默认把 `xxx-detections.png` 写在源图旁边。
   如果数据集目录被直接用于推理，下次扫描会把它当训练图片（已在本项目中真实发生过一次）。
   → **已修**：`predict.rs` 强制 `--output <输出目录>/predictions/xxx-predicted.png`，
   调用方无法绕过。
2. **WebView2 版本差异**：老 Win10 可能缺 WebView2 或版本过旧，需要安装包引导。
3. **ECharts 全量引入**：JS 约 1.37 MB（gzip 450 KB）。桌面应用可接受，若在意可改按需引入。
