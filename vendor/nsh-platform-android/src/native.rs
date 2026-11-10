//! Rust 写的原生组件（本仓库加的）：0.4.0 的原生组件只能在 Java 里往 `NshView` 上注册，
//! 打包出的 `AppActivity` 又没地方放应用的 Java 代码。这里让应用在 Rust 里按名字注册工厂
//! （[`register_component`]），Java 的 `Mounter` 找不到 Java 工厂时建一个 `RustComponent`：
//! 它的视图是一个 `SurfaceView`，画面、属性、`command`、硬件按键和手柄的轴都交给 Rust 的组件。
//! 全部回调都在 UI 线程上。

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};

use jni::{
    JNIEnv,
    objects::{GlobalRef, JClass, JFloatArray, JObject, JString, JValue},
    signature::ReturnType,
    sys::{JNI_FALSE, JNI_TRUE, jboolean, jfloat, jint, jlong},
};
use nsh_core::{EventKind, EventPayload, JsonValue, NodeId};

use crate::host::{Host, host};
use crate::java::{java, with_env};
use crate::registry::Registry;

/// 一个 Rust 写的原生组件。
pub trait NativeComponent {
    /// 属性变了（`None`：去掉了）。刚建好时应用设过的属性都会来一遍。
    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>);

    /// JS 调了 `ref.command(name, args)`。
    fn command(&mut self, name: &str, args: &JsonValue) {
        let _ = (name, args);
    }

    /// `SurfaceView` 的画面建好、尺寸变了（`Some`），或者没了（`None`：返回之前不能再用旧的）。
    fn surface(&mut self, surface: Option<Surface>) {
        let _ = surface;
    }

    /// 硬件键盘、手柄的按键（在应用的快捷键之前）；返回 true 吞掉，不再往下传。
    fn key(&mut self, key: &Key) -> bool {
        let _ = key;
        false
    }

    /// 手柄的轴（[`AXES`] 的顺序）；返回 true 吞掉。
    fn motion(&mut self, device: i32, axes: &[f32]) -> bool {
        let _ = (device, axes);
        false
    }
}

/// `motion` 收到的轴：`MotionEvent.AXIS_X`、`Y`、`Z`、`RZ`、`HAT_X`、`HAT_Y`、`LTRIGGER`、`RTRIGGER`、`BRAKE`、`GAS`。
pub const AXES: [i32; 10] = [0, 1, 11, 14, 15, 16, 17, 18, 23, 22];

/// 一个按键事件（`KeyEvent`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    /// `KeyEvent.getKeyCode()`。
    pub code: i32,
    /// `KeyEvent.getSource()`（`InputDevice.SOURCE_*`）。
    pub source: i32,
    /// 按下（否则是松开）。
    pub down: bool,
    /// `getRepeatCount()`：系统自己的自动重复。
    pub repeat: i32,
    /// `getMetaState()`。
    pub meta: i32,
    /// 焦点在输入框（或别的组件）里：在打字。
    pub editing: bool,
}

/// `SurfaceView` 的画面。
pub struct Surface {
    /// `ANativeWindow_fromSurface` 拿到的窗口，带一个引用：组件负责 `ANativeWindow_release`
    /// （比如交给 `ndk::native_window::NativeWindow::from_ptr`）。
    pub window: NonNull<c_void>,
    /// 像素宽。
    pub width: u32,
    /// 像素高。
    pub height: u32,
}

/// 组件能对应用说的话，以及它要的 Android 环境。可以跨线程用。
#[derive(Clone)]
pub struct ComponentContext {
    host: Arc<Host>,
    id: NodeId,
    shared: Arc<Shared>,
}

struct Shared {
    /// 应用的 `Context`（`getApplicationContext()`）。
    context: GlobalRef,
    code_cache_dir: PathBuf,
    density: f32,
}

impl ComponentContext {
    /// 发事件：JS 的 `on<Event>` 收到 `data`。
    pub fn emit(&self, event: &str, data: JsonValue) {
        self.host.dispatch(self.id, EventKind::NativeEvent, EventPayload::Native { name: event.into(), data });
    }

    /// 组件自己的尺寸（dp），样式没定的那一维用它。
    pub fn set_size(&self, width: f32, height: f32) {
        self.host.set_component_size(self.id, width, height);
    }

    /// `asset:/…` 文件：APK 里的资源，或者 `nsh run` 的开发服务器（`NshView.readAsset`）。会阻塞，不要在 UI 线程上调。
    pub fn read_asset(&self, uri: &str) -> Result<Vec<u8>, String> {
        let bytes: Option<Vec<u8>> = with_env(|env| {
            let uri = env.new_string(uri)?;
            // SAFETY: 和 `java::init` 里查过的签名一致。
            let array = unsafe {
                env.call_method_unchecked(self.host.view.as_obj(), java().view_read_asset, ReturnType::Array, &[JValue::Object(&uri).as_jni()])
            }?
            .l()?;
            if array.is_null() {
                return Ok(None);
            }
            env.convert_byte_array(jni::objects::JByteArray::from(array)).map(Some)
        });
        bytes.ok_or_else(|| format!("读不到 {uri}"))
    }

    /// 进程的 `JavaVM*`。
    pub fn java_vm(&self) -> *mut c_void {
        java().vm.get_java_vm_pointer().cast()
    }

    /// 应用的 `Context`（全局引用，和组件一样长寿）。
    pub fn android_context(&self) -> *mut c_void {
        self.shared.context.as_obj().as_raw().cast()
    }

    /// `Context.getCodeCacheDir()`：放编好的代码的缓存目录，App 更新后系统会清掉。
    pub fn code_cache_dir(&self) -> &Path {
        &self.shared.code_cache_dir
    }

    /// 每 dp 多少像素。
    pub fn density(&self) -> f32 {
        self.shared.density
    }
}

/// 建组件的工厂。
pub type ComponentFactory = Arc<dyn Fn(ComponentContext) -> Box<dyn NativeComponent> + Send + Sync>;

fn registry() -> &'static Registry<ComponentFactory> {
    static REGISTRY: OnceLock<Registry<ComponentFactory>> = OnceLock::new();
    REGISTRY.get_or_init(Registry::new)
}

/// 把 `name`（TSX 的 `nativeComponent(name, …)`）登记成 Rust 组件；同名的换掉。
/// 生成的宿主不调组件注册函数，在应用的 `#[native_module]` 里调。
pub fn register_component(name: &str, factory: ComponentFactory) {
    registry().register(name, factory);
}

fn string(env: &mut JNIEnv<'_>, s: &JString<'_>) -> Option<String> {
    if s.is_null() {
        return None;
    }
    env.get_string(s).ok().map(String::from)
}

/// JSON 文本（null 当 `null`），解析不了记 warn。
fn json(env: &mut JNIEnv<'_>, s: &JString<'_>) -> Option<JsonValue> {
    let text = string(env, s)?;
    serde_json::from_str(&text).inspect_err(|e| log::warn!("组件收到的不是 JSON: {e}")).ok()
}

/// # Safety
/// `ptr` 是 `rustComponentCreate` 给的、还没 `rustComponentDestroy` 的。
unsafe fn component<'a>(ptr: jlong) -> Option<&'a mut Box<dyn NativeComponent>> {
    // SAFETY: 由调用方保证。
    (ptr != 0).then(|| unsafe { &mut *(ptr as *mut Box<dyn NativeComponent>) })
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentExists(mut env: JNIEnv<'_>, _: JClass<'_>, name: JString<'_>) -> jboolean {
    let Some(name) = string(&mut env, &name) else { return JNI_FALSE };
    if registry().get(&name).is_some() { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentCreate(
    mut env: JNIEnv<'_>,
    _: JClass<'_>,
    handle: jlong,
    id: jint,
    name: JString<'_>,
    context: JObject<'_>,
    code_cache_dir: JString<'_>,
    density: jfloat,
) -> jlong {
    let Some(name) = string(&mut env, &name) else { return 0 };
    let Some(factory) = registry().get(&name) else {
        if registry().first_miss(&name) {
            log::warn!("没有叫 `{name}` 的 Rust 组件");
        }
        return 0;
    };
    let Ok(context) = env.new_global_ref(context) else { return 0 };
    let code_cache_dir = string(&mut env, &code_cache_dir).map(PathBuf::from).unwrap_or_default();
    // SAFETY: Java 传的是活着的 handle。
    let host = unsafe { host(handle) }.clone();
    let ctx = ComponentContext { host, id: id as NodeId, shared: Arc::new(Shared { context, code_cache_dir, density }) };
    Box::into_raw(Box::new(factory(ctx))) as jlong
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentProp(mut env: JNIEnv<'_>, _: JClass<'_>, ptr: jlong, name: JString<'_>, value: JString<'_>) {
    let Some(name) = string(&mut env, &name) else { return };
    let value = json(&mut env, &value).filter(|v| !v.is_null());
    // SAFETY: Java 传的是活着的指针。
    if let Some(c) = unsafe { component(ptr) } {
        c.set_prop(&name, value.as_ref());
    }
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentCommand(mut env: JNIEnv<'_>, _: JClass<'_>, ptr: jlong, name: JString<'_>, args: JString<'_>) {
    let Some(name) = string(&mut env, &name) else { return };
    let args = json(&mut env, &args).unwrap_or(JsonValue::Null);
    // SAFETY: Java 传的是活着的指针。
    if let Some(c) = unsafe { component(ptr) } {
        c.command(&name, &args);
    }
}

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeWindow_fromSurface(env: *mut jni::sys::JNIEnv, surface: jni::sys::jobject) -> *mut c_void;
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentSurface(env: JNIEnv<'_>, _: JClass<'_>, ptr: jlong, surface: JObject<'_>, width: jint, height: jint) {
    // SAFETY: Java 传的是活着的指针。
    let Some(c) = (unsafe { component(ptr) }) else { return };
    if surface.is_null() {
        return c.surface(None);
    }
    // SAFETY: `surface` 是一个 `android.view.Surface`。
    let window = unsafe { ANativeWindow_fromSurface(env.get_raw(), surface.as_raw()) };
    match NonNull::new(window) {
        Some(window) => c.surface(Some(Surface { window, width: width.max(0) as u32, height: height.max(0) as u32 })),
        None => log::error!("ANativeWindow_fromSurface 没拿到窗口"),
    }
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentKey(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    ptr: jlong,
    code: jint,
    source: jint,
    down: jboolean,
    repeat: jint,
    meta: jint,
    editing: jboolean,
) -> jboolean {
    // SAFETY: Java 传的是活着的指针。
    let Some(c) = (unsafe { component(ptr) }) else { return JNI_FALSE };
    let key = Key { code, source, down: down == JNI_TRUE, repeat, meta, editing: editing == JNI_TRUE };
    if c.key(&key) { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentMotion(env: JNIEnv<'_>, _: JClass<'_>, ptr: jlong, device: jint, axes: JFloatArray<'_>) -> jboolean {
    // SAFETY: Java 传的是活着的指针。
    let Some(c) = (unsafe { component(ptr) }) else { return JNI_FALSE };
    let mut values = [0.0; AXES.len()];
    if env.get_float_array_region(&axes, 0, &mut values).is_err() {
        return JNI_FALSE;
    }
    if c.motion(device, &values) { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_rustComponentDestroy(_: JNIEnv<'_>, _: JClass<'_>, ptr: jlong) {
    if ptr != 0 {
        // SAFETY: `rustComponentCreate` 给的，Java 只释放一次。
        drop(unsafe { Box::from_raw(ptr as *mut Box<dyn NativeComponent>) });
    }
}
