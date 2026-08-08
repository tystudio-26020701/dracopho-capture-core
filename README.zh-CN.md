<div align="center">

<img src="assets/TY-DracoPho.svg" width="96" alt="DracoPho 龙徽 logo"/>

# dracopho-capture-core

**DracoPho 自研截屏核心（Rust）**

一个屏幕捕获引擎库：以**开源标准协议 + 自研客户端**（PipeWire screencast /
wlr-screencopy）或自研直接抓取（X11 XComposite/XGetImage）为主干；KDE 窗口级
经 KWin ScreenShot2（铁律放宽后启用，对标 Spectacle）。**按桌面类型智能路由
到"最轻专用通道"**，并支持调用方参数化指定路由方案。

`MIT OR Apache-2.0` · Rust 库（供 mark-shot 等应用集成）

由 **北京太殷造物科技有限公司** 维护

</div>

## 特性

- **路由层（智能感知 + 参数化）**：`routing::detect_routing()` 按桌面/会话
  类型给出推荐后端与可直接回填的路由参数；`CaptureRequest.route`
  （`RouteMode::Auto/Only/Order/Prefer`）让调用方灵活切换指定模式。每桌面
  只用最轻专用通道：wlroots→wlr-screencopy（免 portal、免授权）、
  GNOME/KDE→portal ScreenCast（唯一合法像素通道）、KDE 窗口级→KWin
  ScreenShot2（静默、精确、零干扰）、原生 X11→XGetImage+XComposite。
- **不依赖 portal Screenshot / GNOME screenshot_area**：整屏/区域/录制走
  ScreenCast（PipeWire）或自研协议，绝不弹合成器选择器（KDE 窗口级除外，
  用 KWin ScreenShot2，同样静默）。
- **多种捕获模式**：全屏 / 区域 / **多屏幕（集合，每屏一张，`capture_outputs`）**
  / 跨屏幕组合（X11 原生）/ 多窗口 / 进程 / 窗口组件子区域。
- **授权一次、永久静默**：ScreenCast 授权经恢复 token 持久化（portal 轮换
  时自动跟随轮换），跨进程、跨重启复用，无需反复授权弹窗。无头模式下调用
  `Start` 前会**静默
  复核 token**（对照 portal 权限存储），失效/被撤销时快速失败并提示，绝不
  触发合成器选择器。预检（`auth::verify_saved_token`）既由库自动执行，也可由
  调用程序主动调用。
- **无头铁律**：不建窗口、不弹窗、不干扰用户其他进程。
- **流式捕获**：持续拉帧（滚动截图）带帧时间戳、陈旧帧过滤、帧率限速（录制）。
- **多流选屏**：使用 portal `multiple=true`，每个被选中的显示器各返回一个
  流（带 `position`/`size`）；`preferred_output` 名称解析为几何后与流匹配
  （未命中回退第一个）——与 GNOME/Ubuntu 截图工具相同的交互选屏模型，
  之后由恢复 token 绑定所选屏。
- **输出枚举**：Wayland 用自研 `wl_output` v4 客户端（名称 + 逻辑几何），
  X11/XWayland 用 XRandR；多屏感知。
- **X11 光标合成**：截图包含鼠标指针（XFixes），不依赖系统截图。
- **窗口枚举**：X11 自研 / GNOME 自研扩展 / KDE KWin scripting D-Bus（含
  窗口 UUID，供 ScreenShot2 抓取）；列出最小化/隐藏窗口，供 PID/进程定位。

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
| 授权一次、跨重启持久 | ✅ 恢复 token 持久化 | ❌ | — | — |
| DMABUF GPU 帧（EGL 导入） | ✅ 自研 | — | — | — |
| KDE 窗口真实内容（遮挡/最小化） | ✅ KWin ScreenShot2（铁律放宽） | ✅ | ❌ | ❌ |
| 不依赖 portal Screenshot / GNOME screenshot_area | ✅ **硬性保证** | ❌ | ✅ | ✅ |

¹ xdg-desktop-portal Screenshot / GNOME Shell `screenshot_area` —— 本项目
**不采用**；KDE 窗口级使用 KWin ScreenShot2（静默通道，铁律放宽后启用）。

## 铁律

- **不调用 portal Screenshot / GNOME screenshot_area**；KDE 窗口级用
  KWin ScreenShot2（静默、精确、零干扰）。
- **无头模式严禁弹窗**：未授权时直接报错退出并提示授权方式，绝不触发
  合成器选择器。
- 通道最小化：PipeWire（ScreenCast 门户 + 自研流客户端）、
  wlr-screencopy-unstable-v1（自研客户端）、X11 协议（x11rb）、
  KWin ScreenShot2（DBus，仅 KDE 窗口级）。

## 后端矩阵

| 后端 | 覆盖 | 说明 |
| --- | --- | --- |
| `pipewire-screencast` | GNOME / KDE / 所有支持 ScreenCast 门户的 Wayland 合成器 | 自研 PipeWire 客户端消费门户流；DMA-BUF 帧经自研 EGL 导入（缺 EGL 环境自动降级共享内存）；`multiple=true` 多流按 `position`/`size` 匹配选屏 |
| `wlr-screencopy` | wlroots 系（sway / hyprland / niri / river…） | 自研 wlr-screencopy 协议客户端 + wl_shm 读取，无需门户、零弹窗；支持区域抓取 |
| `kwin-screenshot2` | KDE Plasma（窗口级 / 区域 / 全屏） | 自研 KWin ScreenShot2 DBus 客户端（管道 FD 直读像素）；窗口级直接渲染目标窗口合成缓冲（遮挡/最小化真实内容）；整屏/区域仍优先 portal ScreenCast |
| `x11` | X11 会话 / XWayland | 自研 XGetImage 区域/全屏抓取 + XComposite 命名 pixmap 窗口内容抓取（含遮挡/最小化窗口真实内容） |

选择由路由层决定（`routing::detect_routing`）：wlroots→`wlr-screencopy`；
GNOME/KDE 整屏/区域/录制→`pipewire-screencast`（portal ScreenCast，唯一合法
像素通道，含授权门）；**KDE 窗口级**→`kwin-screenshot2`（经
`window_object_backends`，遮挡/最小化真实内容）；原生 X11→`x11`。
`CaptureRequest.route` 可参数化覆盖（`Only` / `Order` / `Prefer`）。
`kwin-screenshot2` 不在默认整屏链路里——无法绕过 portal 授权静默抓取整屏；
用 `Only(KwinScreenShot2)` / `Prefer(KwinScreenShot2)`（或 CLI
`--backend kwin-screenshot2`）显式启用放宽的 KDE 规则——**按设计，这种显式
指定会跳过 portal 授权门**，供明确放行 KWin ScreenShot2 的调用方使用
（含无头区域/整屏的 `CaptureArea`/`CaptureWorkspace`）。

## 授权机制（关键）

Wayland 合成器（GNOME/KDE）的安全模型决定：ScreenCast 是获取像素的唯一官方
途径，**首次使用必须经用户确认一次**。本库将其做到极限：

1. **交互授权**（`allow_interactive_portal=true`，仅首次）弹一次选择器；
2. **持久化授权**：`persist_mode=EXPLICITLY_REVOKED`，portal 保存权限，
   跨重启有效；
3. **token 持久化**：本库始终保存每次 Start 返回的 `restore_token`。portal
   规范称 token 单次轮换，但部分前端（GNOME 50 实证）保持同一 token 持续
   有效——无论哪种行为，授权链都永久延续（跨进程、跨重启，直到 portal 权限
   被撤销）；
4. **常驻进程持有会话**：库内静态复用 PipeWire 会话，同进程后续截图零弹窗。

> 无头模式绝不主动发起 Start；仅在保存的 token 仍有效时静默恢复，否则报错
> 并提示重新授权——宁可失败也不弹窗。
>
> **静默预检**：无头模式调用 portal `Start` 前，本库直接查询 portal 自己的
> 权限存储（`org.freedesktop.impl.portal.PermissionStore`，表 `screencast`），
> 复刻 portal 前端的判定：token 必须存在、权限授予当前进程解析出的
> `app_id`、带恢复数据，且（GNOME 下）引用的显示器仍在线。任一不满足即在
> `Start` 之前终止——失效 token / 撤销权限 / 显示器拔线都不会让合成器选择器
> 在后台截图时弹出。
>
> 预检是"可由库执行、也可由调用程序执行"的：库在无头 `Start` 前自动执行；
> 集成方（常驻托盘/守护）也可在录制启动前调用
> `auth::verify_saved_token()` 提前暴露"需要重新授权"（修复 GNOME 无头录制
> 因 token 失效被选择器阻塞的问题）。

## 库 API 用法

核心交付是 **Rust 库**（`dracopho_capture_core`），CLI 仅作验证工具。

### 路由：智能感知 + 参数化指定

```rust
use dracopho_capture_core::routing::detect_routing;
use dracopho_capture_core::capture_types::{capture_frame, CaptureRequest, Backend, RouteMode};

// 1) 智能感知：按桌面类型给出推荐方案，并返回可直接回填的路由参数。
let plan = detect_routing();
println!("session={}", plan.session.name());          // 如 wayland-gnome / wayland-kde / x11
for b in &plan.recommended { println!("  - {}", b.name()); }

// 2) 把感知到的路由参数固定下来（也可省略，默认 Auto 内部自动感知）。
let req = CaptureRequest {
    route: plan.route.clone(),   // 例如 Order([PipeWireScreencast, X11])
    ..Default::default()
};

// 3) 灵活切换指定模式：Only / Order / Prefer。
let req = CaptureRequest {
    route: RouteMode::Only(Backend::X11),            // 仅 X11，失败不回退
    ..Default::default()
};
let req = CaptureRequest {
    route: RouteMode::Prefer(Backend::KwinScreenShot2), // 优先 KDE ScreenShot2，失败回退自动推荐
    ..Default::default()
};
```

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

// 按名称捕获指定显示器（Wayland：与 portal 多流 position/size 匹配；X11：XRandR 几何）
let req = CaptureRequest {
    preferred_output: Some("HDMI-1".to_string()),
    ..Default::default()
};
```

### 多屏幕 vs 跨屏幕（严禁混淆概念）

- **多屏幕选择**（选中多个显示器）→ 返回**每个屏幕一张图的集合，绝不拼接**。
  使用 `capture_outputs`：

```rust
use dracopho_capture_core::capture_types::{capture_outputs, CaptureRequest};

// 每个显示器各一张图，用 result.output_name 标识对应屏幕；不拼接。
for c in capture_outputs(&CaptureRequest {
    all_outputs: true,
    ..Default::default()
}) {
    match c.image {
        Some(img) => println!("屏幕 {}: {}x{}", c.output_name.as_deref().unwrap_or("?"),
                              img.width(), img.height()),
        None => eprintln!("屏幕 {} 失败: {}", c.output_name.as_deref().unwrap_or("?"),
                          c.error.unwrap_or_default()),
    }
}
```

- **跨屏幕截图**（显式 `source_geometry` 区域跨越多个显示器；或 X11 原生的
  整虚拟桌面 `all_outputs=true`）→ 返回单张组合/裁剪图（允许的拼接例外）。
  Wayland 门户模型没有"虚拟桌面整流"，故 `capture_frame` 传 `all_outputs=true`
  会返回明确错误并引导使用 `capture_outputs`。

### 窗口 / 进程 / 组件截图

```rust
use dracopho_capture_core::capture_types::{capture_windows, CaptureRequest};
use dracopho_capture_core::window::{parse_match, WindowMatch};

// 窗口枚举（自研 X11 / 自研 GNOME 扩展）
for (i, w) in dracopho_capture_core::window::list_windows(true).iter().enumerate() {
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
2. **授权持久化**：token（随 portal 轮换或保持）存储于
   `~/.config/dracopho-capture-core/screencast-token`（0600），跨重启有效；
   即使主应用重启，新进程也会用 token 静默恢复，无需再次弹窗。
3. **窗口内容抓取**：X11 走 XComposite（真实窗口内容）；KDE 走 KWin
   ScreenShot2 `CaptureWindow`（UUID，遮挡/最小化真实内容）；GNOME/Wayland
   回退到全屏帧 + 窗口矩形裁剪（被遮挡窗口内容不可靠，`WindowCapture` 以
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
dracopho-capture --list-routing         # 打印当前会话智能路由方案（会话类型 + 推荐顺序 + 路由参数）
dracopho-capture --list-windows         # 列出窗口（JSON）
dracopho-capture --list-outputs         # 列出输出（wl_output / XRandR）
dracopho-capture --authorize            # 交互授权一次（保存恢复 token）
dracopho-capture --capture-to out.png   # 无头截图（静默）
dracopho-capture --capture-to out.png --region 0,0,1920,1080 --include-cursor
dracopho-capture --capture-to out.png --output HDMI-1   # 捕获指定显示器
dracopho-capture --capture-to out.png --backend x11     # 指定路由（only）：pipewire-screencast|wlr-screencopy|x11|kwin-screenshot2
dracopho-capture --capture-to dir --window VSCodium --window mark-shot \
                 --window-by auto --component 0,0,400,200
```

> CLI 为验证与调试用途，正确的库调用方式见上文 API 与集成指南。

## 工程技术报告

完整工程技术文档见
[`docs/engineering-report/`](docs/engineering-report/README.md)：架构与路由
设计、Python 绑定设计与 API 映射、完整验证矩阵（CLI / KDE / NVIDIA GPU /
Python / 单元测试）、KWin 6 + NVIDIA GPU 深度验证与诚实环境结论、
构建与发布最佳实践。

## KDE Plasma 实机回归

KDE 专属路径（KWin ScreenShot2 窗口级/区域、KWin scripting 窗口枚举、
XWayland X11 id 桥接）依赖正在运行的 KWin 合成器，无法在 GNOME/无头环境验证。
在任意 KDE Plasma Wayland 机器上运行一键回归脚本：

```bash
scripts/kde_regression.sh            # 构建 + 全项验证（KDE Plasma Wayland）
scripts/kde_regression.sh --no-build # 用已构建的 target/release/dracopho-capture
scripts/kde_regression.sh --python   # 追加 Python 绑定冒烟
scripts/kde_regression.sh --force-kde # 跳过会话检查（无头 Xvfb + kwin_x11 验证环境）
```

支持无头验证（无需真实 KDE 桌面）：Xvfb + `kwin_x11`（加
`KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1`、`LIBGL_ALWAYS_SOFTWARE=1`、
`EGL_PLATFORM=surfaceless`）+ 伪造
`XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=6`，
KWin X11 后端即运行在 Mesa llvmpipe EGL（软件渲染）上，KDE 路径全部
端到端可验：KWin scripting 窗口枚举、X11 id 桥接、ScreenShot2
`CaptureWindow` by-UUID `[object]` 与 `CaptureArea`——已在 KWin 6.7 +
zenity 原生窗口上验证（PASS=12 FAIL=0）。缺 EGL 环境变量时 KWin 回退
`KWin::VirtualBackend`（无 EGL 合成），ScreenShot2 取帧返回
"Screenshot got cancelled"；回归脚本检测到该状态会把相关断言降级为 SKIP。

### NVIDIA GPU 服务器验证（诚实记录）

已在带 Tesla T4 的 GPU 服务器上完成多方位验证：

| 测试项 | 结果 |
| --- | --- |
| NVIDIA EGL/CUDA 驱动可用（`nvidia-smi`、EGL vendor=NVIDIA 1.5） | ✅ |
| **NVIDIA GPU 真实渲染**（`scripts/nvidia_egl_render_check.sh` 离屏渲染读回 255,0,0 纯红像素） | ✅ |
| KWin 6.7.2（X11 后端 + llvmpipe EGL）完整回归 | ✅ PASS=11 FAIL=0 |
| 窗口枚举 / `CaptureWindow` by-UUID `[object]` / `CaptureArea` / 多窗口 / 组件子区域 | ✅ |
| Python 绑定（abi3 wheel）在 KDE 会话下全功能 | ✅ |

**NVIDIA GPU 合成（完整 Xorg + GLX server，无需 modeset=Y）**：`modeset=Y`
并非必需——`scripts/gpu_glx_xorg_setup.sh` 装完整 Xorg 并加载 NVIDIA 用户态
xorg 模块（`nvidia_drv.so` + `libglxserver_nvidia.so`，与内核驱动同版本），
建立 NV-GLX server 扩展后，KWin 屏幕级合成（CaptureArea/CaptureWorkspace）
也走真实 GPU。Tesla T4 上实测：renderer=Tesla T4，CaptureArea 400x300、
CaptureWindow `[object]` 422x318 真实出图，完整回归 PASS=11 FAIL=0。

脚本覆盖：会话分类（wayland-kde）、能力探测含 kwin-screenshot2、整屏链不含
kwin-screenshot2（portal 授权门保留）、输出/窗口枚举（UUID + XWayland 0x 桥接）、
窗口对象级抓取 `[object]`、显式 `--backend kwin-screenshot2` 区域抓取。
退出码：0=全通过 / 1=存在失败项 / 2=非 KDE 环境 / 3=构建失败。

## Python 绑定

`python/` 目录提供 PyO3 绑定，覆盖库的**每个功能**，单 abi3 wheel 支持
Python 3.8+，无运行时依赖：

```bash
pip install dracopho-capture-core   # 构建：cd python && maturin build --release
```

```python
from dracopho_capture_core import CaptureRequest, RouteMode, capture_frame

plan = detect_routing()                    # 会话类型 + 推荐后端
res = capture_frame(CaptureRequest(
    source_geometry=(0, 0, 800, 600),
    route=RouteMode.only("x11"),
))
if res.ok:
    open("shot.png", "wb").write(res.png())

# 多屏幕集合（不拼接）：capture_outputs(CaptureRequest(all_outputs=True))
# 流式：start_stream(req).next_frame(min_frame_time_ms, timeout_ms)
# 授权预检：verify_saved_token()
```

详见 `python/README.md` 与 `python/examples/capture_demo.py`；
测试 `python/tests/test_api.py`。

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
  capture_types.rs        公共类型与后端分发（capture_frame / capture_windows / RouteMode）
  routing.rs              路由层（SessionKind 智能感知 + RoutingPlan + resolve_route）
  window.rs               窗口枚举与选择（自研 X11 / GNOME 扩展 / KDE scripting）
  egl_dmabuf.rs           DMA-BUF EGL 导入（dlopen，缺 EGL 优雅降级）
  auth.rs                 授权恢复 token 持久化 + 无头预检（verify_restore_token / verify_saved_token）
  output.rs               输出枚举（wl_output v4 / XRandR）
  backend/
    pipewire_screencast.rs 自研 PipeWire 客户端（ScreenCast + EGL 导入）
    wlr_screencopy.rs      自研 wlr-screencopy 客户端（wl_shm）
    kwin_screenshot2.rs    自研 KWin ScreenShot2 客户端（DBus 管道直读，KDE 窗口级/区域）
    kwin_windows.rs        KDE 窗口枚举（KWin scripting D-Bus，纯 DBus 无 LGPL）
    x11.rs                 自研 X11 抓取（XGetImage + XComposite）
  bin/dracopho_capture.rs CLI 验证工具
examples/
  integration_demo.rs     库 API 集成示例
docs/
  engineering-report/     工程技术报告（架构/Python/验证矩阵/KWin6+GPU/最佳实践）
python/                   PyO3 绑定（maturin；模块 dracopho_capture_core）
  src/lib.rs              绑定实现
  tests/test_api.py       Python API 测试
  examples/capture_demo.py Python 用法示例
  README.md               Python 包说明
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
