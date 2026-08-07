#!/usr/bin/env python3
"""dracopho-capture-core 真实捕获示例（需桌面会话）。

用法：
  python examples/capture_demo.py [out-dir]

依次演示：路由感知 → 全屏 → 区域 → 多屏幕集合 → 窗口捕获 → 流式。
首次运行在 GNOME/KDE 上会弹一次 ScreenCast 授权（allow_interactive_portal）。
"""

import os
import sys

from dracopho_capture_core import (
    CaptureRequest,
    RouteMode,
    capture_frame,
    capture_outputs,
    capture_windows,
    detect_routing,
    list_outputs,
    list_windows,
    parse_match,
    start_stream,
)

OUT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/dracopho-py-demo"


def main() -> int:
    os.makedirs(OUT, exist_ok=True)

    # 0) 智能路由感知
    plan = detect_routing()
    print(f"[0] session={plan.session} recommended={plan.recommended}")
    for n in plan.notes:
        print(f"    note: {n}")

    # 1) 全屏（首次会弹一次授权）
    req = CaptureRequest(source_geometry=None, allow_interactive_portal=True)
    res = capture_frame(req)
    if res.ok:
        p = os.path.join(OUT, "01-fullscreen.png")
        res.save(p)
        print(f"[1] fullscreen -> {p} ({res.width}x{res.height}) via {res.backend}")
    else:
        print(f"[1] fullscreen failed: {res.error}")

    # 2) 区域
    req = CaptureRequest(source_geometry=(0, 0, 800, 600))
    res = capture_frame(req)
    if res.ok:
        p = os.path.join(OUT, "02-region.png")
        res.save(p)
        print(f"[2] region 0,0,800x600 -> {p} ({res.width}x{res.height})")
    else:
        print(f"[2] region failed: {res.error}")

    # 3) 多屏幕集合（每屏一张，不拼接）
    for i, c in enumerate(capture_outputs(CaptureRequest(all_outputs=True))):
        name = c.output_name or f"screen-{i}"
        if c.ok:
            p = os.path.join(OUT, f"03-{name}.png")
            c.save(p)
            print(f"[3] {name} -> {p} ({c.width}x{c.height}) via {c.backend}")
        else:
            print(f"[3] {name} failed: {c.error}")

    # 4) 窗口列表 + 窗口捕获（按 class 匹配，改为你本机的窗口 class）
    for w in list_windows():
        print(f"[4] window title={w.title!r} class={w.window_class} geo={w.geometry}")
    req = CaptureRequest(window_matches=[parse_match("codium", "class")])
    for c in capture_windows(req):
        if c.png():
            p = os.path.join(OUT, f"04-{c.selector}.png")
            with open(p, "wb") as f:
                f.write(c.png())
            print(f"[4] window {c.selector} -> {p} object_capture={c.object_capture}")
        else:
            print(f"[4] window {c.selector} failed: {c.error}")

    # 5) 输出枚举
    for o in list_outputs():
        print(f"[5] output {o.name} geo={o.geometry}")

    # 6) 流式（拉 3 帧后停止）
    req = CaptureRequest(source_geometry=(0, 0, 640, 480), target_fps=5)
    stream = start_stream(req)
    pulled = 0
    while pulled < 3:
        frame = stream.next_frame(min_frame_time_ms=0, timeout_ms=1000)
        if frame is None:
            break
        png_bytes, t = frame
        p = os.path.join(OUT, f"06-frame-{t}.png")
        with open(p, "wb") as f:
            f.write(png_bytes)
        print(f"[6] stream frame @ {t}ms -> {p}")
        pulled += 1
    stream.stop()

    return 0


if __name__ == "__main__":
    sys.exit(main())
