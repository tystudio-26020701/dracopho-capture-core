<div align="center">

<img src="../assets/TY-DracoPho.svg" width="96" alt="DracoPho logo"/>

# dracopho-capture-core (Python)

**DracoPho 自研截屏核心 Python 绑定（PyO3）**

</div>

Rust 截屏库 [`dracopho-capture-core`](https://github.com/tystudio-26020701/dracopho-capture-core)
的 Python 绑定：PipeWire screencast / wlr-screencopy / X11 自研后端 + KWin
ScreenShot2（KDE 窗口级），**按桌面类型智能路由到最轻专用通道**。

- 单 wheel（abi3-py38）覆盖 Python 3.8+。
- 无第三方运行时依赖（不依赖 Pillow；图像以 PNG / 原始 RGBA 字节返回）。

## 安装

> **注意**：该包尚未发布到 PyPI。当前需本地构建 wheel 后安装；待发布后
> 可直接 `pip install dracopho-capture-core`。

```bash
pip install maturin patchelf
maturin build --release                       # → target/wheels/dracopho_capture_core-*.whl
pip install target/wheels/dracopho_capture_core-*.whl
```

## 快速上手

```python
from dracopho_capture_core import (
    CaptureRequest, RouteMode,
    capture_frame, capture_outputs, capture_windows, list_windows,
    list_outputs, available_backends, detect_routing,
    start_stream, parse_match,
)

# 0) 智能感知当前会话的路由方案
plan = detect_routing()
print(plan.session, plan.recommended)

# 1) 全屏截图（首次集成置 allow_interactive_portal=True 触发一次授权）
req = CaptureRequest(source_geometry=None, allow_interactive_portal=True)
result = capture_frame(req)
if result.ok:
    open("screen.png", "wb").write(result.png())   # 或 result.save("screen.png")
    print(result.backend, result.width, result.height)
else:
    print("failed:", result.error)

# 2) 区域截图 + 指定路由
req = CaptureRequest(source_geometry=(0, 0, 800, 600),
                     route=RouteMode.only("x11"))
result = capture_frame(req)

# 3) 多屏幕集合（每屏一张，不拼接）
for c in capture_outputs(CaptureRequest(all_outputs=True)):
    print(c.output_name, c.ok, c.error)

# 4) 窗口列表与窗口捕获
for w in list_windows():
    print(w.title, w.class, w.geometry, w.id)

req = CaptureRequest(window_matches=[parse_match("codium", "class")])
for c in capture_windows(req):
    if c.png():
        open(f"{c.selector}.png", "wb").write(c.png())
    print(c.object_capture, c.error)

# 5) 流式捕获（录制/滚动逐帧）
stream = start_stream(CaptureRequest(source_geometry=(0, 0, 800, 600), target_fps=15))
while True:
    frame = stream.next_frame(min_frame_time_ms=0, timeout_ms=1000)
    if frame is None:
        break
    png_bytes, frame_time_ms = frame
    open(f"frame-{frame_time_ms}.png", "wb").write(png_bytes)
stream.stop()

# 6) 授权预检（无头录制前）
from dracopho_capture_core import verify_saved_token, authorized
print("authorized:", authorized())
print("token still valid:", verify_saved_token())
```

## API 一览

| 功能 | Python 调用 |
| --- | --- |
| 单帧捕获（全屏/区域/指定输出/路由） | `capture_frame(request)` |
| 多屏幕集合（每屏一张，不拼接） | `capture_outputs(request)` |
| 窗口捕获（多选） | `capture_windows(request)` |
| 流式捕获 | `start_stream(request)` → `Stream.next_frame()/stop()` |
| 窗口枚举 | `list_windows(include_hidden=False)` |
| 输出枚举 | `list_outputs()` |
| 可用后端 | `available_backends()` |
| 智能路由感知 | `detect_routing()` → `RoutingPlan(session, recommended, route, notes)` |
| 窗口选择器 | `parse_match(spec, by)` / `WindowMatch.id/title/class/…` |
| 授权 | `authorized()` / `restore_token()` / `save_restore_token()` / `clear_restore_token()` / `verify_saved_token()` / `verify_restore_token()` |

### CaptureRequest 参数

`source_geometry` / `preferred_output` / `all_outputs` / `include_cursor` /
`target_fps` / `minimum_frame_time_ms` / `allow_interactive_portal` /
`hide_own_windows` / `window_matches` / `component` / `route`。

### 路由模式

- `RouteMode.auto()`：按桌面类型智能分发（默认）。
- `RouteMode.only("x11")`：仅指定后端，失败不回退。
- `RouteMode.order(["wlr-screencopy", "pipewire-screencast"])`：显式回退链。
- `RouteMode.prefer("kwin-screenshot2")`：优先指定后端，失败按自动推荐回退。

### 多屏幕 vs 跨屏幕（严禁混淆）

- **多屏幕选择** → `capture_outputs()`，返回每屏一张图的集合，**绝不拼接**。
- **跨屏幕截图**（显式 `source_geometry` 区域跨越显示器；X11 整虚拟桌面）→
  `capture_frame()` 单张组合/裁剪图。Wayland 无虚拟桌面整流，`capture_frame`
  传 `all_outputs=True` 会返回明确错误并引导使用 `capture_outputs`。

## 构建

```bash
# 环境依赖：libpipewire-0.3 开发包 + clang（libspa-sys 绑定生成）
export PKG_CONFIG_PATH=/path/to/libpipewire/pkgconfig
export LIBCLANG_PATH=/path/to/libclang
pip install maturin
maturin build --release
```

## 许可证

**`MIT OR Apache-2.0`** 双许可（与底层 Rust 库一致）。品牌与 logo 归
北京太殷造物科技有限公司所有，不随许可发放。

---

Copyright © 2026 Beijing Taiyin Zaowu Technology Co., Ltd.
