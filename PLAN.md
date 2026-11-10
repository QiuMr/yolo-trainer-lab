# YOLO 轻量训练器研究计划

## 背景与目标

用户需求（按优先级）：

1. **任意显卡可训练**：NVIDIA / AMD / Intel / Apple Silicon 等现代 GPU 都能用，不强制 NVIDIA。
2. **分发包小**：exe 目标几十 MB 量级，拒绝 GB 级 CUDA 捆绑。
3. **用户零配置**：不要求用户安装 CUDA Toolkit / ROCm / cuDNN。
4. **模型对象**：YOLO26n 级别的小模型（参数量约 2.4M）。

技术路线共识：放弃 CUDA 专用栈，转向通用图形 API（Vulkan / DX12 / Metal / WebGPU），
通过 WGPU 抽象层 + JIT 内核编译实现跨 GPU，分发包只依赖系统显卡驱动。

## 候选方案

### 方案 A：boquila/montgomery（Rust + WGPU 原生）

- 定位：任意 GPU 上训练计算机视觉模型，直接支持 YOLO26n。
- 优点：单一原生 exe、体积可控、无需 Python 运行时。
- 风险：项目较新，YOLO 训练链路完整性、ONNX 模型格式（.bpk）导入流程未知。
- 重点验证：模型准备流程、训练一个 epoch 的最小闭环、Windows 上的构建可行性。

### 方案 B：Burn（Rust + WGPU，自建训练循环）

- 定位：从底层构建的 Rust DL 框架，Autodiff 装饰器 + WGPU/Vulkan 后端。
- 优点：体积最极致（可 <1MB 不含权重）、对体积/性能/兼容性完全可控。
- 风险：需要自行组织训练循环，burn-onnx 对 YOLO26n ONNX 图的覆盖度待验证。
- 重点验证：burn-onnx 导入 YOLO26n 的算子覆盖、WGPU 后端在目标显卡上的性能基线。

### 备选方案 C：tynx（Burn/WGPU 的 Python 封装）

- 仅在 A/B 都受阻时评估。wheel 约 15–20MB，但 PyInstaller 打包后 100MB 级别。

## 阶段划分

### 阶段 0：环境与资料（当前）

- [x] 建立项目目录结构
- [ ] 拉取 montgomery、Burn 源码与文档到 research/
- [ ] 记录版本、依赖、许可证

### 阶段 1：montgomery 可行性验证

- [ ] 按官方流程完成 Windows 构建
- [ ] 跑通 YOLO26n 训练示例（小数据集、1 epoch）
- [ ] 记录 exe 体积、构建依赖、实际 GPU 后端（DX12/Vulkan）
- [ ] 评估：能否满足体积 + 任意显卡两个硬指标

### 阶段 2：Burn 路线验证

- [ ] burn-onnx 导入 YOLO26n ONNX 模型，检查算子支持
- [ ] 搭建最小训练循环（dataloader + loss + optimizer）
- [ ] 在 WGPU 后端跑通前向/反向
- [ ] 对比阶段 1 的体积与性能数据

### 阶段 3：选型与原型

- [ ] 基于验证数据选定主路线
- [ ] 构建 exe 分发包原型，测量真实体积
- [ ] 在 NVIDIA / AMD / Intel（或可获取的硬件）上做兼容性冒烟测试
- [ ] 记录性能折损（相对 CUDA 基线）

### 阶段 4：产品化评估

- [ ] 用户使用流程设计（数据集准备、训练参数、产物导出）
- [ ] 错误处理与兜底（无 GPU / 老驱动 / 显存不足）
- [ ] 许可合规检查（montgomery / Burn 均为宽松许可？需确认）

## 关键风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| montgomery 对 YOLO26n 训练链路不完整 | 方案 A 失效 | 阶段 1 尽早跑最小闭环 |
| burn-onnx 不支持 YOLO 部分算子 | 方案 B 需手写算子 | 阶段 2 先做算子扫描 |
| WGPU 性能折损超预期 | 训练慢，用户不满 | 阶段 3 实测，必要时提供 CPU 兜底 |
| 打包体积超预期（依赖膨胀） | 违背核心诉求 | 阶段 3 用 release + LTO 实测 |
| 老显卡驱动不支持 Vulkan/DX12 | 兼容性缺口 | 记录最低驱动要求，明确支持矩阵 |

## 下一步行动

1. 克隆 montgomery 与 Burn 仓库到 research/ 下。
2. 阅读 montgomery 的 README 与训练示例，确认 YOLO26n 的模型格式转换方式。
3. 阅读 Burn 的 burn-onnx 文档，确认 ONNX 导入路径。

## 目录约定

- `docs/`：计划、调研笔记、结论
- `research/montgomery/`：montgomery 源码与实验
- `research/burn/`：Burn 源码与实验
- `spikes/`：一次性验证代码
- `artifacts/`：构建产物、体积/性能记录
