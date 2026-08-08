# 02 — Python 快速上手

> 面向用 Python 调用 dracopho-capture-core 的开发者。目标：安装 wheel 后
> 10 行代码完成真实截图。

## 1. 安装

> **当前需本地构建**：该包尚未发布到 PyPI（发布后可直接
> `pip install dracopho-capture-core`）。

```bash
# 构建本地 wheel（需 Rust 工具链 + libpipewire dev + clang）
cd python
pip install maturin patchelf
maturin build --release

# 安装到当前 Python 环境
pip install target/wheels/dracopho_capture_core-*.whl
```

wheel 为 abi3-py38，覆盖 Python 3.8+，**零运行时依赖**（不依赖 Pillow；
图像以 PNG / 原始 RGBA 字节返回）。

## 2. 最小截图（10 行）

```python
import dracopho_capture_core as d

# 首次在桌面会话运行会弹一次授权，之后静默
req = d.CaptureRequest(source_geometry=None, allow_interactive_portal=True)
res = d.capture_frame(req)
if res.ok:
    open("screen.png", "wb").write(res.png())   # PNG 字节
    print(res.backend, res.width, res.height)
else:
    print("failed:", res.error)
```

## 3. 结果对象

`CaptureResult` 字段与方法：

| 成员 | 含义 |
| --- | --- |
| `ok` / `error` | 成功标志 / 可读错误 |
| `backend` | 实际后端名（pipewire-screencast / wlr-screencopy / x11 / kwin-screenshot2） |
| `width` / `height` | 图像尺寸 |
| `source_geometry` | 实际坐标 `(x, y, w, h)` |
| `output_name` | 命中的显示器名（多屏时区分屏幕） |
| `png()` | PNG 编码字节 |
| `rgba()` | 原始 RGBA8 字节（w×h×4） |
| `save(path)` | 直接保存 PNG |

## 4. 常用操作

```python
# 区域
res = d.capture_frame(d.CaptureRequest(source_geometry=(0, 0, 800, 600)))

# 指定显示器
res = d.capture_frame(d.CaptureRequest(preferred_output="HDMI-1"))

# 指定路由（仅用 KWin ScreenShot2）
res = d.capture_frame(d.CaptureRequest(
    source_geometry=(0, 0, 400, 300),
    route=d.RouteMode.only("kwin-screenshot2"),
))

# 多屏集合（每屏一张，不拼接，output_name 标识屏幕）
for c in d.capture_outputs(d.CaptureRequest(all_outputs=True)):
    print(c.output_name, c.ok, c.error)

# 窗口列表 + 窗口捕获
for w in d.list_windows():
    print(w.title, w.window_class, w.geometry, w.id)
req = d.CaptureRequest(window_matches=[d.WindowMatch.by_class("zenity")])
for c in d.capture_windows(req):
    if c.png():
        open(f"{c.selector}.png", "wb").write(c.png())
    print("object_capture:", c.object_capture)
```

## 5. 路由感知

```python
plan = d.detect_routing()
print(plan.session)          # wayland-gnome / wayland-kde / wayland-wlroots / x11 / ...
print(plan.recommended)      # ['pipewire-screencast', 'x11']
print(plan.route)            # 可直接赋给 CaptureRequest.route
```

## 6. 流式捕获（录制/滚动）

```python
stream = d.start_stream(d.CaptureRequest(source_geometry=(0, 0, 800, 600),
                                          target_fps=15))
while True:
    frame = stream.next_frame(min_frame_time_ms=0, timeout_ms=1000)
    if frame is None:
        break
    png_bytes, ts = frame
    open(f"frame-{ts}.png", "wb").write(png_bytes)
stream.stop()
```

## 7. 授权预检（无头部署前）

```python
from dracopho_capture_core import verify_saved_token, authorized
print("已授权:", authorized())
print("token 仍有效:", verify_saved_token())   # 无头录制前检查
```

## 8. 常见坑

| 现象 | 原因 | 解决 |
| --- | --- | --- |
| `RuntimeError: portal screencast requires interactive authorization` | 无有效 token | 首次交互授权或 `dracopho-capture --authorize` |
| `ValueError: unknown backend` | 后端名写错 | 用 `d.available_backends()` 查看 |
| `all_outputs=True` 报错 | Wayland 无虚拟桌面整流 | 用 `capture_outputs()` 逐屏 |

## 9. 完整示例与测试

- `python/examples/capture_demo.py`：真实捕获示例（需桌面会话 + 授权）
- `python/tests/test_api.py`：9 项无头单元测试
