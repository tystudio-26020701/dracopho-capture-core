#!/usr/bin/env python3
"""dracopho-capture-core Python 绑定 API 测试（可无头运行）。

覆盖：路由感知 / RouteMode / CaptureRequest / WindowMatch / 枚举 / 授权 / 图像 I/O。
真实截图调用（capture_frame/capture_outputs/capture_windows/start_stream）依赖
桌面会话与授权，不在此硬断言，见 examples/ 示例。
"""

import os
import sys
import tempfile

import dracopho_capture_core as d


def test_module_constants():
    assert d.DRM_FORMAT_MOD_INVALID == 0x00ff_ffff_ffff_ffff
    assert d.DRM_FORMAT_MOD_LINEAR == 0
    assert hasattr(d, "detect_routing")
    assert hasattr(d, "capture_frame")
    assert hasattr(d, "capture_outputs")
    assert hasattr(d, "capture_windows")
    assert hasattr(d, "start_stream")
    assert hasattr(d, "list_windows")
    assert hasattr(d, "list_outputs")
    assert hasattr(d, "available_backends")
    assert hasattr(d, "parse_match")
    assert hasattr(d, "authorized")
    assert hasattr(d, "restore_token")
    assert hasattr(d, "save_restore_token")
    assert hasattr(d, "clear_restore_token")
    assert hasattr(d, "verify_saved_token")
    assert hasattr(d, "verify_restore_token")
    assert hasattr(d, "stop_active_stream")
    for cls in ("CaptureRequest", "CaptureResult", "WindowCapture",
                "RouteMode", "WindowMatch", "WindowInfo", "OutputInfo",
                "RoutingPlan", "Stream"):
        assert hasattr(d, cls), f"missing class {cls}"


def test_routing():
    plan = d.detect_routing()
    assert plan.session in (
        "wayland-gnome", "wayland-kde", "wayland-wlroots",
        "wayland-other", "x11", "unknown",
    )
    assert isinstance(plan.recommended, list)
    assert all(isinstance(b, str) for b in plan.recommended)
    assert isinstance(plan.notes, list)
    assert repr(plan.route)


def test_route_mode():
    for r in (
        d.RouteMode.auto(),
        d.RouteMode.only("x11"),
        d.RouteMode.order(["wlr-screencopy", "pipewire-screencast"]),
        d.RouteMode.prefer("kwin-screenshot2"),
    ):
        assert repr(r)
    for bad in ("bogus", ""):
        try:
            d.RouteMode.only(bad)
            raise AssertionError(f"accepted invalid backend {bad!r}")
        except ValueError:
            pass


def test_capture_request():
    req = d.CaptureRequest()
    assert req.source_geometry is None
    assert req.preferred_output is None
    assert req.all_outputs is False
    assert req.include_cursor is False
    assert req.target_fps == 0
    assert req.minimum_frame_time_ms == 0
    assert req.allow_interactive_portal is False
    assert req.hide_own_windows is True

    req = d.CaptureRequest(
        source_geometry=(0, 0, 800, 600),
        preferred_output="HDMI-1",
        include_cursor=True,
        target_fps=15,
        minimum_frame_time_ms=40,
        all_outputs=False,
        hide_own_windows=False,
        route=d.RouteMode.only("x11"),
    )
    assert req.source_geometry == (0, 0, 800, 600)
    assert req.preferred_output == "HDMI-1"
    assert req.include_cursor is True
    assert req.target_fps == 15
    assert req.hide_own_windows is False
    assert repr(req)


def test_window_match():
    m = d.parse_match("codium", "class")
    assert repr(m)
    assert repr(d.WindowMatch.id("0x2a00001"))
    assert repr(d.WindowMatch.title("foo"))
    assert repr(d.WindowMatch.by_class("codium"))
    assert repr(d.WindowMatch.by_instance("codium"))
    assert repr(d.WindowMatch.index(0))
    assert repr(d.WindowMatch.pid(1234))
    assert repr(d.WindowMatch.process("codium"))
    assert repr(d.WindowMatch.auto("codium"))
    req = d.CaptureRequest(window_matches=[d.WindowMatch.by_class("codium")],
                           component=(0, 0, 200, 120))
    assert repr(req)


def test_list_outputs():
    outs = d.list_outputs()
    for o in outs:
        assert o.name
        assert o.geometry[2] > 0 and o.geometry[3] > 0


def test_list_windows():
    ws = d.list_windows()
    for w in ws:
        assert isinstance(w.title, str)
        assert isinstance(w.window_class, str)
        assert len(w.geometry) == 4


def test_auth_isolated():
    # 用独立 XDG_CONFIG_HOME 隔离 token 文件，不触碰真实配置。
    old = os.environ.get("XDG_CONFIG_HOME")
    with tempfile.TemporaryDirectory() as td:
        os.environ["XDG_CONFIG_HOME"] = td
        d.clear_restore_token()
        assert d.restore_token() is None
        d.save_restore_token("test-token-abc")
        assert d.restore_token() == "test-token-abc"
        assert d.authorized()
        d.clear_restore_token()
        assert not d.authorized()
        # 无效 token 预检应返回 False 而非抛异常（DBus 不可用则 Err 上抛，
        # 无头 CI 环境允许该分支）。
        try:
            d.verify_restore_token("nonexistent-token")
        except RuntimeError:
            pass
    if old is None:
        os.environ.pop("XDG_CONFIG_HOME", None)
    else:
        os.environ["XDG_CONFIG_HOME"] = old


def test_stop_active_stream_noop():
    d.stop_active_stream()


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"  ok: {t.__name__}")
    print(f"ALL {len(tests)} PYTHON TESTS PASSED")
