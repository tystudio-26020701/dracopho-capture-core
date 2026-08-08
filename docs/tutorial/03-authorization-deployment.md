# 03 — 授权与无头部署

> 面向部署工程师：把截图能力部署到无头机器（服务器/CI/常驻守护）前，
> 需要理解 ScreenCast 授权模型与预检机制，避免"部署后一截图就弹窗或卡死"。

## 1. 授权模型：一次授权，永久静默

Wayland 合成器（GNOME/KDE）只通过 ScreenCast 暴露像素，**首次必须用户确认
一次**。本库把这次确认的收益最大化：

```
首次（交互会话）                        之后（任何进程/重启）
─────────────                          ─────────────────────
allow_interactive_portal=true           ─ 读持久化 token
    ↓                                      ↓
弹出 ScreenCast 选择器 ──用户同意──▶  portal 存权限（跨重启）
    ↓                                      ↓
Start 返回 restore_token              Start 用 token 静默恢复
    ↓                                      ↓
保存到 ~/.config/dracopho-capture-core/
      screencast-token（0600）           零弹窗
```

- 授权对**应用身份（app_id）**绑定——集成方以 `.desktop` 方式启动（systemd
  app- 单元）时，portal 才能正确解析并持久化权限。
- token 跨进程、跨重启有效，直到 portal 权限被撤销。

## 2. 部署前必须做的事

### 2.1 首次授权（在带桌面的会话执行一次）

```bash
# CLI 方式（推荐，交互授权并持久化 token）
dracopho-capture --authorize

# 或库方式：应用首次带 allow_interactive_portal=true 截图一次
```

### 2.2 验证授权对应用身份有效

```bash
# 从目标启动方式（systemd 单元 / 桌面文件）启动后：
dracopho-capture --list-routing
# 确保会话分类正确（wayland-gnome / wayland-kde）
```

## 3. 无头部署清单

| 检查项 | 命令/方法 | 通过标准 |
| --- | --- | --- |
| 会话类型 | `echo $XDG_SESSION_TYPE` | wayland |
| 应用身份 | 从 `.desktop`/systemd app- 单元启动 | portal 能解析 app_id |
| token 存在 | `dracopho-capture --authorize` 后检查 `~/.config/dracopho-capture-core/screencast-token` | 文件存在且 0600 |
| **token 有效（预检）** | `auth::verify_saved_token()`（Rust）或 `verify_saved_token()`（Python） | 返回 true |
| 无头不弹窗 | 用默认 Auto 路由（整屏走 portal，绝不触发选择器） | 截图直接成功或明确报错 |

## 4. 预检机制（防弹窗的关键）

无头模式下，库在调用 portal `Start` 前**先静默校验 token**：

```
无头 capture_frame
    ↓
读取持久化 token ──无──▶ 报错"requires interactive authorization"
    ↓ 有
查询 portal 权限存储（PermissionStore，表 screencast）
    ├── token 不存在        → 报错"token no longer valid; re-run --authorize"
    ├── 权限未授予 app_id   → 同上（不弹窗）
    ├── restore 数据为空    → 同上
    └── GNOME：引用显示器已拔 → 同上
    ↓ 全部通过
调用 Start（token 静默恢复，毫秒级）
```

- `verify_saved_token()` 暴露给调用方：**录制/批量任务启动前主动调用**，
  提前暴露"需要重新授权"，避免中途弹窗或卡死。
- 预检本身失败（DBus/解析异常，无法确认）→ 退化为带 10s 防线的一次 Start
  尝试，宁可快速失败也不误卡正常部署。

## 5. 常见部署场景

### 场景 A：GNOME 无头录制

```bash
# 首次（带桌面）：dracopho-capture --authorize
# 之后（无头，同一应用身份）：
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=ubuntu:GNOME
dracopho-capture --capture-to out.png          # 静默截图
```

### 场景 B：KDE 窗口级（ScreenShot2）

```bash
# 显式启用 KWin ScreenShot2（设计上跳过 portal 授权门，用于明确放行的调用）
dracopho-capture --capture-to out.png --backend kwin-screenshot2 --region 0,0,800,600
# 或库：route = RouteMode::Only(Backend::KwinScreenShot2)
```

### 场景 C：无头验证回归

```bash
scripts/kde_regression.sh            # KDE Plasma Wayland 上完整验证
```

## 6. 故障排查

| 现象 | 定位 | 解决 |
| --- | --- | --- |
| `verify_saved_token()` 返回 false | token 失效/权限撤销/显示器变动 | 重新交互授权 |
| 截图报"requires interactive authorization" | 无 token 或 app_id 解析失败 | 确认从 `.desktop` 启动 + 重新授权 |
| KDE 下 CaptureArea 报 `Error.Cancelled` | KWin 无 EGL 合成（虚拟后端） | 用完整 Xorg + NVIDIA GLX（见工程报告 04 §7）或 llvmpipe 稳定路径 |
