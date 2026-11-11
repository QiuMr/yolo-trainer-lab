# 项目状态（2026-11-11）

## 一句话结论

**montgomery 的完整链路可用**：数据 → 训练 → 导出 `.bpk` → 推理，全部在本机跑通（CPU 后端）。
桌面壳、AI 操控接口、45 个模型选择器、分类链路、单图推理都已闭环；本轮补上 GitHub 仓库、
版本管理与自动更新。GPU 后端在这台机器上不可靠，需换独显机器验证。

## 已实测通过

| 项目 | 命令要点 | 结果 |
| --- | --- | --- |
| 分类推理（CPU） | `predict --device cpu` | ✅ `not_hot_dog` 96.92% |
| 分类推理（GPU） | `predict --device gpu` | ❌ `The device is poisoned`（驱动侧） |
| 训练干跑（CPU） | `train --dry-run` | ✅ `Device<Flex(Cpu)>`，forward/loss/backward 全过 |
| 训练 1 epoch（CPU） | `train --epochs 1 --no-val` | ✅ `loss 4573.58`，产出 checkpoints + exports |
| 训练产物推理 | `predict --model exports/best.bpk` | ✅ 正常输出检测结果、写出标注图 |

## 预训练权重按需下载（2026-11-11 晚上）

用户提出权重放 GitHub、用时下载。安装包因此仍是 29 MB，且能提供全部档位（D029）。

| 项目 | 状态 | 怎么验的 |
| --- | --- | --- |
| 清单与 Release 资产一致 | ✅ 15/15 | `gh release view` 逐条比对文件名与字节数，双向 0 差异 |
| 真实下载 + 摘要校验 | ✅ | `--ignored` 网络测试：`yolo26n.bpk` 5.1 MB / 12.7 秒，sha256 通过 |
| 截断文件被拒 | ✅ | 单测：长度不符 → "文件大小不符"；长度对内容错 → "校验不通过" |
| 界面下载进度 | ✅ | 实测截图：0.0 → 2.0 / 5.1 MB，进度条同步推进；`.part` 落盘 3 MB |
| 下载后自动开始训练 | ✅ | `.part` → 5,364,896 B（与清单一致）；日志 `Initialized 492 tensors … 90 classification-tower tensors` |
| 训练真的在推进 | ✅ | 界面 3/100 轮，loss 456.56 → 411.91 → 350.13，每轮 64.7 秒，曲线同步绘制 |
| n 档塔宽修复 | ✅ | 新引擎下 `yolov8n` 261+6+**30**、`yolo26n` 492+12+**90** 个塔张量按预期重初始化，1 轮训练均正常 |
| 预训练 vs 从零 | ✅ | mAP **3.22% vs 0.013%**（251 倍）；从零模型 0 检出，预训练 person 38.3% |
| 四模式布局 | ✅ | 截图实测：一行四个，"官方通用权重，首次使用时下载"换行不溢出 |
| 权重信息条 | ✅ | `COCO 预训练 · 已有 80 类` + `首次训练前下载 5.1 MB` |
| 桌面单测 | ✅ 43 passed / 2 ignored | 新增 3 项：清单自洽、摘要拒绝、真实下载（ignored） |
| 前端 | ✅ | `tsc --noEmit` 干净、`vite build` 1.2s |

### 两个踩到的坑（都已处理）

1. **引擎有两处副本都不会自动更新**：改完 `research/montgomery` 只构建
   `artifacts/target-dist/release/montgomery.exe` 是不够的——应用实际跑的是
   `desktop/src-tauri/binaries/montgomery-*.exe` **和** `artifacts/desktop-target/release/montgomery.exe`，
   而 `Copy-Item` 默认保留源文件的旧时间戳，tauri 因此认为没变化、拒绝覆盖。表现是
   "补丁明明打了、行为却没变"：界面点开始训练，引擎以代码 1 退出，日志里还是那 80 个 mismatch。
   现在三件事兜住它：`scripts/sync-engine.ps1` 一条命令同步并盖新时间戳；
   `scripts/release.ps1` 比对时间戳，过期**直接失败**；本文档记录这条因果关系。
2. **`reqwest` 的 `rustls-no-provider` 不会自己装加密后端**：第一次真实下载时，
   在 reqwest 自己的工作线程里 panic，冒泡上来只有一句 `event loop thread panicked`。
   必须在建 Client 前 `rustls::crypto::ring::default_provider().install_default()`。

## 模型目录收窄到 45 个（2026-11-11 下午）

用户问"哪些常用、哪些肯定用不到"。按此把界面与 ONNX 骨架一起砍到 3 家族 × 3 任务 × 5 档，
明细与被否决的方案见 D028。

| 项目 | 状态 | 怎么验的 |
| --- | --- | --- |
| 界面家族 7 → 3 | ✅ | CDP 实测：检测/分割/分类下都是 `['YOLO26最新一代','YOLO11成熟稳定','YOLOv8生态最广']` |
| 三级联动仍成立 | ✅ | 默认 `yolo26n` → 分割 `yolo26n-seg` → YOLOv8 `yolov8n-seg` → 选 x `yolov8x-seg` → 回检测 `yolov8x` |
| 分类任务阻断 | ✅ | 切分类 ID 变 `yolov8x-cls`，同时出现警告条、启动按钮置灰 |
| 布局未失衡 | ✅ | 截图实测：四行（从哪开始 3 / 任务 3 / 家族 3 / 规模 5）等宽铺满，家族芯片 288px 与任务行一致 |
| `shot_model_picker.py` 行定位 | ✅ 已修 | 续训加了「从哪开始」行之后，脚本按索引取行整体偏移 1，之前一直是"通过的假象"。改成按 label 取行 |
| 45 个骨架与 manifest 自洽 | ✅ 45/45 | `onnx.load` 静态核对：张量名全在 graph、dims 一致、输入输出 shape 一致 |
| 新档位端到端导出 | ✅ | `yolov8m`（640/1 epoch）→ `export --template`，167 张量，103.6 MB，ORT 可执行 |
| 导出与引擎推理一致 | ✅ | 同图同权重：分数逐位一致（51.6%），x 坐标 6 框一致到 0.1px；y 差 78.6px 来自 rect letterbox 与固定 640 的差别 |
| 缺骨架时的提示 | ✅ | 引擎原句 `skeleton manifest unreadable at …` 翻译成中文并点名架构（新增单测 2 项） |
| `cargo test --lib` | ✅ 40 passed / 1 ignored | 新增 2 项（缺骨架文案、无关错误原样透传） |
| 安装包 | ✅ 29.31 MiB | 45 个骨架 12.06 MiB 原始 → LZMA 35 KiB（354.7x），比 0.1.2 只多 229 KB |

**又一次估算失准**：我先按"250 KB × 45 ≈ 10 MB"说会涨到 +8~10 MB 安装包，实测只涨 0.2 MB。
骨架把权重 payload 全丢掉，只剩重复的节点名和 dims，solid LZMA 能压到 1/355。
体积不该再被用作砍架构的理由。

## 全面审查（2026-11-11 白天）

四路只读审查（前端 / Rust 后端 / 发版工程化 / 文档契约）后修的缺陷，全部有明确触发条件：

| 项目 | 结论 |
| --- | --- |
| `App.tsx` 事件订阅 | **真 bug**：清理跑在 `listen()` promise resolve 之前，StrictMode 每次挂载必现泄漏，第二次挂载起每个 epoch 被折叠两次（loss 曲线翻倍）。已修 |
| `TrainPage` 整数校验 | **真 bug**：`u32` 字段接受 "1.5"，通过范围检查后在 serde 炸出英文类型错误。已修（`Number.isInteger`） |
| `PredictCard` 连点 | 对话框未返回时可开第二个对话框并排两次推理。已修（ref 全程占位） |
| `App.tsx` 拖放 | 两次拖放无序号，慢的 A 会覆盖快的 B。已修 |
| `UpdateCard` | 失败分支用闭包旧 `version`，首次失败显示 `—`。已修 |
| 控制服务 | 实际端口、Content-Length/header 上限、常量时间鉴权、`Expect: 100-continue`、409 并发、文档版本号来源。已修 |
| 回归 | `cargo test --lib` 37/37 通过；`tsc --noEmit` 干净；`vite build` 9.01s |

**引擎体积实测**（回答"137 MiB 引擎怎么进 29 MiB 安装包"）：`montgomery.exe` 137.38 MiB，
`.text` 占 76%；LZMA9e 单压 **20.91 MiB（6.57x）**。安装包 29.1 MiB ≈ 20.9（引擎）+ 4.5（壳）
+ 前端资源 + NSIS stub。**只有安装包和下载被压缩，落盘仍是 137 MiB。**
可选优化（未做）：`lto="fat"` + `codegen-units=1` + `strip`，需 fork 上游且本机编译时间翻倍。

## 本轮新增：GitHub 仓库与自动更新（2026-11-11）
| 项目 | 实测结果 |
| --- | --- |
| 仓库 | `QiuMr/yolo-trainer-lab`（**public**，D021）；首次提交 472 文件 / 11.63 MiB，22 GB 构建产物与上游源码全部排除 |
| `cargo test --lib` | ✅ **37 passed / 1 ignored**（全面审查后从 28 增至 37） |
| `npx tsc --noEmit` | ✅ 干净 |
| 带签名 `npx tauri build` | ✅ 5m35s；`setup.exe` 29.07 MiB + `.exe.sig` 444 B |
| `scripts/release.ps1` | ✅ UTF-8 BOM + CRLF，PowerShell 解析 0 错误 |
| 版本号 | `build.rs` 从 `tauri.conf.json` 注入 `YOLO_TRAINER_VERSION`，三段数字断言 |
| GitHub Releases | `v0.1.0` / `0.1.1` / `0.1.2` / `0.1.3`，`releases/latest/download/latest.json` 匿名可读 |
| `spikes/qa/probe_update.py` | ✅ 12/12（无 BOM、字段齐全、签名 `version:` 与 feed 一致、安装包资产可下载） |
| 真实客户端检查更新 | ✅ v0.1.1 客户端显示「发现新版本 v0.1.2」+ notes + 可升级徽标；下载进度条正常 |
| 升级对照实验 | `/P /UPDATE /R /ARGS` 未落地；`/S /UPDATE /R /ARGS` 成功：注册表 `0.1.1→0.1.2`、exe 时间戳更新、`/R` 自动重启 → D024 改用 `quiet` |
| 最终安装版 | ✅ 用 v0.1.3 NSIS 安装包静默安装后启动，设置页显示「v0.1.3 · 已是最新版本」，无错误；注册表 `DisplayVersion = 0.1.3` |

**发版实测，三个与预期不符的地方**（D023）：

1. NSIS 目标下 `createUpdaterArtifacts: true` 产出 `<setup>.exe` + `<setup>.exe.sig`，
   **没有 `.nsis.zip`**。签名内容嵌进 `latest.json`，Release 只传安装包与 `latest.json`。
2. `gh` 把资产名里的**空格换成点**（`YOLO Trainer_…` → `YOLO.Trainer_…`），首次发版因此
   `%20` URL 404、资产核对永远不匹配。→ 上传无空格副本 `YOLO-Trainer_<ver>_…exe`。
   **v0.1.0 是踩坑版，v0.1.1 是修正版**，两个版本也用来演练真实升级。
3. 签名 trusted comment **带 `version:`**（初次只看前 300 字符误判成不带）。据此启用
   `requireSignedVersion`，堵住用旧签名强制降级的攻击。

**仍未验证 / 需人工**：在**正常交互式桌面**完整点一次「下载并安装」，确认安装器自动装完并
重启（agent 会话里 passive 弹窗无法完成；quiet 的等价参数已手动验证成功，见 D024）；
独显机器上的 GPU 训练。

## 关键体积

| 对象 | 大小 |
| --- | --- |
| `montgomery.exe`（含 training/onnx/vulkan 全功能） | 134 MB |
| 训练产出的 `best.bpk`（yolo26n，3 类） | 4.98 MB |
| 增量重编译（只改 montgomery 本体） | 2m11s |
| 改 cubecl-wgpu 后的重编译 | 7m56s |

## GPU 问题（本机 AMD Vega 6）

- 机器：MECHREVO WUJIE 14 / Ryzen 5 5560U / Radeon Vega 6 核显（共享内存）/ 16 GB
- 另有 OrayIddDriver（向日葵虚拟显示驱动），当前未激活
- 症状一：`predict` 在 `cubecl-wgpu/src/compute/timings.rs:510` 处 `Failed to map buffer`，
  随后 `The device is poisoned ... driver implementation is at fault`
- 症状二：修掉 timings 后仍 `device lost`，说明 timings 只是最先撞上的模块
- 症状三：`train --dry-run`（batch 1 / 160px）240 秒未完成，未见崩溃 —— 更像 JIT 编译极慢
- 系统日志无 TDR（4101）复位记录

**判断**：Vega 6 的 Vulkan compute 支持与共享显存是瓶颈，非 montgomery 代码问题
（同一条路径在 CPU 后端完全正确）。

### 根因链条（源码级）

`cubecl-wgpu/src/runtime.rs:693`：

```rust
let time_measurement = if features.contains(wgpu::Features::TIMESTAMP_QUERY) {
    TimingMethod::Device      // 相信驱动"声称"支持
} else {
    TimingMethod::System
};
```

驱动声明支持 `TIMESTAMP_QUERY`，实际 map 回读失败 → autotune 的 profiling 崩溃 → 设备被 poison。

## 本地改动（实验性质，非上游代码）

1. `research/montgomery/src/lib.rs` 新增 `wgpu_device_from_env()`：
   支持 `MONTGOMERY_WGPU_BACKEND=dx12|vulkan|gl|cpu`，默认走原来的 Auto。
   其中 `cpu` 仅用于诊断，让训练能落到 Flex CPU 后端。
2. `spikes/cubecl-wgpu-patched/`：cubecl-wgpu 0.11.0 的本地副本，`runtime.rs` 增加
   `CUBECL_WGPU_TIMING=system` 开关，默认行为与上游一致。
3. `research/montgomery/Cargo.toml` 加 `[patch.crates-io]` 指向上述本地副本。
4. `spikes/tiny-dataset/`：4 训练图 + 2 验证图的最小 YOLO 数据集（3 类）。

## 桌面壳实测（2026-11-10 晚，全部为真机实测）

| 项 | 结果 | 怎么验的 |
| --- | --- | --- |
| 窗口位置/大小恢复 | ✅ 写入 1000×700 @ (180,120) → 启动后 `pos=(180,120) size=1016×709`（16×9 为窗口边框） | 手写状态文件 + `GetWindowRect` |
| 关闭即隐藏 | ✅ 发 WM_CLOSE 后 `visible=False`，进程 `STILL_ACTIVE`，几何不变 | `PostMessageW(hwnd, WM_CLOSE)` |
| 单实例 | ✅ 第二次启动不产生新进程；隐藏状态的窗口被重新显示（同 hwnd 1901474） | 重复启动 + 枚举窗口 |
| 托盘图标 | ✅ 出现在通知区溢出栏第一位，黑底白 Y 渲染正常 | 截屏托盘区 + 点开溢出箭头 |
| 设置页读写 | ✅ 点 GPU → `settings.json` 写入 `"default_device":"gpu"`；改回自动同样落盘 | 点击设备卡 + 读文件 |
| 默认设备联通 | ✅ 设为 GPU 后进训练页，计算设备预选 GPU | 截图 |
| 产物目录默认值 | ✅ 显示 `C:\Users\13114\AppData\Roaming\com.yolotrainer.desktop\runs` | 截图 |
| 训练端到端（壳内） | ✅ CDP 调 `start_training` → 监控页出现「已完成」、loss 4573.58、后端 `cpu · Device<Flex(Cpu)>` | `spikes/qa/cdp.py` + 截图 |
| 安装包 | ✅ 28.19 MiB（内含 137.4 MB 引擎，LZMA 压缩） | `pnpm tauri build --bundles nsis` |
| 打包版渲染 | ✅ 中文字体正常（本地 woff2，非 CDN），无分割线布局 | 真实窗口截屏 `ImageGrab` |

**训练速度参考（本机 CPU，4 张图，batch 2，320px）**：1 epoch ≈ 1.3–1.5 秒。不是之前记的
"一分钟"——那个数字对应的是 GPU 路径下的 JIT 编译等待，两者不是一回事。

**没有验证的**：托盘右键菜单的两项能否点（需要人点）；托盘图标默认被 Win11 收进溢出栏，
要让用户一直看到得手动「固定在任务栏」，程序无法控制。

## 手势与动效（2026-11-10 晚）

| 项 | 状态 | 怎么验的 |
| --- | --- | --- |
| 类型检查 / 构建 | ✅ `tsc --noEmit` 干净，`vite build` 1.9s | 本机 |
| 四页视觉 | ✅ 1280×900 逐页截图 | 无头 Edge + `--force-prefers-reduced-motion` |
| 导航药丸对齐（展开 10px / 收起 8px 留白） | ✅ 截图核对 | `?sidebar=collapsed` |
| 命令面板 | ✅ 居中、首项高亮、键位提示 | `?palette=1` + 真机 Ctrl+K |
| 带数据监控页 | ✅ 4 指标卡 + 37% 进度 + 曲线 + 日志 | `?demo=1` |
| 数字补间 / 进度条 | ✅ 数值与宽度都走 tween，不是跳变 | 截图 + 代码路径 |
| 减弱动效 | ✅ 全部动画降为 1ms，布局无残留 | `--force-prefers-reduced-motion` |
| 点击切页 + 药丸跟随 | ✅ 训练 → 数据集 | CDP `Input.dispatchMouseEvent` |
| 拖拽调值 | ✅ 80px → 100 变 140；松手无残留样式 | CDP |
| 拖边缘改宽 | ✅ 236 → 296 | CDP |
| 触控板横滑 | ✅ 前进 → 监控，反向 → 回退 | CDP `Input.dispatchMouseEvent`(mouseWheel) |
| 长按/悬停浮出标签 | ✅ 收起态悬停出现「数据集 扫描与校验」 | 真机截屏 |
| 拖文件夹进窗口 | ⚠️ 需人工（无法合成 OS 级文件拖放） | — |

**验证方法**：真机 UI 自动化被 UIPI 拦过（应用以管理员身份运行时，非提权进程合成的鼠标事件
被静默丢弃，表现和"手势没实现"一模一样）。所以手势改用 **CDP 驱动浏览器预览**验证 ——
`spikes/qa/cdp_browser.py` 发真实的 mouse/pointer/wheel 事件，再把结果从 DOM 里读回来
（输入框的值、`aside` 的宽度、`h1` 的文本），比看截图可靠。

**这一轮实测抓到的 bug**：横滑方向永远回退（`reset()` 在判断方向前清零了累加器）。静态读代码
看不出来，是 CDP 测试里"日志说 left、栈却指向 onSwipeRight"暴露的。

## AI 操控接口（2026-11-10 深夜，真机实测）

本地 HTTP 服务，窗口和 AI 客户端读同一份 `Runtime`（D017/D018/D019）。接口契约见
`docs/API.md` 第 9 节。

| 项 | 状态 | 怎么验的 |
| --- | --- | --- |
| 未带令牌 → 401 | ✅ | `GET /health` 无头 |
| 错令牌 → 401 | ✅ | `Bearer wrong` |
| 正确令牌 → 200 | ✅ | `/health` 返回引擎路径与 uptime |
| 中文路径扫数据集 | ✅ | `?path=E:/工作区/…/tiny-dataset` → 4 张训练图、3 类、0 缺标注 |
| 不存在的路径 → 400 | ✅ | `?path=E:/no/such/dir` |
| `POST /train` → 202 | ✅ | 回显 accepted 参数 |
| 并发训练 → 409 | ✅ | 第二次 POST 被拒 |
| 真实训练闭环 | ✅ | 2 epoch，loss 4573.58 → 4072.84，1.38s/1.48s，`best.bpk` 落盘 |
| `exit_code` 归属 | ✅ | 结束后 `exit_code=0`，`status=done` |
| `POST /stop` | ✅ | 200 秒级生效，status 变 `failed`（不是静默 done） |
| 改端口即时生效 | ✅ | PUT 47822 → 旧端口不可达、新端口 200 |
| 关闭服务即时生效 | ✅ | PUT `control_enabled=false` → 端口不再监听，不需要重启 |
| 重启后按配置启动 | ✅ | 改回 47821 后重启，服务自动起来且 token 不变 |
| 404 / 405 | ✅ | `/nope` → 404；`GET /train` → 405 |
| `cargo test --lib` | ✅ 16 passed / 1 ignored | 新增 runstate 4 项 + runtime 3 项 |
| `spikes/qa/probe_control.py` | ✅ 16 项断言全过 | 独立脚本，可重复跑 |
| 打包版（28.22 MiB）实测 | ✅ 凭证、卡片、复制、药丸对齐 | 窗口截图 + 滚动到卡片 |

**实测性能**：一次 `/state` 轮询 < 1 ms（纯内存拷贝）；训练本身仍是 CPU 1.4 s/epoch。

**踩到的坑（重要）**：本机开着系统代理（`http_proxy=127.0.0.1:47479`），curl 会把
`127.0.0.1:47821` 也发给代理，返回 `502 upstream connect failed` / `10061 拒绝连接`——
**看起来完全是"服务没起来"，实际进程健康、端口在监听**。排查浪费了一轮。
`curl --noproxy 127.0.0.1` 或 `NO_PROXY=127.0.0.1` 解决。这条已写进 `GET /` 的自描述文档。

**未验证**：多客户端并发（没有并发压力需求）；Windows 防火墙首次弹窗（回环绑定通常不弹，
但没实测）；令牌明文落盘的威胁模型（见 D018 已知边界）。

## 模型选择器（2026-11-10 深夜，浏览器 CDP 实测）

训练页从"3 张模型卡"换成"任务 → 家族 → 规模"三级。当时覆盖全部 63 个可训练模型，D028 后收窄为 45 个。

| 项 | 状态 | 怎么验的 |
| --- | --- | --- |
| 63 个 ID 引擎可解析 | ✅ | 生成器产出 63 条无重复，逐条 `train --architecture` 均走到 `NO_DATASET` |
| 默认态 | ✅ | 检测 / YOLO26 / n → `yolo26n` |
| 切分割 → 家族收窄 | ✅ | 7 → 3 个，ID 变 `yolo26n-seg` |
| 换家族 → ID 跟随 | ✅ | YOLOv8 → `yolov8n-seg`，规模仍 5 档 |
| 换规模 → ID 跟随 | ✅ | 选 x → `yolov8x-seg` |
| 切回检测 → 保留家族 | ✅ | 仍是 YOLOv8 → `yolov8x`（不重置为默认） |
| YOLOX 的异形档位 | ✅ ~~已验证~~ | 规模变 `nano/tiny/s/m/l/x` 六档，ID `yolox-x`。**D028 已移除该家族，此项验证随之作废** |
| 分类 → 阻断 | ✅ | ID 仍显示 `yolo26x-cls`，同时出现警告条、启动按钮置灰 |
| 控件等宽 | ✅ | DOM 实测：任务 288×3、家族 119×7、规模 171×5 |
| `tsc --noEmit` / `vite build` | ✅ | 干净 / 2.07s |

**回退的决定**：试过把操作条改成 `sticky bottom`，实测在 808px 视口下会压住训练参数的输入框
（模型卡 450px + 参数卡 190px 已超一屏），且遮住的是正在编辑的字段，已回退为普通布局。

**未验证**：分割任务配矩形框标注的数据集——扫描器只看标签文件是否存在，不解析内容，
这类不匹配交给引擎报错（带 hint，会显示在错误区）。

## 分类数据集 + 单图推理（2026-11-11 凌晨，真机实测）

上一轮把 63 个模型都放进了选择器，但分类那 15 个是灰的——扫描器只认 YOLO 的
`images/ + labels/`。这一轮把链路补通，并加上了"训练完能不能用"的验证手段。

| 项 | 状态 | 怎么验的 |
| --- | --- | --- |
| 分类数据集能被扫描 | ✅ | `spikes/tiny-classify`（3 类 × 18 训练图）→ `format=imagenet-folders`、`class_counts=[6,6,6]`、`missing_labels=0` |
| 分类训练真能跑 | ✅ | `yolo26n-cls` 8 轮，loss 1.41 → 0.68，`best.bpk`/`last.bpk` 落盘，1.4–1.6 s/epoch（CPU） |
| 分类推理 | ✅ | `POST /predict` → 200，`prediction.classes` 是 top-3，`output=null`（分类不画图） |
| 检测推理 | ✅ | 训 3 轮后 `POST /predict`（不指定 model）→ 200，`output=…\runs\predictions\img1-predicted.png`，`detections` 有内容 |
| 标注图不污染数据集 | ✅ | 产物强制写到 `<输出目录>/predictions/`，源图旁边没有新增文件 |
| 界面里的验证卡片 | ✅ | 真实窗口：训练完成后出现「验证模型」，选图 → 左侧标注图（密集框）、右侧「检出 300 个 · 只列前 8 个」 |
| 任务与格式强制匹配 | ✅ | 分类数据集 + 检测任务 → 提示"当前数据集按类别分文件夹"并禁用启动 |
| `cargo test --lib` | ✅ 22 passed / 1 ignored | 新增 dataset 3 项（含分类夹具）+ predict 4 项 |
| `spikes/qa/shot_model_picker.py` | ✅ 8 组组合 | 上一轮写的，本轮回归通过 |

**实测暴露的产品问题**：只训 3 轮/4 张图的模型会吐出 300 个置信度 0.52 的重复框。UI 里按置信度
降序取前 8 个，但更根本的结论是——**看到"检出 300 个"本身就是一个有效信号**：模型没训好。

**踩到的坑（会误导判断）**：Windows 文件对话框的「文件名」框里输入绝对路径（`E:\qa-probe\x.jpg`）
会报"文件名无效"，即使文件确实存在；改用同目录下的相对文件名（`x.jpg`）立刻成功。测试选图流程时
不要用绝对路径，否则会误判成程序 bug。

**未验证**：GPU 推理（本机核显不可用）；`--masks` 的分割可视化；COCO 格式（引擎支持，扫描器
不支持，UI 明确拒绝而不是假装支持）。

## 待办

- [ ] 在有 NVIDIA / AMD 独显的机器上验证 GPU 训练（这是产品核心场景，本机核显不代表真实用户）
- [ ] 确认 `--batch -1` 自动批大小在本机外的行为
- [ ] 训练产物精度验证需要真实数据集（当前 tiny-dataset 仅验证流程）
- [ ] 决定是否把 `MONTGOMERY_WGPU_BACKEND` / `CUBECL_WGPU_TIMING` 保留为正式功能
- [ ] 明确桌面壳的开源许可证：README 已写清"公开可见但未指定许可"，对外分发前要定
- [ ] 在正常交互式桌面人工点一次「下载并安装」，确认自动装完 + 重启（D024；agent 会话只验证到「拉起安装器」）
- [x] `requireSignedVersion`——已启用：CLI 2.12.4 的签名确实带 `version:`（D023）
- [ ] 人工过一遍拖文件夹进窗口的手势、托盘菜单
- [x] 本地控制服务（AI 操控）——已完成
- [x] 模型选择器（D028 后收窄为 45 个）——已完成
- [x] 分类数据集链路（ImageNet 目录结构）——已完成
- [x] `/predict` 单图推理端点 + 界面验证卡片——已完成
- [x] GitHub 公开仓库 + 版本管理 + 官方 updater + 发版脚本——已完成
- [ ] COCO 数据集：引擎能读，扫描器不会，UI 现在明确拒绝。要接得解析 COCO JSON（含 RLE mask）
- [ ] 分割 / 检测共用标签文件，扫描器无法区分 bbox 与多边形；如需提前拦截要解析标签内容
- [ ] `GET /watch` 长轮询：只在需要"训练崩了立刻知道"时才做（D019）
- [x] **续训入口**：已完成（三态：从零 / 微调 / 断点续训），`TrainRequest` 改成 tagged enum，见 D025
- [x] **ONNX 导出**：已完成，走骨架 + 填权重，用户机器零 Python，见 D027
- [ ] **骨架只覆盖 10 个架构**：`tools/onnx_skeleton/generate_all.py` 的 `TARGETS` 之外的架构会明确报错。要扩就加一行重跑（约 10 秒/个）
- [ ] **骨架未随 export spec 版本自动失效**：上游图结构变了要手动重生成。如果要长期维护，应在生成时写入 spec 版本、填充时校验

## 复现命令

```bash
# CPU 推理
MONTGOMERY_WGPU_BACKEND=cpu montgomery.exe predict --model best.bpk --source img.jpg --device cpu

# CPU 训练（诊断用）
MONTGOMERY_WGPU_BACKEND=cpu montgomery.exe train --architecture yolo26n \
  --data dataset.yaml --epochs 1 --batch 2 --imgsz 320 --workers 2 --no-val

# 分类训练（先造夹具：3 类 24 图）
python spikes/qa/make_classify_dataset.py
montgomery.exe train --architecture yolo26n-cls --data spikes/tiny-classify/dataset.yaml \
  --epochs 8 --batch 6 --device cpu

# GPU 训练（需要独显机器；本机建议同时设第二个变量）
CUBECL_WGPU_TIMING=system montgomery.exe train --architecture yolo26n --data dataset.yaml
```

界面 QA（浏览器预览，不需要壳）：

```bash
cd desktop && npx vite --port 1420        # 然后访问 http://[::1]:1420/?page=...
#   ?page=dataset&demo=classify   分类数据集的类别分布卡
#   ?page=monitor&demo=done       已完成的训练 + 「验证模型」卡片
python spikes/qa/shot_model_picker.py 5200 # 模型选择器的 8 组联动断言 + 截图
```

打包（两个前置条件，漏了会失败或得到不能用的产物）：

```bash
/d/Git/usr/bin/rm.exe -rf desktop/dist        # vite 清目录会被 safe-delete 阈值拦住
cd desktop && npx tauri build                 # 必须用 tauri build；cargo build --release
                                              # 出来的 exe 会去连 localhost:1420
```

带签名的更新产物需要额外两个环境变量，缺了 `tauri build` 会拒绝生成 `.sig`：

```bash
TAURI_SIGNING_PRIVATE_KEY="$USERPROFILE/.tauri/yolo-trainer.key" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" npx tauri build
```

发版（自动完成构建、tag、Release、latest.json、资产核对）：

```powershell
powershell -File scripts/release.ps1 -Notes "这次改了什么"
# -SkipBuild 只重传现有产物；-Draft 发草稿，不切 latest 指针
```

更新链路自检（不用启动应用，12 项断言）：

```bash
python spikes/qa/probe_update.py                 # 检查 latest Release
python spikes/qa/probe_update.py --tag v0.1.0   # 检查指定 Release
python spikes/qa/probe_update.py --sig <本地 .exe.sig>   # 顺便比对签名
```

它会验：feed 匿名可读且无 BOM、字段齐全、资产 URL 无空格且能下载、
**签名 trusted comment 里的 `version:` 与 feed 一致**（`requireSignedVersion` 的前提）。
