# 05 — 构建 / 发布 / 集成最佳实践

> Rust crate + Python wheel 的工程化实践。

## 1. Rust 库构建

```bash
# 环境依赖：libpipewire-0.3 dev + clang（libspa-sys 绑定生成）
export PKG_CONFIG_PATH=/path/to/libpipewire/pkgconfig
export LIBCLANG_PATH=/path/to/libclang
export BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-linux-gnu/15/include -isystem /usr/include/x86_64-linux-gnu -isystem /usr/include"
unset PKG_CONFIG_SYSROOT_DIR PKG_CONFIG_LIBDIR  # 清理交叉编译残留

cargo build --release
cargo test --all-targets     # 25 passed / 1 ignored
```

要点：
- `libspa-sys` 需要 clang builtin 头；`LIBCLANG_PATH` 指向 libclang.so 目录；
- 若曾配置过交叉 sysroot，需 `unset` 相关变量避免 regen 失败；
- crates 镜像：`rsproxy.cn`（`[source.crates-io] replace-with`）。

## 2. Python wheel 构建（PyO3 + maturin）

```bash
cd python
pip install maturin patchelf    # patchelf 打包 libpipewire 进 wheel 必需
maturin build --release         # → dracopho_capture_core-0.1.0-cp38-abi3-*.whl
```

要点：
- `abi3-py38`：单 wheel 覆盖 Python 3.8+；
- wheel 内嵌 `libpipewire-0.3.so.0`（maturin patchelf 自动处理）；
- 发布：`maturin publish`（需 PyPI token）。

## 3. KDE 回归验证（无头）

```bash
# 无头验证环境（Xvfb + KWin X11 + llvmpipe EGL）
Xvfb :99 -screen 0 1920x1080x24 -ac +extension GLX +render -noreset &
export DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=<dbus>
export KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1 LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless
kwin_x11 --replace &

# 伪造 KDE Wayland 会话变量，跑完整回归
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=6
scripts/kde_regression.sh --no-build --force-kde
```

要点：
- `KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1`：KWin 官方测试开关，跳过截图授权；
- `LIBGL_ALWAYS_SOFTWARE=1` + `EGL_PLATFORM=surfaceless`：让 KWin X11 后端用
  llvmpipe EGL（否则回退 `KWin::VirtualBackend`，取帧返回 "Screenshot got
  cancelled"，回归脚本会检测并降级 SKIP）；
- `--force-kde`：跳过会话检查（无头环境）。

## 4. NVIDIA GPU 验证

```bash
# 真实 GPU 像素通路
scripts/nvidia_egl_render_check.sh   # PASS: 255,0,0 纯红像素

# 【推荐】完整 Xorg + NVIDIA GLX server（屏幕级也走真 GPU）
scripts/gpu_glx_xorg_setup.sh        # 自动部署 Xorg + NVIDIA xorg 模块，X :8
export DISPLAY=:8 KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1
kwin_x11 --replace &
scripts/kde_regression.sh --no-build --force-kde   # PASS=11 全绿
```

关键：**无需 `modeset=Y` / 无需内核模块操作**——装完整 Xorg（有模块加载器）
并加载 NVIDIA 用户态 xorg 模块（`nvidia_drv.so` + `libglxserver_nvidia.so`，
版本与内核驱动一致）即可建立 NV-GLX server 扩展，KWin 屏幕级合成
（CaptureArea/CaptureWorkspace）也走真实 GPU（实测 Tesla T4 renderer）。

## 5. 集成指南（宿主应用，如 mark-shot）

1. **常驻进程持有会话**：库内静态复用 PipeWire 会话；宿主首次带
   `allow_interactive_portal=true` 完成授权，此后同进程零弹窗。
2. **授权持久化**：token 存 `~/.config/dracopho-capture-core/screencast-token`
   （0600），跨重启；新进程静默恢复。
3. **录制前预检**：调用 `auth::verify_saved_token()` 提前暴露"需重新授权"。
4. **窗口内容**：X11 走 XComposite；KDE 走 ScreenShot2 `CaptureWindow`
   （UUID，遮挡/最小化真实内容）；GNOME 回退全屏帧 + 窗口裁剪
   （`object_capture` 如实标注）。
5. **滚动/录制**：用流式接口（`start_stream`/`next_frame`），`target_fps`
   限帧、`minimum_frame_time_ms` 过滤陈旧帧。
6. **多屏**：`capture_outputs` 返回每屏一张（`output_name` 标识，不拼接）；
   跨屏幕区域用 `capture_frame(source_geometry)` 单张组合。

## 6. 发布检查清单

- [ ] `cargo test --all-targets` 全绿（25/1）
- [ ] `python/tests/test_api.py` 全绿（9/9）
- [ ] 完整 KDE 回归（`kde_regression.sh`）PASS、FAIL=0
- [ ] wheel 构建成功（abi3-py38）+ 服务器实机冒烟
- [ ] 提交前扫描：无密钥、无意外文件、无版本号残留
- [ ] push 前 diff 复核

## 7. 已知边界（诚实记录）

| 边界 | 说明 |
| --- | --- |
| KWin GPU 合成 | 已解决：完整 Xorg + NVIDIA xorg 模块（`gpu_glx_xorg_setup.sh`），CaptureArea 也走真 GPU |
| CaptureWindow by-UUID 像素 | Tesla T4 + NVIDIA GLX 下实测真实出图；回归 PASS=11 全绿 |
| Wayland 整屏组合 | portal 无虚拟桌面整流，`all_outputs=true` 报错引导 `capture_outputs` |
| GNOME 窗口对象 | 无 XComposite，回退区域裁剪（`object_capture=false` 如实标注） |
