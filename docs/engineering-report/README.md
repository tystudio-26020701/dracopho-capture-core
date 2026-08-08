# dracopho-capture-core 工程技术报告

> DracoPho 自研截屏核心（Rust 库 + Python 绑定）工程化验证与设计文档集。

本目录记录该库的完整工程技术实践，覆盖架构设计、Python 绑定、验证矩阵、
KWin 6 + NVIDIA GPU 深度验证、构建与发布最佳实践。

## 文档索引

| 文档 | 内容 | 状态 |
| --- | --- | --- |
| [01-architecture.md](01-architecture.md) | 架构与设计：轻量专用通道 + 路由层、授权模型、多屏/跨屏语义、窗口对象抓取 | ✅ |
| [02-python-bindings.md](02-python-bindings.md) | Python 绑定设计：PyO3/abi3、API 映射、错误语义、常量 | ✅ |
| [03-verification-matrix.md](03-verification-matrix.md) | 全面验证矩阵：CLI/KDE/GPU/Python/单元测试逐项结果 | ✅ |
| [04-kwin6-gpu-verification.md](04-kwin6-gpu-verification.md) | KWin 6 + NVIDIA GPU 链路深度验证与诚实结论 | ✅ |
| [05-build-release-best-practices.md](05-build-release-best-practices.md) | 构建/发布/集成最佳实践（Rust crate + Python wheel） | ✅ |

## 核心结论摘要

- **架构**：没有一条万能重管道，只有一组"最轻专用通道"（wlr-screencopy /
  portal ScreenCast / X11 XComposite / KWin ScreenShot2）+ 按桌面分发的路由层
  （`routing.rs`），调用方可用 `RouteMode` 参数化指定路由方案。
- **Python 绑定**：PyO3 + abi3-py38 单 wheel 覆盖 Python 3.8+，零运行时依赖，
  图像以 PNG/原始 RGBA 字节返回；GPU 服务器 KDE 会话全面验证 **74/74 通过**。
- **KWin 6 + NVIDIA T4**：库全链路通过（窗口枚举/CaptureWindow by-UUID/
  CaptureArea/多窗口/组件/Python 绑定）；KWin GPU 合成受宿主
  `nvidia_drm modeset=N` 与 cgroup 设备过滤限制，如实记录而非掩盖。
- **诚实能力报告**：遮挡/最小化/降级如实上报（`object_capture`/`error`），
  无头模式失效 token 静默预检快速失败，绝不弹合成器选择器。

---

Copyright © 2026 Beijing Taiyin Zaowu Technology Co., Ltd.
