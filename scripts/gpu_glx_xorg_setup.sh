#!/usr/bin/env bash
# 在 NVIDIA GPU 容器（autodl 等）上配置 Xorg + NVIDIA GLX server，
# 使 KWin 的屏幕级合成（CaptureArea/CaptureWorkspace）也走真实 GPU。
#
# 原理：无需内核模块操作（不依赖 CAP_SYS_MODULE），只需
#   1. 完整 Xorg（有模块加载器）
#   2. NVIDIA 用户态 xorg 模块：nvidia_drv.so + libglxserver_nvidia.so
#      （版本必须与内核驱动一致，从 NVIDIA apt 仓库下载 deb 解包）
#   3. xorg.conf 指向 NVIDIA 驱动（AllowEmptyInitialConfiguration）
#
# 用法：scripts/gpu_glx_xorg_setup.sh [DISPLAY_NUM]
#   DISPLAY_NUM 默认 8（:8）。
#
# 之后：
#   DISPLAY=:8 KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1 kwin_x11 --replace &
#   scripts/kde_regression.sh --no-build --force-kde

set -uo pipefail

DISP="${1:-8}"
DRV_VER="580.65.06"          # 与宿主内核驱动版本一致（nvidia-smi 查询）
REPO="https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2404/x86_64"
XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/runtime-root}"

export DEBIAN_FRONTEND=noninteractive

echo "=== 1. 装完整 Xorg（模块加载器）==="
apt-get install -y --no-install-recommends xserver-xorg-core || exit 1

echo "=== 2. 下载并部署 NVIDIA 用户态 xorg 模块（版本 $DRV_VER）==="
mkdir -p /tmp/nvglx && cd /tmp/nvglx

# libglxserver_nvidia.so（GLX server 扩展）
GL_DEB="libnvidia-gl-580_${DRV_VER}-0ubuntu1_amd64.deb"
if [ ! -f "$GL_DEB" ]; then
    curl -sL -o "$GL_DEB" "$REPO/$GL_DEB" || { echo "下载失败: $GL_DEB"; exit 1; }
fi
dpkg-deb -x "$GL_DEB" gl 2>/dev/null
mkdir -p /usr/lib/xorg/modules/extensions
cp gl/usr/lib/x86_64-linux-gnu/nvidia/xorg/libglxserver_nvidia.so.${DRV_VER} \
      /usr/lib/xorg/modules/extensions/libglx.so || { echo "libglxserver 提取失败"; exit 1; }
cp gl/usr/lib/x86_64-linux-gnu/nvidia/xorg/libglxserver_nvidia.so.${DRV_VER} \
      /usr/lib/x86_64-linux-gnu/nvidia/xorg/ 2>/dev/null

# nvidia_drv.so（X video driver）
XV_DEB="xserver-xorg-video-nvidia-580_${DRV_VER}-0ubuntu1_amd64.deb"
if [ ! -f "$XV_DEB" ]; then
    curl -sL -o "$XV_DEB" "$REPO/$XV_DEB" || { echo "下载失败: $XV_DEB"; exit 1; }
fi
dpkg-deb -x "$XV_DEB" xv 2>/dev/null
mkdir -p /usr/lib/xorg/modules/drivers
cp xv/usr/lib/x86_64-linux-gnu/nvidia/xorg/nvidia_drv.so \
      /usr/lib/xorg/modules/drivers/ || { echo "nvidia_drv 提取失败"; exit 1; }

echo "=== 3. 写 xorg.conf ==="
mkdir -p /etc/X11
cat > /etc/X11/xorg.conf <<EOF
Section "ServerFlags"
    Option "AllowEmptyInitialConfiguration" "true"
EndSection
Section "Files"
    ModulePath "/usr/lib/x86_64-linux-gnu/nvidia/xorg,/usr/lib/xorg/modules"
EndSection
Section "Device"
    Identifier "nvidia"
    Driver "nvidia"
EndSection
Section "Screen"
    Identifier "Screen0"
    Device "nvidia"
EndSection
EOF

echo "=== 4. 启动 Xorg :$DISP ==="
mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
pkill Xorg 2>/dev/null
sleep 1
nohup Xorg ":$DISP" -config /etc/X11/xorg.conf -noreset \
    > /tmp/xorg-gpu.log 2>&1 &
sleep 5

echo "=== 5. 验证 ==="
ls /tmp/.X11-unix/ | grep "X$DISP" && echo "X socket OK" || { echo "X socket 未建立"; tail -20 /tmp/xorg-gpu.log; exit 1; }
grep -q "Initializing extension NV-GLX" /tmp/xorg-gpu.log \
    && echo "NV-GLX 扩展已建立 ✅" || { echo "NV-GLX 缺失"; tail -20 /tmp/xorg-gpu.log; exit 1; }
DISPLAY=:$DISP timeout 10 glxinfo 2>/dev/null | grep -iE "renderer" | head -1

echo
echo "完成。后续命令："
echo "  export DISPLAY=:$DISP DBUS_SESSION_BUS_ADDRESS=<dbus> XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR"
echo "  KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1 kwin_x11 --replace &"
echo "  scripts/kde_regression.sh --no-build --force-kde"
