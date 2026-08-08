# 02 — Python 绑定设计与 API 映射

> python/ 目录的 PyO3 绑定：覆盖库全部功能，单 abi3 wheel 支持 Python 3.8+。

## 1. 技术选型

| 项 | 选择 | 理由 |
| --- | --- | --- |
| 绑定框架 | PyO3 0.29 + maturin 1.14 | 直接包装现有 Rust API，零重复实现、自动类型转换 |
| 兼容性 | `abi3-py38` | 单个 wheel 覆盖 Python 3.8+，无需每版本打包 |
| 运行时依赖 | 无 | 图像以 PNG / 原始 RGBA 字节返回，不依赖 Pillow |
| lib 命名 | `dracopho_capture_core_py` | 避免与依赖 crate（同名 lib `dracopho_capture_core`）二义 |

## 2. API 映射（Rust → Python）

| Rust API | Python | 说明 |
| --- | --- | --- |
| `detect_routing()` | `detect_routing()` | → `RoutingPlan(session, recommended, route, notes)` |
| `RouteMode::Auto/Only/Order/Prefer` | `RouteMode.auto()/only()/order()/prefer()` | 字符串后端名（别名兼容） |
| `CaptureRequest` | `CaptureRequest(...)` | 全字段构造 + getter |
| `capture_frame` | `capture_frame(req)` | → `CaptureResult` |
| `capture_outputs` | `capture_outputs(req)` | → `List[CaptureResult]`（每屏一张，`output_name` 标识） |
| `capture_windows` | `capture_windows(req)` | → `List[WindowCapture]` |
| `start_stream` | `start_stream(req)` | → `Stream` |
| `Stream::next_frame/stop` | `stream.next_frame()/stop()` | 返回 `(png_bytes, frame_time_ms)` |
| `list_windows` | `list_windows(include_hidden=False)` | → `List[WindowInfo]` |
| `list_outputs` | `list_outputs()` | → `List[OutputInfo]` |
| `available_backends` | `available_backends()` | 字符串列表 |
| `WindowMatch` / `parse_match` | `WindowMatch.id/title/by_class/…` / `parse_match(spec, by)` | Python 关键字规避：class→by_class、instance→by_instance |
| `auth::*` | `authorized()/restore_token()/save_restore_token()/clear_restore_token()/verify_saved_token()/verify_restore_token()` | 授权与预检 |
| `stop_active_stream` | `stop_active_stream()` | 会话重置 |

### 类与字段

- **CaptureRequest**：`source_geometry` / `preferred_output` / `all_outputs` /
  `include_cursor` / `target_fps` / `minimum_frame_time_ms` /
  `allow_interactive_portal` / `hide_own_windows` / `window_matches` /
  `component` / `route`
- **CaptureResult**：`ok` / `error` / `backend` / `source_geometry` /
  `output_name` / `frame_time_ms` / `width` / `height` / `png()` / `rgba()` /
  `save(path)`
- **WindowCapture**：`window`(WindowInfo) / `selector` / `object_capture` /
  `error` / `png()` / `rgba()` / `width` / `height`
- **WindowInfo**：`id` / `title` / `window_class` / `instance` / `pid` /
  `geometry` / `monitor` / `workspace` / `z_order`
- **OutputInfo**：`name` / `geometry`
- **RoutingPlan**：`session` / `recommended` / `route` / `notes`

## 3. 模块级常量

```python
d.DRM_FORMAT_MOD_INVALID  # 0x00ff_ffff_ffff_ffff（与 drm_fourcc.h 一致）
d.DRM_FORMAT_MOD_LINEAR   # 0
```

## 4. 错误语义

- **参数错误**（无效后端、空 order、空选择器）→ `ValueError`
- **运行时失败**（捕获失败、DBus 不可用、流错误）→ `RuntimeError`
- **诚实失败**：`CaptureResult.ok == False` 时 `error` 字段为可读说明
  （如 `portal screencast requires interactive authorization; run once
  with the GUI to grant it`），不抛异常、不挂起、不弹窗。

## 5. 图像返回

- `png()`：PNG 编码字节（可直接写文件）；
- `rgba()`：原始 RGBA8 字节（宽×高×4），供 Pillow/numpy 等直接消费；
- `save(path)`：直接保存 PNG。

## 6. 多屏幕 vs 跨屏幕（Python）

```python
# 多屏幕集合（不拼接）：
for c in d.capture_outputs(d.CaptureRequest(all_outputs=True)):
    print(c.output_name, c.width, c.height)  # output_name 标识屏幕

# 跨屏幕区域（单张组合/裁剪）：
res = d.capture_frame(d.CaptureRequest(source_geometry=(0, 0, 1920, 1080)))
```

Wayland 下 `capture_frame(all_outputs=True)` 返回明确错误并引导使用
`capture_outputs`（与 Rust 库一致）。

## 7. 测试

- `python/tests/test_api.py`：9 项无头可运行单元测试（含 `test_module_constants`
  全 API 存在性检查）；auth 用独立 `XDG_CONFIG_HOME` 隔离。
- `python/examples/capture_demo.py`：真实捕获示例（需桌面会话 + 授权）。

## 8. 构建

```bash
# 环境：libpipewire dev + clang（libspa-sys 绑定生成）
export PKG_CONFIG_PATH=/path/to/libpipewire/pkgconfig
export LIBCLANG_PATH=/path/to/libclang
pip install maturin
maturin build --release    # → dracopho_capture_core-0.1.0-cp38-abi3-*.whl
```
