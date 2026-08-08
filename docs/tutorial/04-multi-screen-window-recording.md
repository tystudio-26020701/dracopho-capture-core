# 04 — 多屏 / 窗口 / 录制

> 面向所有使用方：多显示器、窗口对象抓取、滚动/录制的最佳实践。

## 1. 多屏幕 vs 跨屏幕（严禁混淆）

| 语义 | API | 结果 |
| --- | --- | --- |
| **多屏幕选择**（选中多个显示器） | `capture_outputs` | **每个屏幕一张图，绝不拼接**；`output_name` 标识屏幕 |
| **跨屏幕区域**（显式区域跨越显示器） | `capture_frame(source_geometry)` | 单张组合/裁剪图（允许的例外） |
| X11 整虚拟桌面 | `capture_frame(all_outputs=true)` | 单张组合图（X11 原生支持） |
| Wayland `all_outputs=true` | `capture_frame` | 明确报错并引导用 `capture_outputs` |

**为什么分开**：Wayland portal 模型没有"虚拟桌面整流"，每个被选中的显示器
各返回一个流（带 position/size）。拼接会丢失屏幕边界语义，且不同分辨率/
缩放下无法正确合成。因此多屏场景的默认契约是**逐屏返回**。

### Rust

```rust
use dracopho_capture_core::capture_types::{capture_outputs, CaptureRequest};

fn main() {
    for c in capture_outputs(&CaptureRequest { all_outputs: true, ..Default::default() }) {
        match c.image {
            Some(img) => println!("screen {}: {}x{}", c.output_name.as_deref().unwrap_or("?"), img.width(), img.height()),
            None => eprintln!("screen {} failed: {}", c.output_name.as_deref().unwrap_or("?"), c.error.unwrap_or_default()),
        }
    }
}
```

### Python

```python
for c in d.capture_outputs(d.CaptureRequest(all_outputs=True)):
    print(c.output_name, c.width, c.height, c.ok)
```

## 2. 窗口对象抓取（遮挡/最小化真实内容）

| 平台 | 通道 | 行为 |
| --- | --- | --- |
| X11 原生 / XWayland | XComposite 命名 pixmap | 窗口自身合成缓冲，遮挡/最小化也真实 |
| KDE Plasma（Wayland） | KWin ScreenShot2 `CaptureWindow`（UUID） | 直接渲染窗口合成缓冲，遮挡/最小化也真实 |
| GNOME Wayland | 无窗口对象通道 | 回退全屏帧 + 窗口矩形裁剪（`object_capture=false` 如实标注） |

```rust
use dracopho_capture_core::capture_types::{capture_windows, CaptureRequest};
use dracopho_capture_core::window::WindowMatch;

fn main() {
    // 枚举窗口（X11 / GNOME 扩展 / KDE scripting 自动选择）
    for (i, w) in dracopho_capture_core::window::list_windows(true).iter().enumerate() {
        println!("[{i}] title={} class={} geo={:?}", w.title, w.class, w.geometry);
    }

    let req = CaptureRequest {
        window_matches: vec![WindowMatch::Class("codium".to_string())],
        ..Default::default()
    };
    for c in capture_windows(&req) {
        // c.object_capture == true 表示拿到窗口自身内容
    }
}
```

```python
# Python
for w in d.list_windows():
    print(w.title, w.window_class, w.geometry)
for c in d.capture_windows(d.CaptureRequest(
        window_matches=[d.WindowMatch.by_class("zenity")])):
    print(c.selector, c.object_capture, len(c.png() or b""))
```

**注意**：GNOME Wayland 下被遮挡/最小化窗口的"真实内容"无法获取——这是
合成器安全模型决定的物理上限，库如实标注（`object_capture=false`），调用方
应据此给出正确 UX，而不是假装有超能力。

## 3. 窗口组件子区域

```rust
use dracopho_capture_core::capture_types::CaptureRequest;
use dracopho_capture_core::window::parse_match;

fn main() {
    let req = CaptureRequest {
        window_matches: vec![parse_match("codium", Some("class")).expect("selector")],
        component: Some((0, 0, 200, 120)),   // 相对窗口左上角
        ..Default::default()
    };
    // ... 传入 capture_windows(&req)
}
```

```python
req = d.CaptureRequest(
    window_matches=[d.WindowMatch.by_class("codium")],
    component=(0, 0, 200, 120))
```

## 4. 滚动截图 / 录制（流式）

流式接口适合滚动截图与录制：持续拉取最新帧，带时间戳、陈旧帧过滤与帧率
限速。

```rust
use dracopho_capture_core::capture_types::{start_stream, CaptureRequest};

fn main() {
    let stream = start_stream(&CaptureRequest {
        source_geometry: Some((0, 0, 800, 600)),
        target_fps: 15,                 // 录制限帧
        ..Default::default()
    }).expect("start stream");

    // min_frame_time_ms：滚动截图隐藏自身 UI 后，用 now+delay 跳过陈旧帧
    let min_frame_time_ms = 0u64;       // 普通录制传 0 即可
    while let Some((frame, t)) = stream.next_frame(min_frame_time_ms, 1000).expect("next frame") {
        frame.save("frame.png").expect("save");
        println!("frame @ {t}ms");
    }
    stream.stop();
}
```

```python
stream = d.start_stream(d.CaptureRequest(source_geometry=(0, 0, 800, 600),
                                          target_fps=15))
while True:
    fr = stream.next_frame(min_frame_time_ms=0, timeout_ms=1000)
    if fr is None:
        break
    png_bytes, ts = fr
    open(f"frame-{ts}.png", "wb").write(png_bytes)
stream.stop()
```

**要点**：
- `next_frame(min_frame_time_ms, timeout_ms)`：只返回到达时间 ≥ min 的帧，
  超时返回 `None`；
- `target_fps` 限制拉取速率（录制）；流式仅由 PipeWire screencast 提供；
- 录制结束必须 `stop()` 释放共享会话。

## 5. 完整示例

- Rust：`examples/integration_demo.rs`（含多屏集合演示）
- Python：`python/examples/capture_demo.py`
