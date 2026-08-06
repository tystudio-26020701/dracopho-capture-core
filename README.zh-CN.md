<div align="center">

<img src="assets/TY-DracoPho.svg" width="96" alt="DracoPho 龙徽 logo"/>

# dracopho-capture-core

**DracoPho 自研截屏核心（Rust）**

一个**不依赖任何"系统自带截图"服务**的屏幕捕获引擎库：仅使用开源标准协议 +
自研客户端（PipeWire screencast / wlr-screencopy）或自研直接抓取
（X11 XComposite/XGetImage）。

`MIT OR Apache-2.0` · Rust 库（供 mark-shot 等应用集成）

由 **北京太殷造物科技有限公司** 维护

</div>

## 特性

- **纯自研后端**：不调用 xdg-desktop-portal Screenshot、GNOME Shell
  `screenshot_area`、KWin ScreenShot2 等任何合成器内置截图服务。
- **多种捕获模式**：全屏 / 区域 / 全部输出 / 多窗口 / 进程 / 窗口组件子区域。
- **授权一次、永久静默**：ScreenCast 授权经恢复 token 持久化与自动轮换，
  跨进程、跨重启复用，无需反复授权弹窗。
- **无头铁律**：不建窗口、不弹窗、不干扰用户其他进程。
- **流式捕获**：持续拉帧（滚动截图）带帧时间戳、陈旧帧过滤、帧率限速（录制）。
- **输出选择**：枚举物理输出（XRandR），按输出名截图，多屏感知。
- **X11 光标合成**：截图包含鼠标指针（XFixes），不依赖系统截图。
- **隐藏窗口枚举**：列出最小化/隐藏窗口，供 PID/进程定位。

## 功能对比列表

| 能力 | **dracopho-capture-core** | 系统自带截图服务¹ | 外部工具（grim） | Qt `grabWindow` |
| --- | --- | --- | --- | --- |
| 纯自研捕获 | ✅ | ❌ 系统服务 | ✅（第三方） | ✅（Qt 框架） |
| Wayland（GNOME/KDE） | ✅ 自研 PipeWire screencast | ✅ 门户 | ❌ 仅 wlroots | ❌ |
| Wayland wlroots（sway/hyprland/niri） | ✅ 自研 wlr-screencopy，免门户 | ✅ 门户 | ✅ | ❌ |
| X11 全屏/区域截图 | ✅ 自研 XGetImage | ✅ | ✅ | ✅ |
| X11 窗口自身内容（遮挡/最小化） | ✅ XComposite 命名 pixmap | ❌ | ❌ | ❌ |
| 窗口/组件/多窗口选择 | ✅ 库 API | ❌ | ❌ | ❌ |
| 进程（pid/名称）目标 | ✅ | ❌ | ❌ | ❌ |
| 无头静默（不建窗、不弹窗） | ✅ 严格保证 | ❌ 交互式门户 | ✅ | ✅ |
| 授权一次、跨重启持久 | ✅ token 自动轮换 | ❌ | — | — |
| DMABUF GPU 帧（EGL 导入） | ✅ 自研 | — | — | — |
| 不依赖系统截图 | ✅ **硬性保证** | ❌ | ✅ | ✅ |

¹ xdg-desktop-portal Screenshot / GNOME Shell `screenshot_area` /
KWin ScreenShot2 —— 均为本项目**明令禁止**使用的系统截图服务。

## 铁律

- **严禁调用系统自带截图**（portal Screenshot / GNOME screenshot_area /
  KWin ScreenShot2）。
- **无头模式严禁弹窗**：未授权时直接报错退出并提示授权方式，绝不触发
  合成器选择器。
- 仅使用开源标准协议：PipeWire（ScreenCast 门户 + 自研流客户端）、
  wlr-screencopy-unstable-v1（自研客户端）、X11 协议（x11rb）。

## 后端矩阵

| 后端 | 覆盖 | 说明 |
| --- | --- | --- |
| `pipewire-screencast` | GNOME / KDE / 所有支持 ScreenCast 门户的 Wayland 合成器 | 自研 PipeWire 客户端消费门户流；DMA-BUF 帧经自研 EGL 导入（缺 EGL 环境自动降级共享内存） |
| `wlr-screencopy` | wlroots 系（sway / hyprland / niri / river…） | 自研 wlr-screencopy 协议客户端 + wl_shm 读取，无需门户、零弹窗；支持区域抓取 |
| `x11` | X11 会话 / XWayland | 自研 XGetImage 区域/全屏抓取 + XComposite 命名 pixmap 窗口内容抓取（含遮挡/最小化窗口真实内容） |

选择优先级：`wlr-screencopy` → `pipewire-screencast` → `x11`，逐个回退。

## 授权机制（关键）

Wayland 合成器（GNOME/KDE）的安全模型决定：ScreenCast 是获取像素的唯一官方
途径，**首次使用必须经用户确认一次**。本库将其做到极限：

1. **交互授权**（`allow_interactive_portal=true`，仅首次）弹一次选择器；
2. **持久化授权**：`persist_mode=EXPLICITLY_REVOKED`，portal 保存权限，
   跨重启有效；
3. **token 自动轮换**：`restore_token` 是单次的，本库在每次 Start 后保存
   返回的新 token，授权链永久延续（跨进程、跨重启，直到 portal 权限被撤销）；
4. **常驻进程持有会话**：库内静态复用 PipeWire 会话，同进程后续截图零弹窗。

> 无头模式绝不主动发起 Start；仅在保存的 token 仍有效时静默恢复，否则报错
> 并提示重新授权——宁可失败也不弹窗。

## 库 API 用法

核心交付是 **Rust 库**（`dracopho_capture_core`），CLI 仅作验证工具。

### 屏幕 / 区域截图

```rust
use dracopho_capture_core::capture_types::{capture_frame, CaptureRequest};

// 全屏（首次集成时置 allow_interactive_portal=true 触发一次授权）
let req = CaptureRequest {
    source_geometry: None,
    allow_interactive_portal: true, // 仅首次；此后恒 false
    ..Default::default()
};
let result = capture_frame(&req);
if let Some(img) = result.image {
    img.save("screen.png")?;
}

// 区域（屏幕坐标）
let req = CaptureRequest {
    source_geometry: Some((0, 0, 800, 600)),
    ..Default::default()
};

// 全部输出（多屏合成一张）
let req = CaptureRequest { all_outputs: true, ..Default::default() };
```

### 窗口 / 进程 / 组件截图

```rust
use dracopho_capture_core::capture_types::{capture_windows, CaptureRequest};
use dracopho_capture_core::window::{parse_match, WindowMatch};

// 窗口枚举（自研 X11 / 自研 GNOME 扩展）
for (i, w) in dracopho_capture_core::window::list_windows().iter().enumerate() {
    println!("[{i}] title={} class={} geo={:?}", w.title, w.class, w.geometry);
}

// 多窗口（按 class + 标题子串 + pid，可重复）
let req = CaptureRequest {
    window_matches: vec![
        WindowMatch::Class("codium".to_string()),
        parse_match("DracoPho", Some("title"))?,
        WindowMatch::Pid(1234),
    ],
    ..Default::default()
};
for c in capture_windows(&req) {
    if let Some(img) = c.image { img.save(format!("{}.png", c.window.title))?; }
}

// 窗口组件子区域（窗口内相对坐标）
let req = CaptureRequest {
    window_matches: vec![parse_match("codium", Some("class"))?],
    component: Some((0, 0, 200, 120)),
    ..Default::default()
};
```

`WindowMatch` 支持：`Id` / `Title` / `Class` / `Instance` / `Index` / `Pid` /
`Process`（/proc 进程名）/ `Auto`（自动匹配）。`parse_match(spec, by)` 提供与
C++ `--window-by` 一致的选择器解析。

完整示例见 `examples/integration_demo.rs`。

## 集成指南（主应用 mark-shot）

1. **常驻进程持有会话**：库的 PipeWire 会话在进程内静态复用。主应用
   （mark-shot 常驻托盘/守护）首次调用带 `allow_interactive_portal=true`
   完成授权，此后同进程截图**零弹窗**。
2. **授权持久化**：token 自动轮换存储于
   `~/.config/dracopho-capture-core/screencast-token`（0600），跨重启有效；
   即使主应用重启，新进程也会用 token 静默恢复，无需再次弹窗。
3. **窗口内容抓取**：X11 走 XComposite（真实窗口内容）；GNOME/Wayland 回退
   到全屏帧 + 窗口矩形裁剪（被遮挡窗口内容不可靠，`WindowCapture` 以
   `object_capture` / `error` 字段如实标注）。
4. **滚动截图 / 录制**：用流式接口替代反复单帧截图：

   ```rust
   use dracopho_capture_core::capture_types::{start_stream, CaptureRequest};

   let stream = start_stream(&CaptureRequest {
       source_geometry: Some((0, 0, 800, 600)),
       target_fps: 15,          // 可选帧率限速（录制）
       ..Default::default()
   })?;
   // 拉最新帧；滚动截图隐藏自身 UI 后用 now+delay 设 min_frame_time_ms 跳过陈旧帧。
   while let Some((frame, t)) = stream.next_frame(min_frame_time_ms, 1000)? {
       frame.save("frame.png")?;
   }
   stream.stop();
   ```

   `Stream::next_frame` 返回 `(RgbaImage, frame_time_ms)`；`target_fps` 限制
   拉取帧率、`minimum_frame_time_ms` 过滤陈旧帧——两者均已接入 PipeWire
   screencast 后端。

## CLI（验证工具）

```bash
dracopho-capture --list-backends        # 列出可用自研后端
dracopho-capture --list-windows         # 列出窗口（JSON）
dracopho-capture --list-outputs         # 列出输出（XRandR）
dracopho-capture --authorize            # 交互授权一次（保存恢复 token）
dracopho-capture --capture-to out.png   # 无头截图（静默）
dracopho-capture --capture-to out.png --region 0,0,1920,1080 --include-cursor
dracopho-capture --capture-to dir --window VSCodium --window mark-shot \
                 --window-by auto --component 0,0,400,200
```

> CLI 为验证与调试用途，正确的库调用方式见上文 API 与集成指南。

## 构建

```bash
# 依赖：libpipewire-0.3 开发包（Debian/Ubuntu: apt install libpipewire-0.3-dev）
export PKG_CONFIG_PATH=/path/to/libpipewire/pkgconfig
cargo build --release
cargo test
```

> 构建还需 clang/bindgen（libspa-sys 绑定生成）；环境缺少时设置
> `LIBCLANG_PATH` 与 `BINDGEN_EXTRA_CLANG_ARGS`（指向 clang 内置头）。

## 目录结构

```
src/
  capture_types.rs        公共类型与后端分发（capture_frame / capture_windows）
  window.rs               窗口枚举与选择（自研 X11 / 自研 GNOME 扩展）
  egl_dmabuf.rs           DMA-BUF EGL 导入（dlopen，缺 EGL 优雅降级）
  auth.rs                 授权恢复 token 持久化
  backend/
    pipewire_screencast.rs 自研 PipeWire 客户端（ScreenCast + EGL 导入）
    wlr_screencopy.rs      自研 wlr-screencopy 客户端（wl_shm）
    x11.rs                 自研 X11 抓取（XGetImage + XComposite）
  bin/dracopho_capture.rs CLI 验证工具
examples/
  integration_demo.rs     库 API 集成示例
assets/
  TY-DracoPho.svg         项目 logo（品牌资产，单独保留版权）
```

## 许可证

**`MIT OR Apache-2.0` 双许可**（见 `LICENSE`）：集成方任选其一，可自由嵌入
到自己的应用（含闭源商业应用），仅需保留版权声明。

**品牌与 logo 保留**：`assets/TY-DracoPho.svg` 及 "DracoPho" / "太殷龙摄"
名称与商标归 DracoPho 项目方（北京太殷造物科技有限公司）所有，**不随上述
许可发放**，不得用于与本项目无关的产品或宣传。

---

Copyright © 2026 北京太殷造物科技有限公司
