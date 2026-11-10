# CLI 契约（UI 依赖此文档）

界面通过**子进程**调用 `montgomery.exe`，只用 stdin/stdout/stderr 通信，不链接其代码（原因见 DECISIONS.md D002）。

**改任何参数、输出字段或错误码，必须同步更新本文件。**

---

## 1. 进程约定

| 流 | 内容 | 谁读 |
| --- | --- | --- |
| **stdout** | 每行一个 JSON 对象（NDJSON），训练进度事件 | UI 逐行解析 |
| **stderr** | 人类可读日志（中文/英文散文），实时进度 | 日志面板，可选 |
| **退出码** | `0` 成功，`1` 失败，`101` panic（驱动崩溃等） | UI 判断结果 |

**关键规则**：UI 只能从 stdout 的 JSON 取数据，**不要正则解析 stderr**。stderr 的措辞随时可能改。

stdout 每行写完立即 flush，因此 UI 可以边跑边读，不必等进程结束。

---

## 2. 训练命令

```
montgomery train --architecture <MODEL> --data <dataset.yaml> [options]
```

三种初始化方式互斥，必须且只能给一个：

| 参数 | 含义 |
| --- | --- |
| `--architecture yolo26n` | 从零初始化（推荐给终端用户，无需预训练权重） |
| `--model <x.bpk>` | 从预训练权重初始化 |
| `--resume <checkpoint>` | 断点续训（数据集与模型从 checkpoint 读取） |

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `--device` | `auto` / `cpu` / `gpu` | **`auto`** | `auto` 会在独立进程里探测 GPU，失败自动回退 CPU |
| `--epochs` | 整数 | 100 | 训练轮数 |
| `--batch` | 整数 / `-1` | 8 | `-1` 表示自动探测最大可行批大小 |
| `--imgsz` | 整数 | 模型默认 | 正方形边长，必须为 32 的倍数 |
| `--workers` | 整数 | CPU 核数 | 数据预处理线程 |
| `--accumulation` | 整数 | 1 | 梯度累积步数 |
| `--prefetch` | 整数 | 2 | 预取批数 |
| `--seed` | 整数 | 0 | 随机种子 |
| `--project` | 路径 | `runs` | 输出根目录 |
| `--name` | 字符串 | `train` | 运行子目录名 |
| `--dry-run` | 标志 | 关 | 只跑 数据→前向→loss→反向，不改参数，不写 checkpoint |
| `--no-val` | 标志 | 关 | 跳过验证（快，但拿不到 mAP） |
| `--no-export` | 标志 | 关 | 不导出 best.bpk / last.bpk |
| `--save-period` | 整数 | 10 | 每 N 轮存一次可续训 checkpoint |
| `--val-confidence` | 浮点 | 自动 | 验证用的置信度下限 |
| `--val-iou` | 浮点 | 自动 | 验证用的 NMS IoU |
| `--max-detections` | 整数 | 自动 | 每图保留的最大预测数 |

### `--device auto` 的行为（重要）

1. 启动一个子进程跑 `__device-probe --device gpu`（跑一次 32×32 矩阵乘并回读结果）。
2. 子进程退出码为 0 且数值正确 → 用 GPU。
3. 子进程崩溃、返回非零或超过 120 秒 → **自动回退 CPU**，并在 stderr 打印
   `[code GPU_FALLBACK]`。

之所以用子进程：驱动出问题时会让设备进入 poisoned 状态并 panic 掉整个进程，进程内无法恢复。

**探测失败的 GPU 不会让训练失败**，只是变慢。UI 应当把这一点显示给用户。

---

## 3. stdout 事件 schema

所有事件共用一个 `type` 字段，按它分发。**未知 type 必须忽略，未知字段必须容忍**（后续会加字段）。

### `start` — 开始训练

```json
{"type":"start","run":"runs\\detect\\train-1794284167-000072e4","backend":"cpu","adapter":"Device<Flex(Cpu)>","epochs":100,"batch_size":2,"image_size":320,"class_names":["dog","person","bike"]}
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `run` | string | 本次运行的输出目录（绝对或相对当前工作目录） |
| `backend` | `"cpu"` / `"gpu"` | **实际**执行的后端，不是用户请求的 |
| `adapter` | string | 适配器描述，可直接显示 |
| `epochs` | number | 总轮数 |
| `batch_size` | number | 实际批大小 |
| `image_size` | number \| null | 输入边长，null 表示用模型默认 |
| `class_names` | string[] | 类别名，按索引顺序 |

### `epoch` — 每轮训练结束

```json
{"type":"epoch","epoch":1,"epochs":2,"loss":4573.5815,"fitness":null,"elapsed_s":1.1945}
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `epoch` | number | 当前轮（从 1 开始） |
| `epochs` | number | 总轮数，用于算进度百分比 |
| `loss` | number | 本轮平均训练损失 |
| `fitness` | number \| null | 本轮验证适应度；`--no-val` 时为 null |
| `elapsed_s` | number | 本轮墙钟耗时（秒） |

UI 可用 `elapsed_s` 累积估算 ETA：`剩余 = (epochs - epoch) × 最近若干轮的平均耗时`。

### `end` — 成功结束

```json
{"type":"end","run":"runs\\detect\\...","best":"...\\exports/best.bpk","last":"...\\exports/last.bpk"}
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `run` | string | 运行目录 |
| `best` | string \| null | 最优模型路径；`--no-export` 或未产出时为 null |
| `last` | string \| null | 最后一轮模型路径 |

### `error` — 失败结束

```json
{"type":"error","code":"NO_DATASET","message":"cannot open dataset manifest missing.yaml: 系统找不到指定的文件。 (os error 2)","hint":"数据集配置无法读取，请检查 dataset.yaml 路径与内容。"}
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `code` | string | 稳定错误码，见下表，UI 按它分支 |
| `message` | string | 原始错误文本，用于展示细节 |
| `hint` | string \| null | 可以直接展示给用户的操作建议 |

发出此事件后进程以退出码 1 结束，不会再发 `end`。

---

## 4. 错误码

| code | 触发场景 | UI 建议提示 |
| --- | --- | --- |
| `GPU_FALLBACK` | 探测失败已回退 CPU（**stderr 警告，非致命**） | 显示"当前使用 CPU，训练会较慢" |
| `GPU_LOST` | 训练中设备丢失 | "显卡驱动异常，请更新驱动或切换到 CPU" |
| `NO_DATASET` | 数据集路径、yaml 或验证集缺失 | 引导重新选择数据集 |
| `BAD_ANNOTATION` | 标注文件格式错误 | 指出格式规则 |
| `EMPTY_DATASET` | 训练划分里没有图片 | 检查 train 目录 |
| `CLASS_MISMATCH` | 续训时类别表与数据集不一致 | 提示类别已变更 |
| `OOM` | 显存/内存不足 | 建议减小 batch 或分辨率 |
| `INTERNAL` | 未归类的其它错误 | 展示原始 message |

---

## 5. 退出码

| 码 | 含义 | UI 处理 |
| --- | --- | --- |
| 0 | 成功，且已收到 `end` 事件 | 显示产物 |
| 1 | 业务失败（参数/数据/IO） | 读 stderr 末行或 `error` 事件 |
| 101 | 进程 panic（多为显卡驱动） | 提示"引擎崩溃"，建议切 CPU 重试 |

---

## 6. 其它相关命令

```
# 单图推理（UI 的"预览"用）
montgomery predict --model <x.bpk> --source <img.jpg> [--device cpu|gpu|auto] [--json]
# 阈值默认 confidence 0.25 / iou 0.45

# 验证已有模型
montgomery val --model <x.bpk> --data <dataset.yaml> [--json]

# 设备自检（隐藏命令，供 --device auto 内部使用）
montgomery __device-probe --device gpu|cpu
```

---

## 7. 已实测的环境

| 项 | 值 |
| --- | --- |
| 构建 | `cargo build --release --features training` |
| 已测后端 | CPU（Flex），完整闭环通过 |
| 未测后端 | GPU（本机 Vega 6 驱动崩溃，需独显机器） |
| exe 体积 | 137 MB |
| 训练产物 | `best.bpk` 约 5 MB（yolo26n、3 类） |

---

## 8. 界面 ↔ 壳的接口（`#[tauri::command]`）

这一层的契约由 `src/lib/api.ts` 实现，字段名与 Rust 结构体一一对应。

本地控制服务（第 9 节）**不复用** `invoke`，它调用的是同一批模块（`engine` / `dataset` /
`settings` / `runstate`），通过 `Runtime` 共享状态（D011、D017）。两个前端，一份实现。

### `get_settings` → `SettingsView`

```jsonc
{
  "engine_path": null,                       // 用户配置；null = 自动检测
  "engine_resolved": "…\\montgomery.exe",     // 实际找到的路径
  "engine_error": null,                      // 配置有误时的原因（非空表示不可用）
  "default_device": "auto",                  // auto | cpu | gpu
  "output_dir": "…\\com.yolotrainer.desktop\\runs",  // 永远是绝对路径
  "output_dir_is_default": true,             // true 表示用户没改过
  "config_dir": "…\\com.yolotrainer.desktop",
  "engine_running": false
}
```

**约定**：路径一律由壳解析，前端不做路径拼接。`output_dir` 在存储值为 `null` 时也返回可用的
绝对路径，UI 不需要知道"默认是哪儿"。

### `save_settings(settings: Settings)`

入参只接受三个可写字段（`engine_path` / `default_device` / `output_dir`），返回保存后的
`SettingsView`。规则：

| 情形 | 行为 |
| --- | --- |
| 空字符串 | 归一化成 `null` |
| `default_device` 不在白名单 | 归一化成 `auto`（否则引擎会以 clap 用法错误退出） |
| 引擎路径不存在 | **照常保存**，但 `engine_error` 非空，界面显示红框提示 |
| 写盘失败 | 返回错误，内存缓存**不更新**（避免界面显示一个重启后就消失的值） |

### `resolve_engine` → `string`

按 `settings.engine_path` → `MONTGOMERY_ENGINE` → exe 同目录 → 仓库构建产物 查找。
配置了却不存在的路径会**直接报错**，不静默回退——否则用户看到"装了却找不到引擎"。

### `predict_image(request: PredictRequest)` → `PredictResult`

与第 9.6 节的 `POST /predict` **共用同一个实现**（`src-tauri/src/predict.rs`），字段一致，
只是入口不同：窗口走 `invoke`，AI 客户端走 HTTP。

`model` 省略时取当前 `RunState.best`；没有任何产物时报错"先跑一次训练"，不返回空结果。

---

## 9. 本地控制服务（AI 操控）

让 AI 助手或脚本直接驱动这个程序：扫描数据集、起训练、读进度、拿产物。默认开启。

| 项 | 值 |
| --- | --- |
| 监听 | `127.0.0.1`，默认端口 `47821`（被占用时自动改用系统分配的端口） |
| 协议 | HTTP/1.1，每个响应带 `Connection: close` |
| 鉴权 | `Authorization: Bearer <token>`，或 `?token=<token>` |
| 令牌 | 32 位十六进制，存于 `<配置目录>/control-token`，重启不变 |
| 例外 | `GET /` 不需要令牌（见下） |
| 实现 | `src-tauri/src/control.rs`，手写解析，无 HTTP 框架依赖（D017） |

**为什么 `GET /` 免鉴权**：它是自描述文档，AI 拿到地址后要先能读到契约。文档里没有任何
运行数据，只有一个端点表、示例和错误码。

**系统代理会吃掉回环请求**（实测踩到）：本机装着 Clash 之类的代理时，`http_proxy` 环境变量
会让 curl 把 `http://127.0.0.1:47821` 也发给代理，得到 `502 upstream connect failed` 或
`10061 拒绝连接`——看起来像服务没起来，其实进程健康。curl 加 `--noproxy 127.0.0.1`，
或设 `NO_PROXY=127.0.0.1`。浏览器的 `fetch` 和大多数 SDK 会自动绕过回环，curl 不会。

### 9.1 端点

| 方法 | 路径 | 说明 | 成功码 |
| --- | --- | --- | --- |
| GET | `/` | Markdown 自描述文档（免鉴权） | 200 |
| GET | `/health` | 服务、引擎、版本 | 200 |
| GET | `/state` | 训练快照（`RunState`） | 200 |
| GET | `/state?since=N` | 快照 + 序号 > N 的事件 | 200 |
| GET | `/events?since=N` | 只有增量事件 | 200 |
| POST | `/train` | 开始训练 | 202 |
| POST | `/stop` | 中止训练 | 200 |
| POST | `/predict` | 用训练好的模型推理一张图 | 200 |
| GET | `/datasets/scan?path=...` | 校验数据集目录 | 200 |
| GET | `/settings` | 读配置（`SettingsView`） | 200 |
| PUT | `/settings` | 整体替换配置 | 200 |

错误一律是 `{"ok":false,"error":"人话"}`，状态码 `400`（参数/路径）、`401`（令牌）、
`404`（端点）、`405`（方法）、`409`（已有训练在跑）、`500`（引擎/磁盘）。

### 9.2 `GET /health`

```json
{"ok":true,"app":"yolo-trainer-lab","version":"0.1.0","uptime_s":42,
 "running":false,"engine":"E:////…////montgomery.exe","platform":"windows"}
```

`engine` 为 `null` 表示没找到引擎——此时 `/train` 会失败并说明原因。

### 9.3 `GET /state` → `RunState`

```jsonc
{
  "status": "running",          // idle | running | done | failed
  "epoch": 7,
  "total_epochs": 20,
  "last_loss": 1.8342,
  "last_epoch_seconds": 1.21,
  "backend": "cpu",             // 实际后端，不是请求的 device
  "adapter": "Device<Flex(Cpu)>",
  "run_dir": "…\\runs\\detect\\train-1794284167-000072e4",
  "best": null,                 // 成功后才有
  "last_model": null,
  "error": null,                // 失败时是 {"code","message","hint"}
  "exit_code": null,
  "seq": 12                     // 已发布事件总数，下次轮询把它当 since
}
```

带 `?since=N` 时返回：

```json
{"ok":true,"state":{…},"cursor":12,"events":[{"type":"epoch","epoch":7,…}]}
```

**不要用 SSE**：轮询 `since` 是幂等的、可断线重连的，而且不需要客户端维持长连接
（DECISIONS D019）。一次轮询的成本是一次内存拷贝。

### 9.4 `POST /train`

```jsonc
{"dataset":"E:/data/coco-mini","epochs":20,"batch":4,"device":"auto"}
```

只有 `dataset` 必填，其余默认 `architecture=yolo26n`、`epochs=1`、`batch=8`、
`device=<设置里的默认设备>`。`dataset` 可以是含 `dataset.yaml` 的目录，也可以是 yaml 本身。

```json
{"ok":true,"accepted":{"architecture":"yolo26n","epochs":20,"batch":4,"imgsz":null,"device":"auto"},
 "note":"已开始。轮询 GET /state 或 GET /events?since=<cursor> 获取进度。"}
```

已被占用时返回 `409`，不会排队——排队会让"我点了开始"和"它什么时候开始"脱节。

### 9.5 `GET /datasets/scan?path=…`

返回与 `scan_dataset` 相同的 `DatasetSummary`（`dataset` 字段里）。路径必须
percent-encode；服务端按 UTF-8 解码，中文目录名可用。

字段里有两个是**前端必须用对的**：

| 字段 | 说明 |
| --- | --- |
| `format` | `yolo`（检测/分割）或 `imagenet-folders`（分类）。任务与格式必须匹配 |
| `manifest` | `dataset.yaml` 的绝对路径。**训练要传这个**，不是 `root`：当 yaml 里的 `path:` 指向别的目录时，数据根下没有 yaml |

其余：`classes`、`train_images`、`val_images`、`missing_labels`（分类恒为 0，文件夹本身就是
标签）、`annotated`、`class_counts`（每类训练图数，仅分类有值，YOLO 返回空数组）、
`warning`。

### 9.6 `POST /predict`

```jsonc
{"source":"E:/data/images/val/0001.jpg",   // 必填，图片绝对路径
 "model":null,                             // 省略 = 最近一次训练导出的 best.bpk
 "confidence":null,                        // 省略 = 引擎默认 0.25
 "masks":false,                            // 分割模型画掩码
 "device":"cpu"}
```

```jsonc
{"ok":true,"result":{
  "model":"…\\exports\\best.bpk",
  "source":"E:/data/images/val/0001.jpg",
  "output":"…\\runs\\predictions\\0001-predicted.png",   // 分类为 null，它不画图
  "prediction":{ /* 引擎 predict --json 的原样输出 */ }}}
```

`prediction` 的形态由任务决定：检测是 `detections[]`（`xmin/ymin/xmax/ymax` 为源图像素、
`confidence`、`class_name`），分割另带掩码信息，分类是 `classes[]`（top-5）。

**标注图只会写进 `<输出目录>/predictions/`**。引擎的默认行为是写在源图旁边
（`xxx-detections.png`），那会污染数据集目录——本项目已经因此踩过一次坑，所以端点强制指定
`--output`。`model` 不存在或 `source` 不是文件时返回 `400`，不会去猜。

### 9.7 `PUT /settings`

请求体是完整的 `settings::Settings`（不是补丁）：

```json
{"engine_path":null,"default_device":"cpu","output_dir":null,
 "control_enabled":true,"control_port":47821}
```

**必须是完整对象**：Rust 侧用 `#[serde(default)]` 反序列化，漏字段会被静默还原成默认值。
改 `control_enabled` / `control_port` 会立即重启监听，不需要重启应用。

### 9.8 与 UI 的一致性

两侧读同一份 `Runtime`：

```
montgomery stdout ──▶ hub ──▶ 订阅线程 ──┬─▶ RunState（服务读这个）
                                          └─▶ webview 事件（界面读这个）
```

所以界面显示到第 7 轮时，`/state` 也一定是第 7 轮。不存在两份进度。

---

## 10. 自动更新（客户端 ↔ GitHub Releases）

第 8 节的命令契约在这里补全更新相关的三个命令；具体选型理由见 `DECISIONS.md` D020–D023。

### 10.1 命令

| 命令 | 入参 | 返回 | 备注 |
| --- | --- | --- | --- |
| `app_version` | 无 | `string`，如 `"0.1.0"` | 由 `build.rs` 从 `tauri.conf.json` 注入，前端不硬编码 |
| `check_update` | 无 | `UpdateInfo` | **永远成功**；网络/解析失败放在 `error` 字段里 |
| `install_update` | 无 | `Result<(), String>` | Windows 上成功会**退出进程**，Promise 通常不会 settle |

三个命令都是 `async`（`check_update` / `install_update` 需要发 HTTPS 请求），不会阻塞命令线程。
浏览器预览（`!isTauri`）下 `getAppVersion()` 返回 `"预览模式"`，`checkUpdate()` 返回带 `error`
的 `UpdateInfo`，`installUpdate()` 抛错——不会伪造一个版本号。

### 10.2 `UpdateInfo`

```jsonc
{
  "current": "0.1.0",       // 当前程序版本
  "latest": "0.1.1",        // 远端版本；无更新时等于 current（不会是空串）
  "has_update": true,
  "notes": "修复了…",        // Release notes，可能含换行；无内容时 ""
  "error": null             // 非空 = "没能检查"，与 "没有更新" 是两种不同的状态
}
```

字段是 **snake_case**（与 Rust 的 `Serialize` 默认一致），不是驼峰。
`error` 是收敛后的人话：`ReleaseNotFound` → "更新源还没有发布记录"，超时/连接失败 →
"连不上更新服务器…"，不会把原始错误串丢给界面。

### 10.3 `update-progress` 事件

`install_update` 期间由 Rust 侧 `Emitter` 发出，前端用 `listen` 订阅：

```jsonc
{ "phase": "downloading", "percent": 42 }
{ "phase": "installing", "percent": 100 }
```

`percent = -1` 表示服务端没给 `Content-Length`，此时界面不画进度条。事件**按百分比变化节流**，
不是每个下载块都发一次（8 KB 一块会淹没 webview）。`installing` 是界面能看到的最后一个状态。

### 10.4 `latest.json`（feed 契约）

固定地址：`https://github.com/QiuMr/yolo-trainer-lab/releases/latest/download/latest.json`。
`releases/latest/download/<asset>` 永远指向最新一个非草稿 Release，所以发新版**不用改客户端**。

```jsonc
{
  "version": "0.1.1",
  "notes": "修复了…",
  "pub_date": "2026-11-11T02:30:00Z",
  "platforms": {
    "windows-x86_64": {
      "url": "https://github.com/QiuMr/yolo-trainer-lab/releases/download/v0.1.1/YOLO%20Trainer_0.1.1_x64-setup.exe",
      "signature": "<base64 字符串，内容就是 .exe.sig 文件>"
    }
  }
}
```

- `version` 必须是三段数字，不带 `v` 前缀（插件会 `trim_start_matches('v')`，但发版脚本不带）。
- `pub_date` 是 RFC3339 UTC。
- 平台键：插件依次尝试 `windows-x86_64-nsis` 和 `windows-x86_64`，命中任一即可。
  NSIS 是本项目唯一目标，用通用键。
- `url` 里的空格**必须** percent-encode（`[uri]::EscapeDataString`），否则下载 404。
- `signature` 是 `.exe.sig` 文件的全部内容（base64 编码的 minisign 签名），首尾空白要去掉。
- JSON **必须无 BOM**：serde_json 遇到 BOM 直接解析失败，而每次启动都要读它。

### 10.5 发版产物

`createUpdaterArtifacts: true` 在 NSIS 目标下产出两个文件，**没有 `.nsis.zip`**：

| 文件 | 是否上传 | 用途 |
| --- | --- | --- |
| `YOLO Trainer_<ver>_x64-setup.exe` | 是 | 用户手动安装 + 更新器下载 |
| `YOLO Trainer_<ver>_x64-setup.exe.sig` | 否 | 内容嵌进 `latest.json.signature` |

Release 资产共两个：安装包与 `latest.json`。`scripts/release.ps1` 会核对两者都在
（gh 上传大文件中断时可能留下只有 JSON 的空壳 Release），缺了自动补传。

### 10.6 失败语义（重要）

| 情形 | 客户端表现 |
| --- | --- |
| 还没发过 Release | `error` = "更新源还没有发布记录"，不弹红框 |
| 离线 / 公司代理 | `error` = 一句可读提示；设置页用浅色文字，不用 danger 色 |
| 版本相同或更旧 | `has_update = false`，界面显示"已是最新版本" |
| feed 版本号比当前旧但插件仍返回更新 | `install_update` 显式拒绝，报"更新源指向 v… 已中止" |
| 签名不匹配 | 下载阶段报错，安装包**不会运行**（插件先验签再 `install`） |
| 安装成功 | Windows 上进程退出 → NSIS `/P` 静默安装 → `/R` 自动重启 |

私钥在 `%USERPROFILE%\.tauri\yolo-trainer.key`，不入库。`requireSignedVersion` **未启用**：
Tauri CLI 2.12.4 的 `.sig` trusted comment 只带 `timestamp` 与 `file`，启用会让当前所有包被拒。
