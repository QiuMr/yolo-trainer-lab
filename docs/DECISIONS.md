# 决策记录（ADR）

记录不可逆或影响面大的选择。格式：背景 → 决定 → 理由 → 否决的方案 → 影响。

---

## D001 · 主引擎用 montgomery，不重写 Burn

**日期**：2026-11-10

**背景**：目标是 YOLO26n 轻量训练器。候选是现成的 montgomery（Rust + Burn + WGPU）和自己用 Burn 搭训练循环。

**决定**：直接用 montgomery。

**理由**：
- 完整链路已实测跑通（数据 → 训练 → 导出 .bpk → 推理），本机 CPU 后端验证过。
- 自己搭要移植骨干、3 个检测头、DFL-free loss、全套数据增强，工作量以周计。
- 许可（AGPL-3.0）与体积（137 MB）用户已明确表示可接受。

**否决的方案**：
- *Burn 自建训练循环*：只有体积和许可优势，但工期不可控，且在没验证 GPU 的前提下投入两周风险过高。
- *回退到 PyTorch/Ultralytics 打包*：违背「几十 MB、零配置」的初衷，且体积反而更大（100 MB+ 且需 CUDA）。

**影响**：产品基于 AGPL 代码，分发时必须附源码。规避方式见 D002。

---

## D002 · UI 与引擎用子进程隔离（sidecar），不静态链接

**日期**：2026-11-10

**背景**：AGPL-3.0 有传染性。UI 以何种方式调用 montgomery，决定 UI 本身是否必须开源。

**决定**：Tauri 进程通过命令行启动 `montgomery.exe`，只用 stdin/stdout/JSON 通信，不链接其代码。

**理由**：
- 两个独立程序通过命令行交互，是 FSF 认可的「聚合」而非「衍生作品」，UI 可以闭源。
- 进程隔离带来额外好处：GPU 崩溃（`device lost`）只会杀掉引擎进程，UI 不陪葬，可以弹提示并降级重试。
- 引擎可独立升级，不必重新打包 UI。

**否决的方案**：
- *把 montgomery 作为 Rust library 链进 Tauri*：技术上更简单（省一次 IPC），但整个 UI 变成 AGPL 衍生作品，必须开源。
- *起本地 HTTP 服务*：多一层端口占用与安全面，且 Tauri 本身已有 IPC 能力，没必要。

**影响**：必须在 `docs/API.md` 中严格定义 CLI 契约；错误信息要能被 UI 解析（需要结构化输出）。

---

## D003 · 前端选 Tauri 2 + React 19 + TypeScript + Vite

**日期**：2026-11-10

**背景**：需要给训练器做桌面界面。候选：Electron、Tauri、egui（纯 Rust）。

**决定**：Tauri 2 + React 19 + TypeScript + Vite，样式用 Tailwind，图表用 ECharts。

**理由**：
- Tauri 用系统 WebView2（本机已装 156/157），安装包只比裸 exe 多几 MB；Electron 要多背 80–150 MB 的 Chromium。
- React 生态最大、AI 辅助生成质量最高、后续维护和查资料最容易。
- Vite 开发热更新快；Tailwind 让三个页面的界面快速成型，不用手写大量 CSS。
- ECharts 的中文文档与示例最全，画 loss 曲线最省事。

**否决的方案**：
- *Electron*：体积和内存都违背本项目的轻量定位。
- *egui*：体积最小（+3 MB）、纯 Rust，但界面偏工程化，做不出面向普通用户的观感。
- *Vue 3*：中文文档更好、上手更平缓，但生态与 AI 生成质量略逊于 React；如果后续开发者更熟 Vue，可替换，UI 与引擎的契约不受影响（D002 的隔离让前端可独立更换）。
- *Svelte 5*：代码量最少、产物体积最小，但生态与参考资料少，长期维护风险更高。

**影响**：需要 Node 工具链（本机已有）；老 Win10 若缺 WebView2 需随包附带引导安装。

---

## D004 · CPU 是正式支持的降级路径

**日期**：2026-11-10

**背景**：本机没有可用 GPU，无法验证 GPU 路径；真实用户也可能无独显或驱动过旧。

**决定**：`--device auto` 优先 GPU，失败则自动回退 CPU 并明确告知用户；CPU 不是隐藏的调试开关，而是产品的一部分。

**理由**：
- 没 GPU 的机器也能用（慢但正确），扩大可用人群。
- 让我们在没有独显机器的情况下也能持续开发和验证全部流程。
- GPU 崩溃时用户看到一个可用的降级结果，而不是 `device is poisoned`。

**影响**：训练耗时可能相差一个数量级，UI 必须显示当前后端，避免用户误判卡死。

---

## D005 · 无边框窗口 + 自绘标题栏

**日期**：2026-11-10

**背景**：系统标题栏在 Windows 上是 32px 的浅灰条，与"纯白画布"的视觉要求直接冲突；
它也无法承载应用状态（当前页、连接状态）。

**决定**：`tauri.conf.json` 设 `decorations: false`，由 `components/TitleBar.tsx` 自绘
40px 标题栏，并在 capabilities 里显式申请 `start-dragging` / `minimize` / `toggle-maximize` /
`close` / `is-maximized` 权限。

**为什么不用 `titleBarStyle`**：那是 macOS 的能力（红黄绿灯内嵌）。Windows 上 Tauri 只能二选一：
保留系统装饰，或完全自绘。

**代价（必须承认）**：
- **失去 Win11 贴靠布局（Snap Layouts）**：鼠标悬停最大化按钮不再弹出四分屏面板。这是自绘标题栏
  在 Windows 上唯一真正丢掉的系统能力，且无法用前端代码补回（需要 Win32 消息钩子）。
- 边缘阴影改由 `shadow: true` 提供，与系统阴影不完全一致。
- 双击最大化交给 Tauri 的 `data-tauri-drag-region` 原生处理；**不能自己再加 dblclick 监听**，
  否则两次 toggle 会互相抵消。

**否决的方案**：
- *保留系统标题栏*：最省事，但整个界面上方会永久压着一条灰带，与设计基线冲突。
- *`transparent: true` + CSS 圆角自绘窗体*：能拿到圆角，但会失去原生阴影、拖动区边界易出错，
  且在部分集显驱动下透明窗口有合成瑕疵（本机是 Vega 6，风险更高）。

---

## D006 · 动效只保留三档时长与一条缓动

**日期**：2026-11-10

**背景**：动效容易失控——每个组件各写一套时长，整体观感就散了。

**决定**：`index.css` 的 `@theme` 里只定义三档时长，全部走同一条缓动
`cubic-bezier(0.22, 1, 0.36, 1)`：

| 时长 | 用途 |
| --- | --- |
| 120ms | 控件状态变化（hover / 按下 / 选中） |
| 180ms | 面板位移（侧边栏收起、展开） |
| 280ms | 内容入场（页面切换、卡片出现） |

并且 `@media (prefers-reduced-motion: reduce)` 把所有动画压到 1ms——侧边栏宽度动画是唯一
可能引起不适的位移，必须可关。

**被否决的做法**：
- *引入 framer-motion*：约 40 KB 运行时，而这个界面需要动的东西（淡入、位移、按下缩放）
  CSS 完全够用；多一个依赖就多一处版本风险。
- *给每个卡片加错峰入场*：卡片多时总时长会累加到 500ms 以上，用户点开页面等动画，
  属于典型的"设计师喜欢、用户嫌慢"。只保留页面级入场 + 导航项 30ms 错峰（最多 4 项）。

**影响**：新增动效必须复用这三档时长，不允许出现第 4 个数字。

---

## D007 · 侧边栏：收起是模式，宽度是手势

**决定**：`collapsed`（236 ↔ 64）与 `width`（拖拽，184–340）是两个独立状态，分别持久化到
`localStorage`。

**理由**：合成一个数字会让"收起后再展开"丢失用户辛苦拖出来的宽度。拖拽时临时禁用宽度过渡，
否则每个 mousemove 都触发 180ms 补间，手感变成"拖不动"。

**手势清单**（当前版本）：
| 手势 | 行为 |
| --- | --- |
| 拖拽侧栏右边缘 | 调宽，clamp 184–340 |
| 双击标题栏 | 最大化 / 还原（Tauri 原生） |
| Ctrl+1..4 | 切页 |
| Ctrl+B | 收起 / 展开侧栏 |

**被否决**：横向双指滑动切页——与页面纵向滚动冲突，误触成本高于收益。


---

## D008 · 关闭窗口不退出进程，托盘才是退出入口

**决定**：点 × 只 `hide()` 窗口，进程与训练子进程继续运行；真正的退出在托盘右键菜单的
「退出」，点击后走 `app.exit(0)`，原有 `RunEvent::Exit` → `kill_engine` 依然生效。

**理由**：训练动辄几十分钟，窗口生命周期不等于进程生命周期。把 × 绑定到退出，等于让用户
在完全没意识到的情况下杀掉半小时的训练。这也是 Docker Desktop / VS Code 这一类的通行做法。

**代价与配套**：
- 用户可能以为"已经关了"，所以托盘左键单击必须能回到窗口（已实现：show + unminimize + focus）。
- 关闭改为隐藏后，"再点一次桌面图标"会变成第二个实例、第二个托盘图标 → 必须同时引入
  `tauri-plugin-single-instance`，第二次启动只把已有窗口抬起来。
- 该插件必须**第一个注册**，否则新进程会在其它插件初始化之后才退出，可能出现两个引擎抢同一
  个数据集。

**被否决**：
- *× = 退出，托盘只作为快捷入口*：托盘就没有存在意义，且长时间训练会被误杀。
- *训练中才隐藏、空闲时退出*：行为随状态变化，用户无法形成预期，比一致的行为更差。

---

## D009 · 窗口状态只恢复位置/大小/最大化，不恢复"可见性"

**决定**：用 `tauri-plugin-window-state`，但显式传 `POSITION | SIZE | MAXIMIZED`，
**不用默认的 `StateFlags::all()`**。

**理由**：`all()` 包含 `VISIBLE`。由于 D008 允许应用在隐藏状态下退出，默认配置会把
`visible: false` 存盘，下次启动就是"进程在跑、窗口不出现"，用户会认定程序坏了。
这个坑不是猜的——读插件源码（`tauri-plugin-window-state-2.5.0/src/lib.rs`）确认
`default() → StateFlags::all()`，且 `restore_state` 会据此调用 `show/hide`。

**顺带确认**：插件在 `RunEvent::Exit` 时保存（`on_event` 分支），所以即便 × 被
`prevent_close` 拦掉，从托盘退出时状态照样落盘。

---

## D010 · 应用图标自绘：黑圆角方块 + 白 Y

**决定**：`spikes/brand/make_icon.py` 用 PIL 几何绘制 1024px 母版，再用
`pnpm tauri icon` 生成全套（ico / icns / Windows Store 图 / 抽屉图标）。托盘复用
`app.default_window_icon()`，不额外维护第二份资源。

**理由**：Tauri 模板默认图标是官方 logo，出现在任务栏和托盘里等于"这个应用没做完"。
几何绘制（而非文字排版）保证任何机器上形状一致，且 16px 下仍可辨认——已用 16/24/32/48
四档放大对照验证。

**被否决**：用字体渲染 "Y"——依赖系统字体，不同机器字形不同，小尺寸下笔画比例会变。

---

## D011 · 引擎操作与 Tauri 解耦，事件走单一 hub —— 为「AI 操控」预埋

**背景**：后续要加一个本地服务，让 AI（或任何外部客户端）能直接驱动这个软件：起训练、
查进度、读产物。这决定了现在的后端结构是否要重写。

**决定**：把「操作」从「Tauri 命令」里拆出来，形成三个不依赖 Tauri 的模块：

| 模块 | 职责 | 依赖 Tauri |
| --- | --- | --- |
| `dataset.rs` | 读 `dataset.yaml`、统计图片与标注 | 否 |
| `engine.rs` | 定位 exe、spawn、事件 fan-out | 否 |
| `settings.rs` | 读写 `settings.json` | 否 |
| `tray.rs` / `lib.rs` | 托盘、`#[tauri::command]` 薄壳、事件转发线程 | 是 |

**关键点一：Engine 句柄不含 `AppHandle`。**
`Engine(Arc<Inner>)` 里只有 `Mutex<Option<Child>>` 和 hub，`Clone` 后可自由丢进线程。
原来的实现是「reaper 线程里 `app.state::<EngineState>()`」，等于每个线程都要拖着一个 Tauri
句柄，这是最先要拆掉的东西。

**关键点二：事件不再 `app.emit`，而是 `Hub::publish`。**
hub 是一个 `Vec<mpsc::Sender>`，订阅者可在运行时增加。桌面壳在 `setup` 里订阅一次，转发到
webview；将来的服务订阅同一份流，直接喂 SSE/WebSocket。**spawn、解析、退码回收只有一份实现。**

**关键点三：事件格式不变。**
stdout 的 NDJSON 仍然原样转发，`exit` 仍由宿主补，`engine-event` / `engine-log` 两个通道名
不变。所以前端和 docs/API.md 一个字都不用改——这不是巧合，是刻意保持的契约。

**被否决的做法**：
- *现在就把 axum/HTTP 服务写出来*：还没有调用方，也没定鉴权与端口策略，先写只会锁死形状。
  但上面的结构已经保证"接一个服务 = 加一个订阅者 + 一层薄 handler"，不是重构。
- *让服务直接调 `montgomery.exe`、绕开应用*：那样服务与界面会各自维护一份参数拼装和状态
  管理，两套逻辑必然漂移；而且应用看不到外部启动的训练，界面会显示"空闲"。
- *把事件总线做成 broadcast/async channel*：进程里没有 async runtime，`std::sync::mpsc`
  足够，且不引入 tokio 依赖。

**给未来的自己**：接服务时**不要**让 handler 直接 `app.emit`。新消费者一律 `engine.subscribe()`，
否则又回到"一个消费者绑死在 webview 上"的老路。

---

## D012 · 默认产物目录用应用数据目录，不用当前目录

**决定**：`--project` 默认指向 `<app_data_dir>/runs`，可在设置里改。

**理由**：引擎自己的默认值是 `./runs`（相对当前工作目录）。打包后应用的工作目录是安装目录，
把权重写进 `Program Files` 会因为权限失败；即便装在可写的用户目录，把训练产物混进程序目录
也是会让用户找不到文件的做法。开发态下这个差异不会暴露，属于"上线才炸"的坑。

**被否决**：默认落到 `Documents`——看起来更好找，但 OneDrive 重定向后同步训练产物会产生大量
上传流量。用户想放 Documents 可以在设置里改。

---

## D013 · 动效自己写，不引 framer-motion

**决定**：动效由 CSS 关键帧 + `lib/motion.ts` 里两个 hook（`useTween` / `usePresence`）实现，
不引入动画库。

**理由**：这个界面真正需要 JS 参与动效的地方只有两处——数字补间、以及"退出时还得先挂载"。
其余全是 CSS 就能表达的东西（入场、hover、药丸位移）。framer-motion 装进来约 30 kB gzip，
换到的是两条 rAF 逻辑，不划算。而且 CSS 关键帧天然跟着 `prefers-reduced-motion` 一起降级，
用库就得每个组件记得包 `useReducedMotion`。

**被否决**：
- *framer-motion / react-spring*：能力过剩，且会诱导把每个 hover 都写成 animate 调用，
  动效从"信息"变成"装饰"，与设计约定冲突。
- *Web Animations API*：能力够，但少了声明式的可读性；CSS 里能一眼看出全部缓动曲线。

**边界**：如果将来要做"共享元素转场"（卡片 → 详情），CSS 的 `view-transition-name` 优先，
仍不引库。

---

## D014 · 手势一律是「增强」，不是唯一路径

**决定**：每个手势都必须有一个显式的等价控件。横滑切页 ↔ 侧栏点击/Ctrl+1..4；
长按浮出标签 ↔ 展开侧栏后本来就有文字；拖动调参 ↔ 直接输入；拖入文件夹 ↔ 「选择目录」按钮。

**理由**：手势不可发现、不可被屏幕阅读器读到、且在某些输入设备上根本不存在（没有触控板的
台式机、远程桌面）。把功能只挂在一个手势上，等于对一部分用户隐藏功能。反过来，手势
存在的意义是让熟练用户少点几次，所以它必须**不打断**已有路径。

**具体约束**：
- 横向滑动必须"决定性横向"才生效（`|Δx| > 72` 且 `|Δx| > 1.8|Δy|`），否则会吃掉正常滚动。
- 日志自动跟随在用户上滑后暂停，并给出「跳到最新」——不能从正在阅读的人脚下抽走内容。
- 拖动调数值只绑在**标签**上，输入框保持可选中文本。

---

## D015 · 文件夹拖放注册在 App 层，不注册在数据集页

**决定**：`useWindowDrop` 挂在 `App.tsx`，掉落的文件夹直接扫描并跳到数据集页。

**理由**：拖放是"我已经知道要什么"的手势，用户不会先导航到某个页面再拖。挂在页面上会出现
"在监控页拖入 → 没反应 → 以为不支持 → 改用按钮"的路径。扫描本身是幂等的，全局接收没有歧义。

**代价**：任意页面都可能被一个误拖触发扫描。可接受——扫描只读目录，不改任何文件；失败时
用 toast 报错而不是抛异常。

---

## D016 · 训练参数在失焦时归一化，输入过程中只提示

**决定**：拖动时立即吸附到合法值；手动输入时不打断，失焦或回车才夹取范围、按步长取整
（分辨率 32 的倍数）。越界时输入框立刻变红并显示范围。

**理由**：边打字边纠正会把"100"打成"1"的中间态改成"64"，手感像输入法坏了。而完全不校验
又会让 641 这种值一路传到引擎，最后变成一条不指向字段的报错。

**被否决**：完全交给引擎报错——错误码无法定位到具体字段，用户还得回来猜。

---

## D017 · 控制服务用同步线程 + 手写 HTTP，不引 web 框架

**决定**：`control.rs` 自己解析 HTTP/1.1 请求行与头部，每个连接一个 `std::thread`，响应带
`Connection: close`。不引 `axum`、不引 `tiny_http`、不引 `tokio`。

**理由**：整个进程是阻塞 `std::thread` 模型（`engine.rs` 的 reader 线程、reaper、hub 订阅），
为了四个 GET 拉进 tokio 等于在一个同步应用里再养一个运行时。客户端只有一个（AI 或 curl），
每分钟几个请求，keep-alive 与并发连接池都是没有收益的复杂度。安装包体积是产品约束（D001），
`axum + hyper + tokio` 大约 1.5–2 MB，换到的是这条 200 行解析。

**被否决**：
- *axum*：路由与提取器确实舒服，但要为了它把整个 `Runtime` 改成 async，或者写一层
  `block_in_place` 之类的桥。为了一个端点表付这个代价不划算。
- *tiny_http*：轻得多，也够用；最终没选是因为它同样带来一个依赖，而这里需要的 HTTP 子集
  小到可以自己写清楚。**如果将来要支持分块上传或 WebSocket，应当换回 tiny_http 或 axum，
  不要再手写第二遍。**
- *SSE / WebSocket 推送*：见 D019。

**代价**：没有 chunked 编码、没有 gzip、没有 HTTP/2。客户端是脚本和 AI，都不需要。

---

## D018 · 控制服务默认开启，回环 + 固定令牌

**决定**：默认 `control_enabled: true`，只绑 `127.0.0.1`，除 `GET /` 外全部要求
`Authorization: Bearer <token>`。令牌 32 位十六进制，存在配置目录的 `control-token`，**重启不变**。

**理由**：
- *默认开*：这个功能的价值就是"装好就能让 AI 驱动"。默认关意味着用户要先知道它存在、
  找到设置页、打开、再复制提示词——四步里任何一步跳过，功能就等于不存在。
- *回环*：不监听 `0.0.0.0`，局域网内的其它机器连不上。这一条比令牌更重要，令牌是纵深。
- *令牌持久化*：用户会把令牌粘进对话里。如果每次重启换一个，粘过的提示词全部失效，
  而用户不会意识到是令牌变了，只会觉得"AI 突然不好使了"。
- *`GET /` 免鉴权*：AI 需要先读契约才能发第一个带令牌的请求；这份文档不含任何运行数据。

**被否决**：
- *只用回环、不要令牌*：本机上任何进程（包括浏览器里的网页脚本，只要它能发请求）都能
  起训练、改配置。令牌是六行代码的事。
- *令牌每次启动轮换*：安全上更干净，但会把"复制提示词"变成"每次开机重新复制"。
- *默认关闭*：见上。

**已知边界**：令牌是明文存在磁盘上的，同一台机器上的其它用户理论上可以读它。这个威胁模型
（多用户 Windows + 恶意本地程序）超出当前阶段，但要在对外分发前重新评估。

---

## D019 · 进度用游标轮询，不用 SSE

**决定**：`GET /state?since=N` 与 `GET /events?since=N` 返回"序号大于 N 的事件"和一个新游标。
不做 SSE、不做 WebSocket。

**理由**：训练一轮要 1.3 秒以上，轮询间隔 0.5–1 秒就足够跟手。游标轮询是幂等的——客户端
断线重连只要带上上次的游标，不会漏也不会重。SSE 需要一个长期占用的连接，而这个服务是
"每连接一个线程"的模型，每个 SSE 客户端就永久占一个线程，还要处理半开连接的心跳。
AI 客户端调用 curl / fetch 也更简单：一次请求一次响应，不需要事件流解析。

**代价**：最低延迟 = 轮询间隔。对训练进度无所谓；如果将来要推送"训练崩了"这种即时事件，
再考虑加一个 `GET /watch`（长轮询，最多挂 25 秒），仍然不引 SSE。

---

## D020 · 自动更新用官方 `tauri-plugin-updater`，不照搬 OCR-Studio 的自研方案

**决定**：NSIS 安装包 + minisign 签名 + GitHub Releases 的固定 `latest.json` 资产，客户端通过
官方插件下载并拉起安装器。参考了 `D:\Python测试\OCR-Studio` 的发版流程，但**替换了更新机制本身**。

**理由**：OCR-Studio 的 `bundle.active = false`，发的是免安装 zip，所以"更新 = 整目录覆盖"是
对的。本项目 `bundle.active = true` + NSIS，安装会写注册表、卸载项和开始菜单。若照搬覆盖法：

1. 覆盖后注册表里的版本/卸载信息仍指向旧版，控制面板显示错误版本，卸载可能删不干净。
2. `externalBin` 的 144 MB `montgomery.exe` 与 WebView2 加载的 DLL 在主进程运行时被占用，
   覆盖会 `ERROR_SHARING_VIOLATION`，留下半新半旧的目录。
3. 自研方案必须复制当前 exe 当"更新助手"，且**必须在单实例插件之前分流**，还得轮询 PID
   并校验进程镜像路径（Windows 会复用 PID）。这三个坑 OCR-Studio 的 `updater.rs` 都踩过。

官方插件让 NSIS 安装器接手，上述三类问题一并消失；签名校验也比自写 sha256 更强（防篡改）。

**借鉴了什么**：版本号唯一来源（`tauri.conf.json`）、`releases/latest/download/latest.json`
这个永不变更的 feed URL、发版全脚本化 + **核对 Release 资产而不是读退出码**、无 BOM JSON、
网络失败收敛成一句人话而不是红字报错。

**被否决**：
- *自研 zip 覆盖*：见上，与 NSIS 的分发模型冲突。
- *GitHub Actions 自动构建*：引擎二进制来自 AGPL 上游且 137 MiB，CI 上既要构建上游又要传大
  文件；本机脚本发版已足够，等真有多人协作再上 CI。

---

## D021 · GitHub 仓库必须是 public

**决定**：`QiuMr/yolo-trainer-lab` 以公开仓库创建。

**理由**：`releases/latest/download/<asset>` 对**私有**仓库的资产需要带令牌才能下载，而客户端
是匿名请求。私有仓库会让自动更新在第一次检查时就 404。这不是偏好问题，是机制约束。

**代价与缓解**：桌面壳源码公开可见（尚未选开源许可证，默认保留所有权利）；密钥、令牌、
本机路径、构建产物一律不入库（`.gitignore` + 发版前扫描）。若将来必须私有，得把 feed 挪到
可匿名读取的托管（R2 / Pages），客户端改一行 endpoint 即可——`tauri.conf.json` 里只有这一处。

---

## D022 · 版本号只有一个来源

**决定**：`desktop/src-tauri/tauri.conf.json` 的 `version`。`build.rs` 读它并注入
`YOLO_TRAINER_VERSION`；Tauri CLI 用它命名安装包；`scripts/release.ps1` 用它打 tag 和写
`latest.json`；Rust 的 `version.rs` 只读 `env!`。`Cargo.toml` / `package.json` 的 version 字段
不参与任何判断。

**理由**：三份版本号一定会漂。漂了的后果不是编译报错，而是客户端拿一个过时的版本去比，
**永远显示"已是最新"**——用户不会报 bug，只会一直用旧版。`build.rs` 还断言版本必须是三段
数字，拼错当场失败。

**被否决**：`env!("CARGO_PKG_VERSION")` —— 读的是 `Cargo.toml`，正是那个会漂的第二份。

---

## D023 · 签名私钥不入库，启用 `requireSignedVersion`

**决定**：私钥在 `%USERPROFILE%\.tauri\yolo-trainer.key`，`.gitignore` 明确排除 `*.key`；仓库里
只有公钥字符串。`tauri.conf.json` 设 `requireSignedVersion: true`。

**理由**：私钥泄漏等于任何人都能发布"通过校验"的假更新，这是不可回滚的事故。
`requireSignedVersion` 要求签名的 trusted comment 里必须带 `version:` 字段。实测 Tauri CLI
2.12.4 生成的 `.sig` **base64 解码全文**是
`timestamp:…\tfile:…\tversion:0.1.0`，确实带版本，所以启用没有兼容风险。它堵住的是：
有人拿一个不带版本字段的旧合法签名，配上虚高的 `latest.json.version` 来强制降级。
（插件即使不开这个开关，只要签名里带 version 也会比对一致性；开关额外要求"必须带"。）

**一次误判，记下来**：最初只看了 `.sig` 的前 300 个字符，见到 `timestamp` 与 `file` 就断定
"CLI 不写版本"，据此写了"不能启用"。完整解码才发现 `version:` 在 trusted comment 末尾。
**判断签名格式必须解码全文，不能 head 一段。**

**附带结论 1 · 产物形态**：NSIS 目标下 `createUpdaterArtifacts: true` 产出的是
`<setup>.exe` + `<setup>.exe.sig`，**没有 `.nsis.zip`**。签名字符串嵌进 `latest.json`，Release
只需上传安装包与 `latest.json` 两个资产。发版脚本最初按 `.nsis.zip` 写，是实测产物名之后改的。

**附带结论 2 · gh 会改资产名**：`gh release create` 把文件名里的空格替换成点
（`YOLO Trainer_…` → `YOLO.Trainer_…`）。后果是 `latest.json` 里 `%20` 编码的 URL 必然 404，
按本地文件名核对资产也永远不匹配。→ 上传前复制一份无空格名 `YOLO-Trainer_<ver>_…exe`，
URL、核对、补传统一用它；文件内容不变，签名仍然有效。

---

## D024 · 更新安装模式用 `quiet`，不用 `passive`

**决定**：`plugins.updater.windows.installMode = "quiet"`（NSIS `/S`），不用官方默认的
`passive`（`/P`）。

**实测经过**：v0.1.1 → v0.1.2 的演练里，客户端完成了检查、下载、验签、拉起安装器
（`%TEMP%` 里有 30 MB 的 `...-installer.exe`，旧进程 `exit 0`），但安装目录与注册表版本都没变，
新进程也没起来。**不是包或签名的问题**：手动拿同一个安装器做对照——

| 参数 | 结果 |
| --- | --- |
| 插件实际传的 `/P /UPDATE /R /ARGS` | 未落地（安装目录仍是 v0.1.1） |
| 手动 `/S /UPDATE /R /ARGS` | ✅ 注册表 `0.1.1 → 0.1.2`，`desktop.exe` 时间戳更新，`/R` 自动重启成功 |

两组只差 `/P` 与 `/S`，所以根因是 **passive 的进度窗口**：在 agent 的非交互 / 受 UIPI 约束
会话里弹窗无法完成，安装器退出却没安装。另外确认了空 `/ARGS` 无害（插件在
`restart_after_install` 时总会带上）。

**为什么不当成"本机特例"放过**：远程桌面、kiosk、服务会话、企业受限环境都可能复现，而
"点了更新，程序关了再也没起来"是用户最难自查的一类故障。我们的 UI 已经显示下载百分比和
"正在安装，程序将自动重启"，安装阶段再弹一个 NSIS 进度条是重复信息。

**代价**：用户看不到 NSIS 自己的安装进度。可接受——下载进度在应用内，安装只有几秒，
完成后 `/R` 自动启动新版。

**遗留**：完整 UI 链路（点按钮 → 自动装完 → 自动重启）仍需在**正常交互式桌面**人工点一次；
agent 会话只能验证到"拉起安装器"，最后一步用等价参数的手动运行验证。

---

## D025 · 续训入口与 ONNX 导出：能力已查清，暂不实现

**决定**：本轮**不做**。两项都不是技术障碍，是产品决策，需要先定方案再动手。这份记录的目的是
下次讨论时不用重新翻上游源码。

### 产品里其实有三种"模型"，语义不能混

实测来自 `artifacts/classify-runs/classify/train-1794327695-00006964`（分类小模型）：

| 路径 | 实测体积 | 内容 | 能干什么 |
| --- | --- | --- | --- |
| `exports/best.bpk` / `last.bpk` | 3.16 MB | 权重 + 架构元数据 | 推理（`predict --model`）；**当预训练权重用** |
| `checkpoints/best` / `last` | 24 MB | `model.bin` + `ema.bin` + `optimizer.bin` + `manifest.json` | **断点续训** |
| 导出的 `.safetensors` / `.onnx` | — | 中间产物 / 部署格式 | 只有导出时才产生 |

体积差 7 倍几乎全在 AdamW 的两个动量状态。`.bpk` 的 metadata 自带架构，所以
`train --model xxx.bpk` **不需要**再传 `--architecture`。

### 续训：引擎支持两种，壳层一个都没接

```console
train --architecture yolo26n   # 从零
train --model best.bpk         # 微调：新 run，优化器状态重置，只借权重
train --resume checkpoints/last # 断点续训：epoch 计数 / 优化器 / EMA / 数据集配置全继承
```

上游 README 原话：**"A Burnpack initializes a new run; it is not a resumable optimizer checkpoint."**
—— 想接着上次进度必须用 `--resume`，用 `.bpk` 会静默丢掉优化器动量和已训 epoch，这是最容易
做错的一处。

现状：`desktop/src-tauri/src/engine.rs` 的 spawn 只拼 `--architecture` + `--data`，
`TrainRequest` 也没有对应字段。**纯 UI 缺口，`TrainRequest` 要加初始化方式并同步 `docs/API.md`。**

### ONNX：不是"要装 Python"，是"图只能由 Python 搭"

`src/export/mod.rs` 模块注释（原文）：

> Burn does not export these ONNX graphs directly. This module therefore loads the requested model
> in Rust, snapshots its parameters to SafeTensors, and launches the pinned repository-owned Python
> graph adapter.

拆开看：**权重命名映射已经在 Rust 里做完了**（`src/export/keymap.rs` 是 importer 的逆向规则，
`burn_store::BurnToPyTorchAdapter` 负责 SafeTensors 交接）。Python 只承担剩下那一件事——按
ultralytics 源码重建计算图并 `torch.onnx.export`。因为 Burn 本身没有 ONNX 导出能力。

环境需求（`tools/onnx/requirements.lock.txt`）：Python 3.11~3.14、`torch==2.13.0`（单包下载约
200 MB、解压近 1 GB）、`onnx` / `onnxruntime` / `safetensors` / `numpy`，外加**一份姊妹目录的
ultralytics 源码**（不是 pip wheel），YOLOX 还需官方 `0.1.1rc0` checkout。
`--no-verify` 只跳过 Burn-vs-PyTorch 数值对比，**ONNX Runtime 校验永远必跑**。

→ 与"几十 MB、零环境"的产品前提直接冲突，客户端调这个命令的方案不可行。

### 三条候选路线（都评估过，未采纳）

1. **骨架 + 填权重**（真免环境，估 3~5 天）
   原理：图结构与权重正交，同一架构的图永远一样。我们侧装 Python 用真实权重导一次图 →
   权重清零保留 name/shape/dtype 得到骨架（每个几百 KB）→ 客户端读 `.bpk` 按 keymap 填进去。
   更省的实现：骨架用 `--external-data always` 导出，权重外置成 `weights.bin`，客户端只需按
   offset 写字节，**连 protobuf 都不用解析**。
   风险：骨架随上游图结构漂移需重生成（要跟 export spec 版本走）；5 任务 × 6 family × 3 profile
   的矩阵要用 `--dynamic-spatial` 压（但要确认下游 runtime 支持动态 shape）；`end2end` 把 NMS
   阈值烤进图里，阈值一改就得重导；`portable` 只出 raw head，下游得自己写 NMS。
2. **联网转换**（1~2 天）：客户端上传 `.bpk`，服务器跑现成 export 回传 `.onnx`。代价是服务器、
   依赖联网、权重出本机。
3. **只导 SafeTensors**（半天）：权重 + 元数据 JSON，Python 下游可用；C++/OpenCV/TensorRT 仍无解。

### 重新评估的触发条件

出现"模型要部署到没有训练器的环境"的真实需求时再做。优先选 1（符合免环境理念），
需求急且能接受联网时先用 2 过渡。

---

## D026 · ONNX 导出：骨架方案成立，但检测模型被上游结构错配卡住

**背景**：用户确认"用这个软件的人就是要拿模型去别处推理，一般就是 ONNX"，所以要做
**不装 Python 的 ONNX 导出**。本轮做了环境、验证和一处上游修复，结论如下。

### 已完成：方案可行性验证通过（分类模型）

用"权重清零 → 结构指纹比对 → 回填 → 数值比对"验证"图结构正交于权重"这条假设，
脚本 `spikes/qa/probe_onnx_skeleton.py`：

| 检查 | 结果 |
| --- | --- |
| 权重清零后图结构不变（op 序列 / 张量名 / shape / dtype 指纹） | ✅ 202 节点、80 个 initializer 前后完全一致 |
| 清零后的骨架通过 `onnx.checker` | ✅ |
| 回填原始权重后输出与原始模型一致 | ✅ **max\|diff\| = 0.000e+00**（逐字节） |
| initializer 名字唯一（name→offset 表可寻址） | ✅ 80 个无重复 |

→ **骨架 + 填权重方案成立**，可以在客户端用纯 Rust 完成，不需要 Python。

### 已完成：修掉一个致命上游 bug（类别数不读模型）

`ExportSpec::for_model` 的 `num_classes` 是**写死**的：分类 1000、语义 19、深度 1、其余 80。
Python 侧却用它建图（`classes = int(manifest["num_classes"])`），所以任何在自定义数据集上训的
模型都会在 state-dict 审计里失败（分类表现为 `2 wrong-shape keys`）。

已在 `src/export/mod.rs::export_onnx_inner` 里补：从 `artifact_metadata(weights).class_names`
读真实类别数覆盖 spec（仅对 detect/segment/classify）。实测输出
`Overriding model.yaml nc=1000 with nc=3`，3 类分类模型导出成功（6.17 MB，ORT 校验通过）。

### 阻塞 1（已解决）：检测头的隐藏通道下限被写成了常量

yolov8n（3 类）导出失败：**0 missing / 0 unexpected / 33 wrong-shape**，全部集中在
`model.22.cv3.*`（分类分支的所有张量，3 尺度 × 11）。

根因不是"两边公式不同"，而是**一行笔误**。`head.rs` 的注释自己写着 Ultralytics 的规则是
`max(ch[0], min(nc, 100))`，代码却写成 `p3_channels.max(DEFAULT_NUM_CLASSES.min(100))` ——
把 `nc` 写成了常量 80。更关键的是 `with_num_classes` 只改 `num_classes`、**不重算
`cls_channels`**，所以通道宽度在构造时就按 80 定死了：

| | 实际值 | p3=64, nc=3 |
| --- | --- | --- |
| 修复前 | `max(64, 80)` | **80** |
| Ultralytics | `max(64, min(3,100))` | **64** |

后果：训练完全正常（多余通道闲置），只有导出时才暴露。**只有 nc ≥ 80 的模型能导出**——
上游显然只在 COCO 上验证过这条路径。

修复：`with_num_classes` 里同步重算 `self.cls_channels = self.p3_channels.max(num_classes.min(100))`，
改 `yolov8` / `yolo11` / `yolo26` / `yolov10` 四个 head（`yolo12` 复用 yolo11 的 head，自动覆盖）。
**代价：改变模型结构，此前训的 checkpoint 不兼容，需要重训。**

### 阻塞 2（已解决）：yolo26 的辅助头被写进了导出快照

yolo26n 修完通道后从 `48 wrong-shape` 变成 **0 wrong-shape，102 unexpected**，且全部是
`head.o2m_p3/p4/p5`。

确认推理不需要它们：`Yolo26::forward()` 只调 `head.forward(...)`（one2one 单头），
`forward_train_dual` 才是 `head.o2m_*` 的唯一使用者 —— o2m 是训练辅助头，任何推理图都不可能消费它。

修复：`snapshot.rs` 里给 `SafetensorsStore` 加
`.filter(PathFilter::new().with_predicate(|path, _| !path.contains("o2m_")))`。
注意 `PathFilter` 默认 `new()` 是"不匹配任何路径"，且 `match_all` 状态下谓词会被忽略，
所以必须显式构造新 filter 而不是在默认上追加谓词。

**没有**选择"让 Python 审计忽略这个前缀"：那等于改测试来迁就产物，而"推理图不含辅助头"是
语义上正确的事，应该在产物生成时就做对。

### 实测结果（三种模型全部通过）

| 模型 | 导出 | 体积 | 骨架等价性 |
| --- | --- | --- | --- |
| 分类 3 类（yolo26n-cls） | ✅ ORT 校验通过 | 6.17 MB | ✅ max\|diff\| = 0 |
| 检测 3 类（yolov8n） | ✅ ORT 校验通过 | 11.7 MB | ✅ max\|diff\| = 0 |
| 检测 3 类（yolo26n） | ✅ ORT 校验通过 | 9.77 MB | ✅ max\|diff\| = 0 |

骨架规模：分类 202 节点 / 80 张量；yolov8 375 节点 / 127 张量；yolo26 514 节点 / 204 张量。

三份补丁在 `patches/`（`research/` 不入库，不留补丁下次构建出来的 exe 行为会不同）：

1. `0001-export-read-num-classes-from-checkpoint.patch` —— 类别数读模型而非常量
2. `0002-head-classification-tower-width.patch` —— 分类塔宽度按 nc 重算
3. `0003-drop-yolo26-auxiliary-head.patch` —— 导出时丢弃 o2m 辅助头

### 环境侧的坑（只影响我们复现，不影响产品）

上游 `tools/onnx/requirements.lock.txt` **不完整**：缺 `opencv-python`、`torchvision`、
`filelock`、`polars`、`nvidia-ml-py`、`ultralytics-thop`（`ultralytics/__init__.py` 一 import 就炸）。
另外直接装 `torchvision` 会把 `torch` 从 2.13.0 顶到 2.15.0，破坏 lock 的一致性 —— 必须
`uv pip install --index-url https://download.pytorch.org/whl/cpu "torch==2.13.0" torchvision`
让它按 torch 的 pin 解算（得到 torchvision 0.28.0）。

开发期环境落在 `research/montgomery/target/.venv`（`research/` 已 gitignore，不会入库）。

### 下一步（需决策）

1. 是否改 head 对齐 ultralytics（检测可导出，已训模型作废重训）
2. yolo26 双头怎么办：单独适配 keymap + Python 侧建双头图，或把 yolo26 排除在 ONNX 支持之外

---

## D027 · 免 Python 的 ONNX 导出：骨架 + 填权重，已实测通过

**背景**：D025/D026 之后用户明确"用这个软件的人就是要拿模型去别处推理，一般就是 ONNX"，
所以必须做成**用户机器上不需要 Python**。本条目记录最终方案与实测证据。

### 先纠正 D026 里一句说过头的话

D026 写的是"回填原始权重后输出一致 → 方案成立"。**那个实验是 ONNX 回填 ONNX，不是权重文件回填 ONNX**，
所以它只证明了"清零不改变图结构"，没证明"`.bpk` 的权重能放进图里"。真实链路有两个坑，
D026 当时没看到，这次必须记住：

1. **导出时 Conv+BN 已融合**：113/127 个初始化器叫 `onnx::Conv_*`（匿名），并且数值是
   `w * gamma / sqrt(var + eps)`，不是检查点里的原始张量。所以填充器必须自己做融合，
   而且 eps **不是常量**（`BnFlavor`：Ultralytics 1e-3、YOLOv8-cls 1e-5），要按家族记录。
2. **DFL 不是可学习参数**：`dfl.conv.weight` 在推理图里是固定的 `[0, 1, ..., 15]`，
   Burn 侧没有这个参数、检查点里也不写。骨架必须**保留它的真实字节**，否则所有框距离整体偏移。

### 类别数会从三个地方泄漏进图里

这是最花时间的一段。三类机制必须同时处理，只改其中一类会得到"能加载但输出错"的模型：

| 机制 | 例子 | 处理方式 |
| --- | --- | --- |
| 张量形状 | `cv3.*.2.weight` `[80,80,1,1]`、分类塔宽度 `max(p3, min(nc,100))` | 形状直接取自检查点张量 |
| `Constant` 节点的值 | `Slice` 终点 `4+nc`（84→7）、`Reshape` 的 `[1, nc, -1]` | 生成时对比 nc 与 nc-1 找出可变元素，填充时按类别差平移 |
| 节点整数属性 | 深度可分离塔的 `Conv.group` = 塔宽 | 同上找出，但**取值来自权重输出通道数**，不是 `nc`（塔宽是 `max(p3, nc)`，和 nc 不等） |

最后一行是踩过的坑：一开始把 `group` 也按类别差平移，yolo26 直接报
`Input channels C is not equal to kernel channels * group. C: 64 kernel channels: 1 group: 3`。

### 方案

- **骨架**：`tools/onnx_skeleton/generate.py` 在开发机用 torch 导出一张图，把权重清零
  （只留 name/dtype/dims），把融合权重改名成 `<模块>.conv.fused_weight` 并记录融合配方
  （含 eps），把随类别数变化的张量标 `class_dependent`，最后输出 `<model>.onnx` + `<model>.json`。
  生成时对每个参数加极小扰动：随机初始化会让所有 BN 融合偏置恰好为 0，torch 会把它们折叠成
  **一个共享常量 + 一堆 Identity 节点**，那不是真实检查点会产生的图。
- **填充**：引擎 `export --template <dir>`（补丁 0004）。读骨架 → 读检查点快照（复用
  `snapshot::write_snapshot`，名字映射沿用 `keymap`）→ 逐个初始化器写字节 → 改输出维度 →
  改 `Constant` / 整数属性 → 落盘。全程 Rust，无 Python、无联网。
- **protobuf**：手写极简编解码（`src/export/pb.rs`，~200 行）。理由一是两条消息不值得引 codegen
  依赖，二是本机 `CARGO_HOME` 在 D 盘且 D 盘已满，`cargo fetch` 装不了新 crate（实测
  `os error 112`）。未知字段原样保留，只重编码认识的字段。

### 实测（本机 CPU，ORT 1.23.2，单张随机输入）

用**真实的 3 类检查点**填**80 类骨架**，与官方 Python 导出对比：

| 模型 | 产物 | 框/输出 1 | 分数/输出 2 |
| --- | --- | --- | --- |
| yolov8n detect（3 类） | 12.26 MB | max 6.10e-5（相对 9.6e-8） | max 5.96e-8 |
| yolo26n detect（3 类） | 9.78 MB | max 7.63e-6（相对 1.2e-8） | max 2.98e-8 |
| yolo26n-cls（3 类） | 6.16 MB | logits max 5.59e-9 | probabilities **0.000e+00** |

差异量级 = float32 舍入（融合在 f32 下做、图里 100+ 层累积）。分类输出的概率逐元素完全相同。

骨架体积：detect 231–270 KiB、classify 26–38 KiB。当时只打包了 10 个作为验证；
**D028 之后按产品目录铺满 45 个**（12.06 MiB 原始，LZMA 后 35 KiB），生成耗时 9m37s。

### 覆盖面与已知限制

- 只有 `tools/onnx_skeleton/generate_all.py` 矩阵里的架构能导出，当前是 45 个（见 D028）。
  训练了没打包的架构（引擎 CLI 仍支持的那 18 个），导出会失败并说明缺骨架——
  **不会产出图与权重不匹配的坏模型**。
- 骨架跟 `export spec` 版本绑定：上游图结构一变，骨架必须重新生成（`generate_all.py` 重跑一次）。
- 只支持 fp32、`portable` profile、固定 batch/尺寸（骨架就是按这些生成的）。`end2end` 或
  `dynamic-spatial` 需要另一套骨架。
- 大模型（s 以上）没逐个验证，但走的是同一条代码路径，且 `verify()` 会比对每个非类别相关张量的
  形状，对不上直接报错而不是静默产出。

### 补丁 0004

`patches/0004-export-offline-template.patch`：`OnnxExportOptions.template` + `--template` 参数 +
`src/export/pb.rs` + `src/export/template.rs`。**必须带 `--features training,onnx` 编译。**

---

## D028 · 模型目录收窄到三个家族，界面与骨架一起砍

**背景**：D027 落地时安装包里只有 10 个骨架（yolov8/yolo11/yolo26 的 n/s × 三种任务里的一部分）。
铺满全部 63 个可训练架构要 63 个骨架。用户的问题是"哪些是常用的、哪些肯定用不到，只留能用到的"。
本次按此收窄。

### 决策

界面（`desktop/src/lib/models.ts`）与骨架（`tools/onnx_skeleton/generate_all.py`）**同时**收窄到：

**YOLO26 / YOLO11 / YOLOv8 × 检测 / 分割 / 分类 × n / s / m / l / x = 45 个**

砍掉的 18 个：

| 家族 | 可训练数 | 为什么不要 |
| --- | --- | --- |
| YOLOv10 | 6（检测 n/s/m/b/l/x） | 唯一卖点是免 NMS，而**本产品的默认架构 YOLO26 本身就是 NMS-free 且更快**，这个家族存在的理由已经消失。`b` 档（m 与 l 之间）更是全生态独一份的冷门 |
| YOLO12 | 5（检测 n/s/m/l/x） | 注意力架构，依赖 flash-attn，速度不如同规模 YOLO11，社区几乎没有采用 |
| YOLOX | 6（检测 nano/tiny/s/m/l/x） | 2021 年旷视，不属于 ultralytics 生态（要单独 checkout 官方仓库）。更要命的是档位命名异形（`nano`/`tiny`），逼着选择器为它写一套特例——D020 的 `resolveSelection` 回退逻辑就是为它写的 |
| YOLOv3-tinyu | 1（检测） | 上游自己的注释写着"对照基线"。这是开发者自查训练管线的工具，不是产品功能 |

### 被否决的方案

- **UI 保留 7 家族、只打包 3 家族骨架**：用户仍能选 YOLOX 训出模型，但点导出会缺骨架。
  这会制造"能训不能导"的隐藏坑——产品契约不自洽，否决。
- **保留全部 63**：见下面的体积实测。体积不是理由，**认知负担**才是——63 个组合里新手无法判断
  "YOLOX nano 和 YOLO26n 有什么区别"，而这个问题没有一句话能答。

### 体积：初估错了，必须更正

我先按"每个骨架 250 KB × 45 ≈ 10 MB"估了个 +8~10 MB 安装包。**实测是错的**：

| 项 | 数据 |
| --- | --- |
| 45 个骨架原始体积 | onnx 8.94 MiB + json 3.12 MiB = **12.06 MiB** |
| 同批文件 LZMA9e solid | **35 KiB（354.7x）** |
| 安装包 | 0.1.2 的 29.09 MiB → 0.1.3 的 **29.31 MiB（+229 KB）** |

原因：`zero()` 把每个初始化器的 payload 全丢掉，骨架里剩下的只有**节点名和 dims**，45 个文件之间
重复度极高，solid LZMA 的字典能整个吃掉。所以"多打包 18 个骨架"本来也不会让安装包涨多少。

结论：**体积从来不是这个决策的约束**。以后不要再拿体积为砍架构辩护。

### 代价与遗留

- 引擎**没改**，CLI 仍能 `train --architecture yolox-x`。只是产品界面不提供，也不随附骨架。
- 旧版本训出来的这 18 个架构的 run，在「导出 ONNX」时会缺骨架。`onnx_export.rs::explain()`
  把引擎那句 `skeleton manifest unreadable at ...` 翻译成中文，点名架构并给出"重训一次"的路径。
- 骨架清单是手动重生成的（`generate_all.py` 现在会顺手删掉矩阵外的旧骨架），与 export spec
  版本绑定这条限制不变。

### 这次改动的验证

| 项 | 怎么验的 |
| --- | --- |
| 45 个骨架与 manifest 自洽 | 逐个用 `onnx.load` 静态核对：张量名全在 graph 里、dims 一致、输入输出 shape 一致 → 45/45 OK |
| m 档新增骨架能端到端导出 | 训 `yolov8m`（640 输入 1 epoch）→ `export --template` 成功，167 张量，103.6 MB |
| 导出结果与引擎推理一致 | ORT 跑同一张图：分类分数与引擎逐位一致（51.6%），x 坐标 6 个框一致到 0.1 px。y 有 78.6 px 常数差，来自引擎的 stride 对齐 rect letterbox（pad_y=11）与 ORT 侧固定 640×640（pad=75）的差别，与骨架无关 |
| 选择器三家族 | CDP 实测：默认 `yolo26n`、切分割 `yolo26n-seg`、换 YOLOv8 `yolov8n-seg`、选 x `yolov8x-seg`、回检测 `yolov8x`、切分类阻断 → 全通过 |
| 桌面单测 | `cargo test --lib` **40 passed**（新增 2 项：缺骨架文案、无关错误原样透传） |
| 前端 | `tsc --noEmit` 干净、`vite build` 2.0s |
| 安装包 | `tauri build --bundles nsis` → 29.31 MiB，staging 里 90 个资源文件 |

---

## D029 · 预训练权重按需下载，不打进安装包

**背景**：D027 之后用户提出想要"用官方预训练权重开始训练"。上一轮实测了它的价值（见下），
但没有定产品形态。用户的判断是：**权重放 GitHub，需要时像自动更新一样下载**。

### 决策

安装包只带**一份清单**（`resources/pretrained/manifest.json`，15 条共几 KB：
架构名、大小、sha256、类别数、预训练数据集）。权重本身放在 GitHub Release `weights-v1`，
首次真正用某个架构训练时下载，缓存到 `%LOCALAPPDATA%\com.yolotrainer.desktop\weights\`。

- 下载走 `reqwest`（blocking），**复用 updater 插件已经引入的依赖**，不新增 crate
- 每 MB 发一次 `pretrained-progress` 事件，UI 显示进度条
- 下载完必须过 **sha256 + 长度**双重校验才改名落地；不通过就删掉并报错
- 缓存命中时 `ensure()` 不联网（仍校验一次摘要，防手改/半截文件）
- 离线可用：报错文案直接给出"把 `yolov8n.bpk` 放到 `<目录>`"的路径

### 被否决的方案

- **内置 n 档 9 个（51 MB）**：安装包 29 → 80 MB。看起来不大，但 `.bpk` 压缩率只有 **1.11x**
  （实测），一旦开了口子，"再内置 s 档""再内置 m 档"就没法拒绝，最后是几百 MB。
  按需下载还顺带解锁了 s/m/l/x——反正不占安装包。
- **全部内置（374 MB 检测 + 分割分类约 420 MB）**：把"几十 MB 安装包"这个卖点直接抹掉。
- **首次启动全量下载**：用户可能只训练一次分类，却要先下 50 MB。按架构惰性下载更合理。

### 体积实测（这是决策的依据，也是上次估算失准的更正）

| 项 | 数据 |
| --- | --- |
| `.bpk` LZMA9e 压缩率 | **1.11x**（真实权重，压不动） |
| 检测 n/s/m/l/x × 3 家族 | 16.4 / 59.1 / ~150 / ~250 / ~390 MiB |
| 15 个已转权重合计 | **373.9 MiB** |
| n 档 9 个（3 家族 × 3 任务） | **51.0 MiB** |

对比 D028：ONNX 骨架能压 355x，因为它是**没有数值的空壳**；权重是数值本身。两个"体积"结论
方向相反，不能互相套用。

### 预训练到底值多少：实测

`coco128` 抽 3 类（person/car/dog），60 张训练、15 张验证，320 输入，30 轮，yolov8n，同一台 CPU：

| 初始化 | 最终 mAP | 同一张验证图 |
| --- | --- | --- |
| 从零 | **0.013%** | No objects detected |
| COCO 预训练 | **3.22%** | person 38.3%，框贴合 |

**差 251 倍**，从零训练那个模型一个框都没检出。这是"预训练有价值"的硬证据。
注意这是**同域测试**（3 个类都在 COCO 里），所以 251 倍是收益上限，不是平均值。

### n 档的塔宽问题（本次一并修掉，见 patches/0005）

分类塔宽 `max(ch[0], min(nc, 100))` 只有 n 档（`ch[0] = 64`）会随类别数变。80 类权重 →
3 类模型时塔的两个卷积形状不同，原来的 `transfer_pretrained` 直接报 30 个 shape error。
改成"塔内形状不符视为新初始化，其余仍致命"，与 Ultralytics 的 `intersect_dicts` 一致。
修复前后都实测过：修复前 n 档微调失败，修复后正常且 mAP 3.22%。

### 已知限制

- 清单是**编译进二进制**的，加权重（比如补 seg 的 s/m/l/x）需要发一个小版本。
  想改成远端拉清单也容易，但那会让"清单和代码不一致"成为可能，暂不引入。
- GitHub 直连速度实测约 0.4 MB/s（本机网络），yolov8x（130 MB）约 5 分钟。慢但可完成，
  进度条就是为这个场景加的。
- 权重来自官方 Ultralytics 产物转换，转换脚本在 `artifacts/pretrained/convert.py`
  （开发机专用，用户侧永远不碰 `.pt`）。
