//! DMA-BUF 帧的 EGL 导入实现。
//!
//! GNOME/KDE 合成器的 ScreenCast 流通常提供 DMA-BUF（GPU 显存）帧，CPU 无法
//! 直接 mmap 读取，必须通过 EGL 导入（EGL_LINUX_DMA_BUF_EXT + GL 纹理 +
//! glReadPixels）转成 CPU 可读的 RGBA 像素。
//!
//! 实现要点：
//! - 通过 dlopen 动态加载 libEGL/libGL，避免编译期硬链接；缺 EGL 时返回 None，
//!   调用方优雅降级（共享内存路径不受影响）。
//! - 读回 GL_RGBA 即为标准 RGBA，无需 C++ 版的红蓝交换 shader（C++ 要输出
//!   QImage ARGB32 小端才交换）。
//! - GL 纹理读回行序自底向上，读回后垂直翻转。
//!
//! 本模块只导入像素，不调用任何"系统自带截图"服务。

use std::ffi::{c_char, c_void, CStr, CString};

use pipewire::spa;

use crate::capture_types::DRM_FORMAT_MOD_INVALID;

/// EGL 常量（EGL 规范稳定值，跨实现不变）。
const EGL_OPENGL_API: u32 = 0x30A2;
const EGL_OPENGL_BIT: i32 = 0x0008;
const EGL_PBUFFER_BIT: i32 = 0x0001;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_CONTEXT_MAJOR_VERSION: i32 = 0x3098;
const EGL_NONE: i32 = 0x3038;
const EGL_EXTENSIONS: i32 = 0x3055;

const EGL_LINUX_DMA_BUF_EXT: u32 = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: i32 = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: i32 = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: i32 = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: i32 = 0x3274;
const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: i32 = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: i32 = 0x3444;

/// GL 常量。
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_LINEAR: i32 = 0x2601;
const GL_CLAMP_TO_EDGE: i32 = 0x812F;
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_NO_ERROR: u32 = 0;

type EGLDisplay = *mut c_void;
type EGLContext = *mut c_void;
type EGLConfig = *mut c_void;
type EGLSurface = *mut c_void;
type EGLImage = *mut c_void;
type EGLBoolean = i32;
type EGLint = i32;
type GLuint = u32;

/// libc dlopen/dlsym 精简封装。
struct Dl {
    handle: *mut c_void,
}

impl Dl {
    fn open(names: &[&str]) -> Option<Dl> {
        for name in names {
            let cname = CString::new(*name).ok()?;
            // RTLD_LAZY=1, RTLD_GLOBAL=0x100
            let handle = unsafe { libc::dlopen(cname.as_ptr(), 1 | 0x100) };
            if !handle.is_null() {
                return Some(Dl { handle });
            }
        }
        None
    }

    fn sym(&self, name: &str) -> Option<*mut c_void> {
        let cname = CString::new(name).ok()?;
        let ptr = unsafe { libc::dlsym(self.handle, cname.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(ptr)
        }
    }
}

impl Drop for Dl {
    fn drop(&mut self) {
        unsafe {
            libc::dlclose(self.handle);
        }
    }
}

/// unsafe 函数指针包装：从裸指针转成带签名的函数。
macro_rules! fptr {
    ($ptr:expr, $ty:ty) => {
        unsafe { std::mem::transmute::<*mut c_void, $ty>($ptr) }
    };
}

/// 单帧 DMA-BUF 导入结果。
pub struct ImportedFrame {
    pub width: u32,
    pub height: u32,
    pub image: image::RgbaImage,
}

/// 支持的 SPA raw 格式 → DRM fourcc 映射（与 C++ gl_helpers 一致）。
pub fn fourcc_for_format(format: spa::param::video::VideoFormat) -> Option<u32> {
    use spa::param::video::VideoFormat as V;
    let c = match format {
        V::BGRA => 0x3432_5241u32, // DRM_FORMAT_ARGB8888
        V::BGRx => 0x3432_5258,    // DRM_FORMAT_XRGB8888
        V::RGBA => 0x3432_4241,    // DRM_FORMAT_ABGR8888
        V::RGBx => 0x3432_4258,    // DRM_FORMAT_XBGR8888
        V::ARGB => 0x3432_4742,    // DRM_FORMAT_BGRA8888
        V::ABGR => 0x3432_4752,    // DRM_FORMAT_RGBA8888
        V::xRGB => 0x5852_4742,    // DRM_FORMAT_BGRX8888
        V::xBGR => 0x5842_4752,    // DRM_FORMAT_RGBX8888
        V::RGB => 0x3834_3252,     // DRM_FORMAT_RGB888
        V::BGR => 0x3834_3242,     // DRM_FORMAT_BGR888
        _ => return None,
    };
    Some(c)
}

/// EGL + GL 函数集。
#[repr(C)]
pub struct GlContext {
    // EGL 核心
    egl_display: EGLDisplay,
    egl_context: EGLContext,
    egl_get_display: unsafe extern "C" fn(*mut c_void) -> EGLDisplay,
    egl_initialize:
        unsafe extern "C" fn(EGLDisplay, *mut EGLint, *mut EGLint) -> EGLBoolean,
    egl_bind_api: unsafe extern "C" fn(u32) -> EGLBoolean,
    egl_choose_config:
        unsafe extern "C" fn(EGLDisplay, *const EGLint, *mut EGLConfig, EGLint, *mut EGLint) -> EGLBoolean,
    egl_create_context:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, EGLContext, *const EGLint) -> EGLContext,
    egl_make_current:
        unsafe extern "C" fn(EGLDisplay, EGLSurface, EGLSurface, EGLContext) -> EGLBoolean,
    egl_terminate: unsafe extern "C" fn(EGLDisplay) -> EGLBoolean,
    egl_get_error: unsafe extern "C" fn() -> EGLint,
    egl_query_string: unsafe extern "C" fn(EGLDisplay, EGLint) -> *const c_char,
    // EGL 扩展
    egl_create_image_khr:
        unsafe extern "C" fn(EGLDisplay, EGLContext, u32, *mut c_void, *const EGLint) -> EGLImage,
    egl_destroy_image_khr: unsafe extern "C" fn(EGLDisplay, EGLImage) -> EGLBoolean,
    // GL
    gl_gen_textures: unsafe extern "C" fn(GLsizei, *mut GLuint),
    gl_bind_texture: unsafe extern "C" fn(u32, GLuint),
    gl_tex_parameteri: unsafe extern "C" fn(u32, u32, i32),
    gl_egl_image_target_texture_2d: unsafe extern "C" fn(u32, EGLImage),
    gl_gen_framebuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    gl_bind_framebuffer: unsafe extern "C" fn(u32, GLuint),
    gl_framebuffer_texture_2d: unsafe extern "C" fn(u32, u32, u32, GLuint, i32),
    gl_check_framebuffer_status: unsafe extern "C" fn(u32) -> u32,
    gl_read_pixels:
        unsafe extern "C" fn(i32, i32, GLsizei, GLsizei, u32, u32, *mut c_void),
    gl_delete_textures: unsafe extern "C" fn(GLsizei, *const GLuint),
    gl_delete_framebuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    gl_get_error: unsafe extern "C" fn() -> u32,
    _handles: (Dl, Dl), // 保持库加载
}

type GLsizei = i32;

impl GlContext {
    /// 尝试初始化 EGL（surfaceless）+ GL。任何一步失败返回 None。
    fn new() -> Option<GlContext> {
        let egl = Dl::open(&["libEGL.so.1", "libEGL.so"])?;
        let gl = Dl::open(&["libGL.so.1", "libGL.so"])?;

        let egl_get_display = fptr!(egl.sym("eglGetDisplay")?, unsafe extern "C" fn(*mut c_void) -> EGLDisplay);
        let egl_initialize = fptr!(egl.sym("eglInitialize")?, unsafe extern "C" fn(EGLDisplay, *mut EGLint, *mut EGLint) -> EGLBoolean);
        let egl_bind_api = fptr!(egl.sym("eglBindAPI")?, unsafe extern "C" fn(u32) -> EGLBoolean);
        let egl_choose_config = fptr!(egl.sym("eglChooseConfig")?, unsafe extern "C" fn(EGLDisplay, *const EGLint, *mut EGLConfig, EGLint, *mut EGLint) -> EGLBoolean);
        let egl_create_context = fptr!(egl.sym("eglCreateContext")?, unsafe extern "C" fn(EGLDisplay, EGLConfig, EGLContext, *const EGLint) -> EGLContext);
        let egl_make_current = fptr!(egl.sym("eglMakeCurrent")?, unsafe extern "C" fn(EGLDisplay, EGLSurface, EGLSurface, EGLContext) -> EGLBoolean);
        let egl_terminate = fptr!(egl.sym("eglTerminate")?, unsafe extern "C" fn(EGLDisplay) -> EGLBoolean);
        let egl_get_error = fptr!(egl.sym("eglGetError")?, unsafe extern "C" fn() -> EGLint);
        let egl_query_string = fptr!(egl.sym("eglQueryString")?, unsafe extern "C" fn(EGLDisplay, EGLint) -> *const c_char);
        let egl_get_proc_address = fptr!(egl.sym("eglGetProcAddress")?, unsafe extern "C" fn(*const c_char) -> *mut c_void);

        // 扩展函数
        let get_proc = |name: &str| -> Option<*mut c_void> {
            let c = CString::new(name).ok()?;
            let p = unsafe { egl_get_proc_address(c.as_ptr()) };
            if p.is_null() {
                None
            } else {
                Some(p)
            }
        };
        let egl_create_image_khr = fptr!(get_proc("eglCreateImageKHR")?, unsafe extern "C" fn(EGLDisplay, EGLContext, u32, *mut c_void, *const EGLint) -> EGLImage);
        let egl_destroy_image_khr = fptr!(get_proc("eglDestroyImageKHR")?, unsafe extern "C" fn(EGLDisplay, EGLImage) -> EGLBoolean);

        // GL 核心函数
        let gl_gen_textures = fptr!(gl.sym("glGenTextures")?, unsafe extern "C" fn(GLsizei, *mut GLuint));
        let gl_bind_texture = fptr!(gl.sym("glBindTexture")?, unsafe extern "C" fn(u32, GLuint));
        let gl_tex_parameteri = fptr!(gl.sym("glTexParameteri")?, unsafe extern "C" fn(u32, u32, i32));
        let gl_gen_framebuffers = fptr!(gl.sym("glGenFramebuffers")?, unsafe extern "C" fn(GLsizei, *mut GLuint));
        let gl_bind_framebuffer = fptr!(gl.sym("glBindFramebuffer")?, unsafe extern "C" fn(u32, GLuint));
        let gl_framebuffer_texture_2d = fptr!(gl.sym("glFramebufferTexture2D")?, unsafe extern "C" fn(u32, u32, u32, GLuint, i32));
        let gl_check_framebuffer_status = fptr!(gl.sym("glCheckFramebufferStatus")?, unsafe extern "C" fn(u32) -> u32);
        let gl_read_pixels = fptr!(gl.sym("glReadPixels")?, unsafe extern "C" fn(i32, i32, GLsizei, GLsizei, u32, u32, *mut c_void));
        let gl_delete_textures = fptr!(gl.sym("glDeleteTextures")?, unsafe extern "C" fn(GLsizei, *const GLuint));
        let gl_delete_framebuffers = fptr!(gl.sym("glDeleteFramebuffers")?, unsafe extern "C" fn(GLsizei, *const GLuint));
        let gl_get_error = fptr!(gl.sym("glGetError")?, unsafe extern "C" fn() -> u32);

        // GL_OES_EGL_image：优先 eglGetProcAddress，回退 libGL
        let gl_egl_image_target_texture_2d = get_proc("glEGLImageTargetTexture2DOES")
            .or_else(|| gl.sym("glEGLImageTargetTexture2DOES"))
            .map(|p| unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32, EGLImage)>(p) })?;

        // 初始化 display
        unsafe {
            let dpy = egl_get_display(0 as *mut c_void); // EGL_DEFAULT_DISPLAY
            if dpy.is_null() {
                return None;
            }
            let mut major = 0;
            let mut minor = 0;
            if egl_initialize(dpy, &mut major, &mut minor) == 0 {
                return None;
            }
            if egl_bind_api(EGL_OPENGL_API) == 0 {
                egl_terminate(dpy);
                return None;
            }

            // 检查扩展
            let extensions_ptr = egl_query_string(dpy, EGL_EXTENSIONS);
            let extensions = if extensions_ptr.is_null() {
                String::new()
            } else {
                CStr::from_ptr(extensions_ptr).to_string_lossy().into_owned()
            };
            if !extensions.contains("EGL_EXT_image_dma_buf_import") {
                egl_terminate(dpy);
                return None;
            }
            let surfaceless = extensions.contains("EGL_KHR_surfaceless_context");

            // choose config（surfaceless 下仍需要一个 config 建 context）
            let mut config = 0 as EGLConfig;
            let mut num_configs = 0;
            let config_attrs = [
                EGL_SURFACE_TYPE,
                EGL_PBUFFER_BIT,
                EGL_RENDERABLE_TYPE,
                EGL_OPENGL_BIT,
                EGL_NONE,
            ];
            if egl_choose_config(dpy, config_attrs.as_ptr(), &mut config, 1, &mut num_configs) == 0
                || num_configs == 0
            {
                egl_terminate(dpy);
                return None;
            }

            // 创建 context（major 3，回退 2）
            let mut ctx = 0 as EGLContext;
            for version in [3i32, 2] {
                let ctx_attrs = [EGL_CONTEXT_MAJOR_VERSION, version, EGL_NONE];
                let candidate = egl_create_context(dpy, config, 0 as EGLContext, ctx_attrs.as_ptr());
                if !candidate.is_null() {
                    ctx = candidate;
                    break;
                }
            }
            if ctx.is_null() {
                egl_terminate(dpy);
                return None;
            }
            // surfaceless 需要 EGL_KHR_surfaceless_context；否则不兼容环境
            if !surfaceless {
                egl_terminate(dpy);
                return None;
            }
            if egl_make_current(dpy, 0 as EGLSurface, 0 as EGLSurface, ctx) == 0 {
                egl_terminate(dpy);
                return None;
            }

            Some(GlContext {
                egl_display: dpy,
                egl_context: ctx,
                egl_get_display,
                egl_initialize,
                egl_bind_api,
                egl_choose_config,
                egl_create_context,
                egl_make_current,
                egl_terminate,
                egl_get_error,
                egl_query_string,
                egl_create_image_khr,
                egl_destroy_image_khr,
                gl_gen_textures,
                gl_bind_texture,
                gl_tex_parameteri,
                gl_egl_image_target_texture_2d,
                gl_gen_framebuffers,
                gl_bind_framebuffer,
                gl_framebuffer_texture_2d,
                gl_check_framebuffer_status,
                gl_read_pixels,
                gl_delete_textures,
                gl_delete_framebuffers,
                gl_get_error,
                _handles: (egl, gl),
            })
        }
    }

    /// 导入一个 DMA-BUF 帧并读回 RGBA。
    ///
    /// - `fd` / `offset` / `stride`：plane0 的 DMA-BUF 描述。
    /// - `fourcc`：DRM fourcc。
    /// - `modifier`：Some(mod) 时携带 modifier 属性；None 时不带。
    pub fn import(
        &self,
        fd: i32,
        offset: i64,
        stride: i32,
        width: u32,
        height: u32,
        fourcc: u32,
        modifier: Option<u64>,
    ) -> Option<ImportedFrame> {
        let mut attributes: Vec<EGLint> = vec![
            EGL_LINUX_DRM_FOURCC_EXT,
            fourcc as EGLint,
            EGL_DMA_BUF_PLANE0_FD_EXT,
            fd,
            EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            offset as EGLint,
            EGL_DMA_BUF_PLANE0_PITCH_EXT,
            stride,
        ];
        if let Some(modifier) = modifier {
            if modifier != DRM_FORMAT_MOD_INVALID {
                attributes.push(EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT);
                attributes.push((modifier & 0xffff_ffff) as EGLint);
                attributes.push(EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT);
                attributes.push((modifier >> 32) as EGLint);
            }
        }
        attributes.push(EGL_NONE);

        let mut texture = 0u32;
        let mut fbo = 0u32;

        unsafe {
            let image = (self.egl_create_image_khr)(
                self.egl_display,
                0 as EGLContext,
                EGL_LINUX_DMA_BUF_EXT,
                0 as *mut c_void,
                attributes.as_ptr(),
            );
            if image.is_null() {
                return None;
            }

            (self.gl_gen_textures)(1, &mut texture);
            (self.gl_bind_texture)(GL_TEXTURE_2D, texture);
            (self.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            (self.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            (self.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            (self.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            (self.gl_egl_image_target_texture_2d)(GL_TEXTURE_2D, image);

            // 附着到 FBO 并读回
            (self.gl_gen_framebuffers)(1, &mut fbo);
            (self.gl_bind_framebuffer)(GL_FRAMEBUFFER, fbo);
            (self.gl_framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                texture,
                0,
            );
            if (self.gl_check_framebuffer_status)(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
                (self.gl_delete_textures)(1, &texture);
                (self.gl_delete_framebuffers)(1, &fbo);
                (self.egl_destroy_image_khr)(self.egl_display, image);
                return None;
            }

            let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
            (self.gl_read_pixels)(
                0,
                0,
                width as GLsizei,
                height as GLsizei,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                buf.as_mut_ptr() as *mut c_void,
            );
            let gl_err = (self.gl_get_error)();
            (self.gl_delete_textures)(1, &texture);
            (self.gl_delete_framebuffers)(1, &fbo);
            (self.egl_destroy_image_khr)(self.egl_display, image);

            if gl_err != GL_NO_ERROR {
                return None;
            }

            // GL 纹理读回行序自底向上：垂直翻转。
            let mut image = image::RgbaImage::new(width, height);
            let row_bytes = (width as usize) * 4;
            let pixels = image.as_mut();
            for y in 0..height as usize {
                let src_start = y * row_bytes;
                let dst_start = (height as usize - 1 - y) * row_bytes;
                pixels[dst_start..dst_start + row_bytes].copy_from_slice(&buf[src_start..src_start + row_bytes]);
            }
            Some(ImportedFrame {
                width,
                height,
                image,
            })
        }
    }
}

// EGL display/context 的生命周期由 dlopen 句柄持有保证；本模块仅在 PipeWire
// 流线程（同一线程）内调用，跨线程共享只用于惰性初始化，故 unsafe 标记。
unsafe impl Send for GlContext {}
unsafe impl Sync for GlContext {}

impl Drop for GlContext {
    fn drop(&mut self) {
        unsafe {
            (self.egl_make_current)(self.egl_display, 0 as EGLSurface, 0 as EGLSurface, 0 as EGLContext);
            (self.egl_terminate)(self.egl_display);
        }
    }
}

/// 惰性初始化并返回全局 EGL 导入器（进程内复用，跨线程安全）。
pub fn global_importer() -> Option<&'static GlContext> {
    static ONCE: std::sync::OnceLock<Option<GlContext>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| GlContext::new()).as_ref()
}

/// 便捷入口：把 PipeWire DMA-BUF data 转成 RGBA。
///
/// `modifier` 为 `Some(mod)` 且 `mod != DRM_FORMAT_MOD_INVALID` 时才携带
/// modifier 属性；`None` 表示无 modifier（必须由调用方依据 SPA_VIDEO_FLAG_MODIFIER
/// 标志判断后传入，绝不能把"无 modifier"的 0 值当作有效 modifier）。
pub fn import_dmabuf(
    fd: i32,
    offset: i64,
    stride: i32,
    width: u32,
    height: u32,
    format: spa::param::video::VideoFormat,
    modifier: Option<u64>,
) -> Option<ImportedFrame> {
    let fourcc = fourcc_for_format(format)?;
    let importer = global_importer()?;
    let has_modifier = modifier.is_some_and(|m| m != DRM_FORMAT_MOD_INVALID);
    importer.import(
        fd,
        offset,
        stride,
        width,
        height,
        fourcc,
        has_modifier.then_some(modifier.unwrap()),
    )
}

#[cfg(test)]
mod tests {
    use super::fourcc_for_format;
    use pipewire::spa::param::video::VideoFormat as V;

    #[test]
    fn maps_spa_formats_to_drm_fourcc() {
        assert_eq!(fourcc_for_format(V::BGRA), Some(0x3432_5241)); // ARGB8888
        assert_eq!(fourcc_for_format(V::BGRx), Some(0x3432_5258)); // XRGB8888
        assert_eq!(fourcc_for_format(V::xRGB), Some(0x5852_4742)); // BGRX8888
        assert_eq!(fourcc_for_format(V::ARGB), Some(0x3432_4742)); // BGRA8888
        assert_eq!(fourcc_for_format(V::RGB), Some(0x3834_3252));  // RGB888
    }

    #[test]
    fn rejects_unsupported_format() {
        assert_eq!(fourcc_for_format(V::NV12), None);
    }
}
