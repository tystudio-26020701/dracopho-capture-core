# dracopho-capture-core 使用教程

> 面向**使用方**（集成到 mark-shot 的开发者 / Python 调用方 / 部署工程师）的
> 操作向文档。架构原理与验证细节见 [`docs/engineering-report/`](../engineering-report/)。

## 教程索引

| 教程 | 面向 | 内容 |
| --- | --- | --- |
| [01-rust-quickstart.md](01-rust-quickstart.md) | Rust 集成方 | 从零到截图：添加依赖、构造请求、处理结果、路由指定 |
| [02-python-quickstart.md](02-python-quickstart.md) | Python 调用方 | 安装 wheel、10 行截图、全部 API 一览 |
| [03-authorization-deployment.md](03-authorization-deployment.md) | 部署工程师 | 首次授权 → token 持久化 → 无头部署 → 预检 |
| [04-multi-screen-window-recording.md](04-multi-screen-window-recording.md) | 所有使用方 | 多屏 vs 跨屏、窗口截图、滚动/录制 |

## 快速导航

- **5 分钟跑通**：`01-rust-quickstart.md` 或 `02-python-quickstart.md`
- **部署到无头机器**：`03-authorization-deployment.md`
- **多显示器 / 窗口 / 录制**：`04-multi-screen-window-recording.md`
- **运行完整验证**：`../../scripts/kde_regression.sh`（见 README "KDE Plasma 实机回归"）

---

Copyright © 2026 Beijing Taiyin Zaowu Technology Co., Ltd.
