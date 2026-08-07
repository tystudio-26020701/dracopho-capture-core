//! dracopho-capture-core Python 绑定（PyO3，abi3-py38）。
//!
//! 每个功能均提供 Python 调用：
//! - 单帧捕获：`capture_frame(request)`（全屏/区域/指定输出/路由）
//! - 多屏幕集合：`capture_outputs(request)`（每屏一张，不拼接）
//! - 窗口捕获：`capture_windows(request)` / `list_windows()`
//! - 流式捕获：`start_stream(request)` → `Stream.next_frame()`
//! - 输出枚举：`list_outputs()`
//! - 后端与路由：`available_backends()` / `detect_routing()`
//! - 授权：`authorized()` / `save_restore_token()` / `verify_saved_token()` …

use std::io::Cursor;

// 依赖 crate 的 lib 名与本 crate 的 pymodule 函数名（模块名）同为
// dracopho_capture_core，会遮蔽依赖引用，故显式别名导入。
extern crate dracopho_capture_core as core_lib;

use core_lib::capture_types::{self, Backend, CaptureRequest, RouteMode};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

// ---------------------------------------------------------------------------
// 后端与路由
// ---------------------------------------------------------------------------

/// 把 Rust `Backend` 转为 Python 可序列化字符串。
fn backend_from_name(name: &str) -> Result<Backend, String> {
    match name.to_lowercase().replace('_', "-").as_str() {
        "pipewire" | "pipewire-screencast" | "screencast" => Ok(Backend::PipeWireScreencast),
        "wlr" | "wlr-screencopy" | "screencopy" => Ok(Backend::WlrScreencopy),
        "x11" => Ok(Backend::X11),
        "kwin" | "kwin-screenshot2" | "kwin-screenshot" => Ok(Backend::KwinScreenShot2),
        "windows-wgc" | "wgc" => Ok(Backend::WindowsWgc),
        "none" => Ok(Backend::None),
        other => Err(format!("unknown backend: {other}")),
    }
}

/// 路由模式：Auto / Only / Order / Prefer。
#[pyclass(name = "RouteMode", module = "dracopho_capture_core", from_py_object)]
#[derive(Clone)]
pub struct PyRouteMode {
    inner: RouteMode,
}

#[pymethods]
impl PyRouteMode {
    /// 自动：按桌面类型智能分发。
    #[staticmethod]
    fn auto() -> Self {
        Self { inner: RouteMode::Auto }
    }

    /// 仅使用指定后端（失败不回退）。
    #[staticmethod]
    fn only(backend: &str) -> PyResult<Self> {
        let b = backend_from_name(backend).map_err(PyValueError::new_err)?;
        Ok(Self { inner: RouteMode::Only(b) })
    }

    /// 按给定优先级依次尝试（显式回退链）。
    #[staticmethod]
    fn order(backends: Vec<String>) -> PyResult<Self> {
        let mut v = Vec::with_capacity(backends.len());
        for b in backends {
            v.push(backend_from_name(&b).map_err(PyValueError::new_err)?);
        }
        if v.is_empty() {
            return Err(PyValueError::new_err("route order must not be empty"));
        }
        Ok(Self { inner: RouteMode::Order(v) })
    }

    /// 优先指定后端，失败后按自动推荐顺序回退。
    #[staticmethod]
    fn prefer(backend: &str) -> PyResult<Self> {
        let b = backend_from_name(backend).map_err(PyValueError::new_err)?;
        Ok(Self { inner: RouteMode::Prefer(b) })
    }

    /// 路由描述（调试用）。
    fn __repr__(&self) -> String {
        format!("RouteMode({:?})", self.inner)
    }
}

/// 窗口匹配选择器：Id / Title / Class / Instance / Index / Pid / Process / Auto。
#[pyclass(name = "WindowMatch", module = "dracopho_capture_core", from_py_object)]
#[derive(Clone)]
pub struct PyWindowMatch {
    inner: core_lib::window::WindowMatch,
}

#[pymethods]
impl PyWindowMatch {
    /// 精确窗口 id（X11 十六进制，如 "0x2a00001"）。
    #[staticmethod]
    fn id(spec: String) -> Self {
        Self { inner: core_lib::window::WindowMatch::Id(spec) }
    }

    /// 标题精确或子串（大小写不敏感）。
    #[staticmethod]
    fn title(spec: String) -> Self {
        Self { inner: core_lib::window::WindowMatch::Title(spec) }
    }

    /// WM_CLASS class / app_id。
    #[staticmethod]
    #[pyo3(name = "by_class")]
    fn class(spec: String) -> Self {
        Self { inner: core_lib::window::WindowMatch::Class(spec) }
    }

    /// WM_CLASS instance / app_name。
    #[staticmethod]
    #[pyo3(name = "by_instance")]
    fn instance(spec: String) -> Self {
        Self { inner: core_lib::window::WindowMatch::Instance(spec) }
    }

    /// 枚举序号。
    #[staticmethod]
    fn index(idx: usize) -> Self {
        Self { inner: core_lib::window::WindowMatch::Index(idx) }
    }

    /// 属主进程 id。
    #[staticmethod]
    fn pid(pid: i64) -> Self {
        Self { inner: core_lib::window::WindowMatch::Pid(pid) }
    }

    /// 进程名（/proc，尽力而为）。
    #[staticmethod]
    fn process(spec: String) -> Self {
        Self { inner: core_lib::window::WindowMatch::Process(spec) }
    }

    /// 自动匹配：id → 精确标题 → class → 序号 → pid → 子串。
    #[staticmethod]
    fn auto(spec: String) -> Self {
        Self { inner: core_lib::window::WindowMatch::Auto(spec) }
    }

    fn __repr__(&self) -> String {
        format!("WindowMatch({:?})", self.inner)
    }
}

/// 从字符串解析窗口选择器（`by` 取 auto/id/title/class/index/pid/process）。
#[pyfunction]
#[pyo3(signature = (spec, by=None))]
fn parse_match(spec: &str, by: Option<&str>) -> PyResult<PyWindowMatch> {
    let inner = core_lib::window::parse_match(spec, by)
        .map_err(PyValueError::new_err)?;
    Ok(PyWindowMatch { inner })
}

// ---------------------------------------------------------------------------
// 捕获请求与结果
// ---------------------------------------------------------------------------

/// 后端无关的捕获请求。
#[pyclass(name = "CaptureRequest", module = "dracopho_capture_core", skip_from_py_object)]
#[derive(Clone)]
pub struct PyCaptureRequest {
    pub inner: CaptureRequest,
}

#[pymethods]
impl PyCaptureRequest {
    #[new]
    #[pyo3(signature = (
        source_geometry=None,
        preferred_output=None,
        all_outputs=false,
        include_cursor=false,
        target_fps=0,
        minimum_frame_time_ms=0,
        allow_interactive_portal=false,
        hide_own_windows=true,
        window_matches=None,
        component=None,
        route=None,
    ))]
    fn new(
        source_geometry: Option<(i32, i32, i32, i32)>,
        preferred_output: Option<String>,
        all_outputs: bool,
        include_cursor: bool,
        target_fps: u32,
        minimum_frame_time_ms: u64,
        allow_interactive_portal: bool,
        hide_own_windows: bool,
        window_matches: Option<Vec<PyWindowMatch>>,
        component: Option<(i32, i32, i32, i32)>,
        route: Option<PyRouteMode>,
    ) -> Self {
        let mut inner = CaptureRequest {
            source_geometry,
            preferred_output,
            all_outputs,
            include_cursor,
            target_fps,
            minimum_frame_time_ms,
            allow_interactive_portal,
            hide_own_windows,
            window_matches: window_matches
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.inner)
                .collect(),
            component,
            ..Default::default()
        };
        if let Some(r) = route {
            inner.route = r.inner;
        }
        Self { inner }
    }

    #[getter]
    fn source_geometry(&self) -> Option<(i32, i32, i32, i32)> {
        self.inner.source_geometry
    }

    #[getter]
    fn preferred_output(&self) -> Option<String> {
        self.inner.preferred_output.clone()
    }

    #[getter]
    fn all_outputs(&self) -> bool {
        self.inner.all_outputs
    }

    #[getter]
    fn include_cursor(&self) -> bool {
        self.inner.include_cursor
    }

    #[getter]
    fn target_fps(&self) -> u32 {
        self.inner.target_fps
    }

    #[getter]
    fn minimum_frame_time_ms(&self) -> u64 {
        self.inner.minimum_frame_time_ms
    }

    #[getter]
    fn allow_interactive_portal(&self) -> bool {
        self.inner.allow_interactive_portal
    }

    #[getter]
    fn hide_own_windows(&self) -> bool {
        self.inner.hide_own_windows
    }

    fn __repr__(&self) -> String {
        format!(
            "CaptureRequest(geo={:?}, output={:?}, all_outputs={}, cursor={}, route={:?})",
            self.inner.source_geometry,
            self.inner.preferred_output,
            self.inner.all_outputs,
            self.inner.include_cursor,
            self.inner.route
        )
    }
}

/// 后端无关的捕获结果。
#[pyclass(name = "CaptureResult", module = "dracopho_capture_core")]
pub struct PyCaptureResult {
    inner: capture_types::CaptureResult,
}

/// 把 RgbaImage 编码为 PNG 字节。
fn image_to_png(img: &image::RgbaImage) -> PyResult<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| PyRuntimeError::new_err(format!("png encode failed: {e}")))?;
    Ok(buf.into_inner())
}

#[pymethods]
impl PyCaptureResult {
    /// 是否捕获成功。
    #[getter]
    fn ok(&self) -> bool {
        self.inner.image.is_some()
    }

    /// 错误信息（失败时）。
    #[getter]
    fn error(&self) -> Option<String> {
        self.inner.error.clone()
    }

    /// 实际命中的后端名。
    #[getter]
    fn backend(&self) -> &'static str {
        self.inner.backend.name()
    }

    /// image 实际表示的全局坐标。
    #[getter]
    fn source_geometry(&self) -> Option<(i32, i32, i32, i32)> {
        self.inner.source_geometry
    }

    /// 实际命中的输出名。
    #[getter]
    fn output_name(&self) -> Option<String> {
        self.inner.output_name.clone()
    }

    /// 帧时间戳（毫秒）。
    #[getter]
    fn frame_time_ms(&self) -> u64 {
        self.inner.frame_time_ms
    }

    /// 图像 PNG 字节（成功时）。
    fn png<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.inner.image.as_ref().and_then(|img| {
            image_to_png(img).ok().map(|b| PyBytes::new(py, &b))
        })
    }

    /// 原始 RGBA 字节（成功时，直通内存布局）。
    fn rgba<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.inner.image.as_ref().map(|img| PyBytes::new(py, img.as_raw()))
    }

    /// 图像宽度（成功时）。
    #[getter]
    fn width(&self) -> Option<u32> {
        self.inner.image.as_ref().map(|img| img.width())
    }

    /// 图像高度（成功时）。
    #[getter]
    fn height(&self) -> Option<u32> {
        self.inner.image.as_ref().map(|img| img.height())
    }

    /// 保存为 PNG 文件（成功时）。
    fn save(&self, path: &str) -> PyResult<()> {
        let img = self
            .inner
            .image
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("capture failed, no image"))?;
        img.save(path).map_err(|e| PyRuntimeError::new_err(format!("save failed: {e}")))
    }

    fn __repr__(&self) -> String {
        format!(
            "CaptureResult(ok={}, backend={}, error={:?}, geo={:?})",
            self.inner.image.is_some(),
            self.inner.backend.name(),
            self.inner.error,
            self.inner.source_geometry
        )
    }
}

/// 单个窗口的捕获结果。
#[pyclass(name = "WindowCapture", module = "dracopho_capture_core")]
pub struct PyWindowCapture {
    inner: capture_types::WindowCapture,
}

#[pymethods]
impl PyWindowCapture {
    /// 命中的窗口信息。
    #[getter]
    fn window(&self) -> PyWindowInfo {
        PyWindowInfo { inner: self.inner.window.clone() }
    }

    /// 匹配时使用的选择器原文。
    #[getter]
    fn selector(&self) -> String {
        self.inner.selector.clone()
    }

    /// 是否拿到窗口自身内容（X11 XComposite / KWin CaptureWindow）。
    #[getter]
    fn object_capture(&self) -> bool {
        self.inner.object_capture
    }

    /// 错误信息（失败时）。
    #[getter]
    fn error(&self) -> Option<String> {
        self.inner.error.clone()
    }

    /// 图像 PNG 字节（成功时）。
    fn png<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.inner.image.as_ref().and_then(|img| {
            image_to_png(img).ok().map(|b| PyBytes::new(py, &b))
        })
    }

    /// 原始 RGBA 字节（成功时）。
    fn rgba<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.inner.image.as_ref().map(|img| PyBytes::new(py, img.as_raw()))
    }

    #[getter]
    fn width(&self) -> Option<u32> {
        self.inner.image.as_ref().map(|img| img.width())
    }

    #[getter]
    fn height(&self) -> Option<u32> {
        self.inner.image.as_ref().map(|img| img.height())
    }
}

// ---------------------------------------------------------------------------
// 窗口 / 输出 / 路由信息
// ---------------------------------------------------------------------------

/// 窗口信息。
#[pyclass(name = "WindowInfo", module = "dracopho_capture_core", skip_from_py_object)]
#[derive(Clone)]
pub struct PyWindowInfo {
    inner: core_lib::window::WindowInfo,
}

#[pymethods]
impl PyWindowInfo {
    #[getter]
    fn id(&self) -> String {
        self.inner.id.clone()
    }
    #[getter]
    fn title(&self) -> String {
        self.inner.title.clone()
    }
    /// WM_CLASS class（Python 关键字 `class`，故命名为 `window_class`）。
    #[getter]
    fn window_class(&self) -> String {
        self.inner.class.clone()
    }
    #[getter]
    fn instance(&self) -> String {
        self.inner.instance.clone()
    }
    #[getter]
    fn pid(&self) -> i64 {
        self.inner.pid
    }
    #[getter]
    fn geometry(&self) -> (i32, i32, i32, i32) {
        self.inner.geometry
    }
    #[getter]
    fn monitor(&self) -> String {
        self.inner.monitor.clone()
    }
    #[getter]
    fn workspace(&self) -> String {
        self.inner.workspace.clone()
    }
    #[getter]
    fn z_order(&self) -> Option<i32> {
        self.inner.z_order
    }

    fn __repr__(&self) -> String {
        format!(
            "WindowInfo(title={:?}, class={:?}, geo={:?}, id={:?})",
            self.inner.title, self.inner.class, self.inner.geometry, self.inner.id
        )
    }
}

/// 输出（显示器）信息。
#[pyclass(name = "OutputInfo", module = "dracopho_capture_core", skip_from_py_object)]
#[derive(Clone)]
pub struct PyOutputInfo {
    inner: core_lib::output::OutputInfo,
}

#[pymethods]
impl PyOutputInfo {
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }
    #[getter]
    fn geometry(&self) -> (i32, i32, i32, i32) {
        self.inner.geometry
    }
    fn __repr__(&self) -> String {
        format!("OutputInfo(name={:?}, geo={:?})", self.inner.name, self.inner.geometry)
    }
}

/// 路由方案（智能感知结果）。
#[pyclass(name = "RoutingPlan", module = "dracopho_capture_core", skip_from_py_object)]
#[derive(Clone)]
pub struct PyRoutingPlan {
    inner: core_lib::routing::RoutingPlan,
}

#[pymethods]
impl PyRoutingPlan {
    /// 识别出的会话/桌面类型（wayland-gnome / wayland-kde / wayland-wlroots / x11 …）。
    #[getter]
    fn session(&self) -> &'static str {
        self.inner.session.name()
    }

    /// 按优先级排序的推荐后端名列表。
    #[getter]
    fn recommended(&self) -> Vec<&'static str> {
        self.inner.recommended.iter().map(|b: &Backend| b.name()).collect()
    }

    /// 可直接赋给 CaptureRequest 的路由参数描述。
    #[getter]
    fn route(&self) -> PyRouteMode {
        PyRouteMode { inner: self.inner.route.clone() }
    }

    /// 补充说明。
    #[getter]
    fn notes(&self) -> Vec<String> {
        self.inner.notes.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "RoutingPlan(session={}, recommended={:?})",
            self.inner.session.name(),
            self.inner.recommended.iter().map(|b: &Backend| b.name()).collect::<Vec<_>>()
        )
    }
}

// ---------------------------------------------------------------------------
// 流式捕获
// ---------------------------------------------------------------------------

/// 流式捕获会话（滚动截图/录制逐帧拉取）。
#[pyclass(name = "Stream", module = "dracopho_capture_core")]
pub struct PyStream {
    inner: capture_types::Stream,
}

#[pymethods]
impl PyStream {
    /// 拉取下一帧，返回 `(png_bytes, frame_time_ms)`；超时返回 None。
    ///
    /// - `min_frame_time_ms`：只返回到达时间 ≥ 该值的帧（滚动隐藏自身 UI 后跳过陈旧帧）。
    /// - `timeout_ms`：等待上限。
    #[pyo3(signature = (min_frame_time_ms=0, timeout_ms=1000))]
    fn next_frame<'py>(
        &self,
        py: Python<'py>,
        min_frame_time_ms: u64,
        timeout_ms: u64,
    ) -> PyResult<Option<(Bound<'py, PyBytes>, u64)>> {
        match self.inner.next_frame(min_frame_time_ms, timeout_ms) {
            Ok(Some((img, t))) => {
                let png = image_to_png(&img)?;
                Ok(Some((PyBytes::new(py, &png), t)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(PyRuntimeError::new_err(e)),
        }
    }

    /// 结束流式捕获（释放共享会话）。
    fn stop(&self) {
        self.inner.stop();
    }
}

// ---------------------------------------------------------------------------
// 顶层函数
// ---------------------------------------------------------------------------

/// 捕获一帧（全屏 / 区域 / 指定输出 / 路由）。
#[pyfunction]
fn capture_frame(request: PyRef<'_, PyCaptureRequest>) -> PyCaptureResult {
    PyCaptureResult { inner: capture_types::capture_frame(&request.inner) }
}

/// 捕获多个显示器，返回**每个屏幕一张图**的集合（不拼接）。
///
/// 跨屏幕区域（显式 source_geometry 跨越显示器；X11 整虚拟桌面）用
/// `capture_frame` 的单帧组合/裁剪，二者严禁混用。
#[pyfunction]
fn capture_outputs(request: PyRef<'_, PyCaptureRequest>) -> Vec<PyCaptureResult> {
    capture_types::capture_outputs(&request.inner)
        .into_iter()
        .map(|inner| PyCaptureResult { inner })
        .collect()
}

/// 捕获多个指定窗口（每个窗口一张图）。
#[pyfunction]
fn capture_windows(request: PyRef<'_, PyCaptureRequest>) -> Vec<PyWindowCapture> {
    capture_types::capture_windows(&request.inner)
        .into_iter()
        .map(|inner| PyWindowCapture { inner })
        .collect()
}

/// 启动一个流式捕获会话。
#[pyfunction]
fn start_stream(request: PyRef<'_, PyCaptureRequest>) -> PyResult<PyStream> {
    let inner = capture_types::start_stream(&request.inner)
        .map_err(PyRuntimeError::new_err)?;
    Ok(PyStream { inner })
}

/// 枚举当前会话的所有可见窗口。
#[pyfunction]
#[pyo3(signature = (include_hidden=false))]
fn list_windows(include_hidden: bool) -> Vec<PyWindowInfo> {
    core_lib::window::list_windows(include_hidden)
        .into_iter()
        .map(|inner| PyWindowInfo { inner })
        .collect()
}

/// 枚举当前会话的输出（显示器）。
#[pyfunction]
fn list_outputs() -> Vec<PyOutputInfo> {
    core_lib::output::list_outputs()
        .into_iter()
        .map(|inner| PyOutputInfo { inner })
        .collect()
}

/// 列出当前会话可用的自研后端名。
#[pyfunction]
fn available_backends() -> Vec<&'static str> {
    capture_types::available_backends().iter().map(|b: &Backend| b.name()).collect()
}

/// 智能感知当前会话并返回推荐路由方案。
#[pyfunction]
fn detect_routing() -> PyRoutingPlan {
    PyRoutingPlan { inner: core_lib::routing::detect_routing() }
}

/// 是否已授权（存在持久化 restore token）。
#[pyfunction]
fn authorized() -> bool {
    core_lib::auth::authorized()
}

/// 读取已保存的 restore token（无则返回 None）。
#[pyfunction]
fn restore_token() -> Option<String> {
    core_lib::auth::restore_token()
}

/// 保存 restore token（交互授权成功后调用）。
#[pyfunction]
fn save_restore_token(token: &str) {
    core_lib::auth::save_restore_token(token);
}

/// 删除已保存的 restore token。
#[pyfunction]
fn clear_restore_token() {
    core_lib::auth::clear_restore_token();
}

/// 便捷预检：当前持久化的 token 是否仍可静默恢复（无头录制前调用）。
#[pyfunction]
fn verify_saved_token() -> PyResult<bool> {
    core_lib::auth::verify_saved_token().map_err(PyRuntimeError::new_err)
}

/// 预检指定 token 是否仍可恢复。
#[pyfunction]
fn verify_restore_token(token: &str) -> PyResult<bool> {
    core_lib::auth::verify_restore_token(token).map_err(PyRuntimeError::new_err)
}

/// 停止当前复用的流式后端会话（滚动截图暂停/失败时调用）。
#[pyfunction]
fn stop_active_stream() {
    capture_types::stop_active_stream();
}

// ---------------------------------------------------------------------------
// 模块注册
// ---------------------------------------------------------------------------

#[pymodule]
fn dracopho_capture_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCaptureRequest>()?;
    m.add_class::<PyCaptureResult>()?;
    m.add_class::<PyWindowCapture>()?;
    m.add_class::<PyRouteMode>()?;
    m.add_class::<PyWindowMatch>()?;
    m.add_class::<PyWindowInfo>()?;
    m.add_class::<PyOutputInfo>()?;
    m.add_class::<PyRoutingPlan>()?;
    m.add_class::<PyStream>()?;

    m.add_function(wrap_pyfunction!(parse_match, m)?)?;
    m.add_function(wrap_pyfunction!(capture_frame, m)?)?;
    m.add_function(wrap_pyfunction!(capture_outputs, m)?)?;
    m.add_function(wrap_pyfunction!(capture_windows, m)?)?;
    m.add_function(wrap_pyfunction!(start_stream, m)?)?;
    m.add_function(wrap_pyfunction!(list_windows, m)?)?;
    m.add_function(wrap_pyfunction!(list_outputs, m)?)?;
    m.add_function(wrap_pyfunction!(available_backends, m)?)?;
    m.add_function(wrap_pyfunction!(detect_routing, m)?)?;
    m.add_function(wrap_pyfunction!(authorized, m)?)?;
    m.add_function(wrap_pyfunction!(restore_token, m)?)?;
    m.add_function(wrap_pyfunction!(save_restore_token, m)?)?;
    m.add_function(wrap_pyfunction!(clear_restore_token, m)?)?;
    m.add_function(wrap_pyfunction!(verify_saved_token, m)?)?;
    m.add_function(wrap_pyfunction!(verify_restore_token, m)?)?;
    m.add_function(wrap_pyfunction!(stop_active_stream, m)?)?;
    Ok(())
}
