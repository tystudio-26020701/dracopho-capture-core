#!/usr/bin/env bash
# NVIDIA GPU 渲染冒烟验证（真实 GPU 像素通路）。
#
# 在带 NVIDIA 显卡的机器上运行：
#   scripts/nvidia_egl_render_check.sh
#
# 通过离屏 EGL 渲染纯红像素并读回，验证：
#   - NVIDIA EGL 驱动可用（vendor=NVIDIA）
#   - GPU 像素通路真实工作（读回 255,0,0,255）
#
# 构建依赖：libegl-dev libgles-dev（或等效）。

set -uo pipefail

SRC="${TMPDIR:-/tmp}/nv_egl_render.c"
cat > "$SRC" <<'EOF'
#include <stdio.h>
#include <EGL/egl.h>
#include <GLES2/gl2.h>

int main(void) {
    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (dpy == EGL_NO_DISPLAY) { printf("FAIL: no display\n"); return 1; }
    EGLint major = 0, minor = 0;
    if (eglInitialize(dpy, &major, &minor) != EGL_TRUE) { printf("FAIL: init\n"); return 1; }
    printf("EGL %d.%d vendor=%s\n", major, minor, eglQueryString(dpy, EGL_VENDOR));

    const EGLint cfg_attrs[] = {
        EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
        EGL_NONE
    };
    EGLConfig cfg;
    EGLint ncfg = 0;
    if (eglChooseConfig(dpy, cfg_attrs, &cfg, 1, &ncfg) != EGL_TRUE || ncfg < 1) {
        printf("FAIL: chooseConfig\n"); return 1;
    }
    const EGLint pbuf_attrs[] = { EGL_WIDTH, 64, EGL_HEIGHT, 64, EGL_NONE };
    EGLSurface surf = eglCreatePbufferSurface(dpy, cfg, pbuf_attrs);
    if (surf == EGL_NO_SURFACE) { printf("FAIL: pbuffer\n"); return 1; }
    const EGLint ctx_attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLContext ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, ctx_attrs);
    if (ctx == EGL_NO_CONTEXT) { printf("FAIL: context\n"); return 1; }
    eglMakeCurrent(dpy, surf, surf, ctx);

    glClearColor(1.0f, 0.0f, 0.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    eglSwapBuffers(dpy, surf);

    unsigned char px[4] = {0};
    glReadPixels(32, 32, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, px);
    printf("center pixel RGBA = %d,%d,%d,%d\n", px[0], px[1], px[2], px[3]);
    eglTerminate(dpy);
    if (px[0] > 200 && px[1] < 50 && px[2] < 50) {
        printf("RESULT: PASS - NVIDIA GPU rendered a real red pixel\n");
        return 0;
    }
    printf("RESULT: FAIL - pixel unexpected\n");
    return 1;
}
EOF

BIN="${TMPDIR:-/tmp}/nv_egl_render"
if ! gcc -o "$BIN" "$SRC" -lEGL -lGLESv2 2>/tmp/nv_egl_build.log; then
    echo "编译失败（缺 libEGL/libGLESv2 头文件或库）:"
    head -5 /tmp/nv_egl_build.log
    exit 1
fi
LIBGL_ALWAYS_SOFTWARE=0 "$BIN"
