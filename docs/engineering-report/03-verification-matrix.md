# 03 — 全面验证矩阵

> CLI / KDE / NVIDIA GPU / Python 绑定 / 单元测试逐项实测结果（截至 2026-08）。

## 1. 单元测试（Rust，无头）

| 目标 | 结果 |
| --- | --- |
| `cargo test --all-targets`（lib + bin + example） | ✅ 25 passed / 1 ignored / 0 failed |

覆盖：wlr_screencopy 像素转换、X11 像素转换、PipeWire 流匹配、crop 几何、
auth token 持久化、KWin JSON 解析、QImage 格式转换/预乘反预乘、路由启发式。

## 2. CLI 验证（GNOME Wayland 实机）

| 命令 | 结果 |
| --- | --- |
| `--list-backends` | ✅ `wlr-screencopy / pipewire-screencast / x11` |
| `--list-routing` | ✅ `session: wayland-gnome`，`Order([PipeWireScreencast, X11])` |
| `--list-outputs` | ✅ HDMI-1/HDMI-2 几何正确 |
| `--list-windows` | ✅ GNOME 扩展枚举 |
| `--capture-to --backend x11 --region` | ✅ 诚实报错（XWayland root 不可用，非挂起） |

## 3. KDE 回归（`scripts/kde_regression.sh`）

### 3.1 KWin 6.7.2 + llvmpipe EGL（X11 后端，无头 Xvfb）

| 检查项 | 结果 |
| --- | --- |
| 会话分类 `wayland-kde` | ✅ |
| 能力探测含 `kwin-screenshot2` | ✅ |
| 整屏链不含 kwin-screenshot2（授权门） | ✅ |
| 输出/窗口枚举（UUID + XWayland 0x 桥接） | ✅ |
| 窗口对象级抓取 `[object]` | ✅ |
| CaptureArea 区域抓取 | ✅ |
| **合计** | ✅ **PASS=11 FAIL=0**（多台服务器复测 PASS=12） |

### 3.2 KWin 6.7.3（neon）+ zenity 原生窗口

首台服务器（`region-9` 前身）验证：KWin scripting 枚举返回
`id={3b940b2c-...}` UUID，CaptureWindow 完整链路（UUID 传递/窗口查找/授权）
验证通过；修复 QUuid 对象提取后 PASS=9。

## 4. NVIDIA GPU 服务器（Tesla T4，region-9.autodl.pro）

### 4.1 驱动与渲染

| 项 | 结果 |
| --- | --- |
| `nvidia-smi` | ✅ Tesla T4 16GB，驱动 580.65.06，CUDA 13.0 |
| NVIDIA EGL | ✅ `EGL 1.5 vendor=NVIDIA` |
| **GPU 真实渲染**（`scripts/nvidia_egl_render_check.sh`） | ✅ 离屏渲染读回 `255,0,0,255` 纯红像素 |
| DRM 渲染节点 | ✅ `/dev/dri/renderD129` DRM_CAP_RENDERER=1 |

### 4.2 KWin 6.7.2（Debian sid）完整链路

| 测试项 | 结果 |
| --- | --- |
| 窗口枚举（2 个 zenity，各自 UUID） | ✅ |
| CaptureWindow by-UUID `[object]`（422×318 含装饰） | ✅ 真实像素（中心 240,240,240） |
| CaptureArea（400×300） | ✅ |
| CaptureWorkspace（1920×1080） | ✅ |
| 多窗口捕获（2 个 `[object]`） | ✅ |
| 组件子区域（120×80 裁剪） | ✅ |
| 完整回归 | ✅ **PASS=11 FAIL=0 SKIP=1** |

### 4.3 远程桌面（Xvnc + NVIDIA GLX）路线

用户指引"autodl 允许安装远程桌面做可视化操作"——实测可行并取得突破：

| 测试项 | 结果 |
| --- | --- |
| 安装并启动 Xvnc（TigerVNC，:1，VNC 5901） | ✅ |
| Xvnc 加载 NVIDIA GLX（`__GLX_VENDOR_LIBRARY_NAME=nvidia`） | ✅ `Tesla T4/PCIe/SSE2` / OpenGL 4.6.0 NVIDIA 580.65.06 |
| KWin 以 NVIDIA GLX 启动（renderer=Tesla T4） | ✅ |
| **CaptureWindow by-UUID（离屏渲染）在 NVIDIA 下** | ✅ 连续 3 次稳定 `[object]`，像素真实（240,240,240） |
| CaptureArea（屏幕级，需 X server NV-GLX 扩展） | ❌ Xvnc 无模块加载器 → 无法加载 libglxserver_nvidia → KWin 崩溃 |

### 4.4 决定性突破：完整 Xorg + NVIDIA GLX server（屏幕级也走真 GPU）

实测证明 **`modeset=Y` 非必需**：装完整 Xorg（有模块加载器）+ NVIDIA 用户态
xorg 模块（`nvidia_drv.so` + `libglxserver_nvidia.so`，与内核驱动同版本），
NV-GLX server 扩展可建立：

| 测试项 | 结果 |
| --- | --- |
| Xorg 加载 NVIDIA 驱动（`NVIDIA dlloader X Driver 580.65.06`） | ✅ |
| Xorg 加载 NVIDIA GLX server + `Initializing extension NV-GLX` | ✅ |
| NVIDIA 虚拟屏幕（2560×1600） | ✅ |
| KWin renderer = `Tesla T4/PCIe/SSE2` | ✅ |
| **CaptureArea（之前唯一失败项）** | ✅ `400x300 via kwin-screenshot2` |
| CaptureWindow by-UUID | ✅ `[object]` |
| KWin 稳定性（无 GL_OUT_OF_MEMORY） | ✅ |
| **完整回归** | ✅ **PASS=11 FAIL=0 SKIP=1** |
| Python 绑定 capture_frame | ✅ 300×200 PNG 1768B |

### 4.5 诚实结论（宿主限制）

- `kwin_wayland --drm`：T4 无 KMS（`drmIsKMS` 失败，宿主 `nvidia_drm modeset=N`
  只读）→ "No suitable DRM devices"；
- `kwin_wayland --virtual`：EGL layer 缓冲分配（GBM dumb / `/dev/udmabuf`）被
  cgroup 设备过滤拦截 → "Rendering a layer failed"。
- 均为宿主 GPU 配置限制，**非库代码缺陷**；
- **这些限制已被 §4.4 突破绕过**：装完整 Xorg + NVIDIA 用户态 xorg 模块
  （`nvidia_drv.so` + `libglxserver_nvidia.so`）即可建立 NV-GLX server 扩展，
  KWin 屏幕级合成也走真实 GPU，`modeset=Y` 非必需（详见 §4.4）。

## 5. Python 绑定（GPU 服务器 KDE 会话）

### 5.1 全功能验证脚本（74 项断言）

| 分组 | 覆盖 | 结果 |
| --- | --- | --- |
| 模块与常量 | 导入、DRM_FORMAT_MOD_* | ✅ |
| 路由感知 | detect_routing/RoutingPlan | ✅ |
| RouteMode | auto/only/order/prefer + 错误拒绝 | ✅ |
| CaptureRequest | 默认/全参构造 + getter | ✅ |
| 后端与输出 | available_backends/list_outputs | ✅ |
| 窗口枚举 | list_windows + WindowInfo 全字段 | ✅ |
| 窗口选择器 | WindowMatch 8 种 + parse_match | ✅ |
| 授权 | authorized/save/clear/verify | ✅ |
| 单帧捕获 | capture_frame（kwin 取帧 + PNG/RGBA/save） | ✅ |
| 多屏集合 | capture_outputs + output_name | ✅ |
| 窗口捕获 | capture_windows `[object]` | ✅ |
| 流式 | start_stream 诚实失败 | ✅ |
| **合计** | | ✅ **74/74 PASS** |

### 5.2 单元测试

`python/tests/test_api.py`：✅ 9/9（含新增 `test_module_constants`）。

### 5.3 验证中修复的缺陷

| 缺陷 | 修复 |
| --- | --- |
| Python 绑定缺 `DRM_FORMAT_MOD_*` 常量 | 补导出，与 Rust 对齐 |
| `capture_outputs` 的 output_name 后端回填不一致（x11/pipewire 返回 None） | 统一在 capture_outputs 内补齐屏幕名 |

## 6. 验证环境汇总

| 环境 | 用途 | 结果 |
| --- | --- | --- |
| 本机 GNOME Wayland + XWayland | 单元测试/CLI/路由感知 | ✅ |
| 服务器 A（Xvfb + KWin 5.27/6.7.3） | KDE 枚举/CaptureArea 首验 | ✅ |
| GPU 服务器 B（Tesla T4 + KWin 6.7.2 sid） | 全链路 + Python 74/74 | ✅ |
