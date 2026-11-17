//! Android：引擎画在 nsh 的 `RustComponent` 给的 `SurfaceView` 上（仓库里改过的 `nsh-platform-android`，见 docs/android.md）。
//! 画面来了才起引擎；画面没了（切后台、锁屏）同步暂停，回来时换上新的 surface。
//! 硬件键盘和手柄的事件由 `NshView` 先交给这里，按和 Mac 一样的键名转给游戏。

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Once};

use nge_core::bridge::{GAMEPAD, KEY, KEY_EVENT, KeyInput, PAD, RUMBLE_OUT, SUSPEND, Suspend};
use nge_core::{Engine, GameFactory, Msg, wgpu};
use nsh::{ComponentContext, ComponentFactory, Key, NativeComponent, Surface, register_component};
use raw_window_handle::{AndroidDisplayHandle, DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, WindowHandle};
use serde_json::{Value as JsonValue, json};

use crate::android_keys::{PadInput, device_of, from_gamepad, key_name, pad_id, shortcut};
use crate::android_state::{Pending, viewport};
use crate::input::{self, CAPTURE_KEYS, capture_keys};
use crate::pad;
use crate::rumble::{amplitude, millis};
use crate::shared::Proxy;

/// 把游戏视图注册成 `nge.game` 组件。在应用的 `#[native_module]` 里调（生成的宿主不调组件注册函数）。
pub fn register(factory: GameFactory) {
    register_component(nge_core::GAME_COMPONENT, component(factory));
}

pub fn component(factory: GameFactory) -> ComponentFactory {
    Arc::new(move |ctx| Box::new(GameView::new(ctx, factory.clone())))
}

/// 建游戏视图时：管线缓存放 `codeCacheDir/nge`；第一次时给 cpal 要的 `ndk-context`。
fn setup(ctx: &ComponentContext) {
    nge_core::pipeline_cache::set_dir(ctx.code_cache_dir().join("nge"));
    static CONTEXT: Once = Once::new();
    CONTEXT.call_once(|| {
        // 只要 VM 和应用的 Context：另建一份 Context 的全局引用交出去，和进程一样长（不拿着视图和 Activity）
        let context = ctx.new_android_context_ref();
        if context.is_null() {
            return log::error!("拿不到应用的 Context，声音可能出不来");
        }
        // SAFETY: VM 是进程的；`context` 是全局引用，不释放
        unsafe { ndk_context::initialize_android_context(ctx.java_vm(), context) };
    });
}

/// `asset:/` 文件走 `NshView.readAsset`（APK 里的资源，或 `nsh run` 的开发服务器）。
struct ViewAssets(ComponentContext);

impl nge_assets::AssetSource for ViewAssets {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        self.0.read_asset(uri)
    }
}

/// `wgpu::Surface` 自己拿着窗口：surface 释放时才 `ANativeWindow_release`。
struct Window(ndk::native_window::NativeWindow);

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: Android 的显示句柄没有内容
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Android(AndroidDisplayHandle::new())) })
    }
}

/// 只开 Vulkan：3D 要顶点阶段的 storage buffer、sRGB 贴图要 `view_formats`，GLES 上都不保证。
fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::VULKAN;
    wgpu::Instance::new(desc)
}

fn surface(instance: &wgpu::Instance, window: NonNull<c_void>) -> Result<wgpu::Surface<'static>, String> {
    // SAFETY: `RustComponent` 给的窗口带一个引用，交给 NativeWindow 管
    let window = unsafe { ndk::native_window::NativeWindow::from_ptr(window.cast()) };
    instance.create_surface(Window(window)).map_err(|e| e.to_string())
}

pub struct GameView {
    ctx: ComponentContext,
    /// 给 `nge_assets` 的资源来源（视图没了时放掉）。
    assets: Arc<dyn nge_assets::AssetSource>,
    factory: GameFactory,
    instance: wgpu::Instance,
    proxy: Option<Proxy>,
    /// 起引擎失败过：不再试。
    failed: bool,
    /// 现在画着的窗口（只拿来比是不是同一个；引用在 surface 手里）。
    window: Option<NonNull<c_void>>,
    /// 画面来之前收到的属性和 command。
    pending: Pending,
    /// prop `captureKeys`。
    capture: Option<Vec<String>>,
    /// 转发过按下、还没松开的键盘的键。
    keys: BTreeSet<&'static str>,
    /// 手柄：按钮和轴合起来。
    pads: PadInput,
    /// 每个手柄上次发出去的状态（按设备的那条路，见 docs/input-actions.md）。
    devices: pad::Devices,
    /// 画面没了时停过游戏（声音），回来时恢复。
    suspended: bool,
}

impl GameView {
    pub fn new(ctx: ComponentContext, factory: GameFactory) -> Self {
        setup(&ctx);
        // `asset:/` 从 `NshView` 读
        let assets: Arc<dyn nge_assets::AssetSource> = Arc::new(ViewAssets(ctx.clone()));
        nge_assets::set_source(assets.clone());
        Self {
            ctx,
            assets,
            factory,
            instance: instance(),
            proxy: None,
            failed: false,
            window: None,
            pending: Pending::default(),
            capture: None,
            keys: BTreeSet::new(),
            pads: PadInput::default(),
            devices: pad::Devices::default(),
            suspended: false,
        }
    }

    fn send(&mut self, msg: Msg) {
        match &self.proxy {
            Some(proxy) => proxy.send(msg),
            None => self.pending.push(msg),
        }
    }

    fn error(&mut self, message: String) {
        log::error!("{message}");
        self.failed = true;
        self.ctx.emit("error", json!({ "message": message }));
    }

    /// 第一次有画面：起引擎，把攒着的消息发过去。
    fn start(&mut self, surface: wgpu::Surface<'static>) {
        let ctx = self.ctx.clone();
        // 引擎发的震动不交给 NSH，直接让手柄震（渲染线程上调 Java）
        let emit = Box::new(move |n: &str, d: JsonValue| {
            if n != RUMBLE_OUT.name() {
                return ctx.emit(n, d);
            }
            match RUMBLE_OUT.decode(&d) {
                // 强度、时长是 0 就是停（Java 那边 cancel）；指定了手柄却换不回设备号的不震，不能变成「所有手柄」
                Ok(r) => match r.pad.map(device_of) {
                    Some(None) => log::warn!("手柄 {:?} 不是 Android 的设备号", r.pad),
                    device => ctx.vibrate_pad(device.flatten(), amplitude(r.low), amplitude(r.high), millis(r.duration)),
                },
                Err(e) => log::warn!("{e}"),
            }
        });
        match Engine::spawn(self.instance.clone(), surface, emit, self.factory.clone(), Box::new(())) {
            Ok(engine) => {
                let proxy = Proxy::new(engine, true);
                for msg in self.pending.take() {
                    proxy.send(msg);
                }
                self.proxy = Some(proxy);
            }
            Err(e) => self.error(e),
        }
    }

    fn forward(&self, code: &'static str, down: bool, repeat: bool) {
        let key = KeyInput { code: code.to_owned(), down, repeat };
        if let Some(proxy) = &self.proxy {
            proxy.command(KEY.name(), &KEY.encode(&key));
        }
        self.ctx.emit(KEY_EVENT.name(), KEY_EVENT.encode(&key));
    }

    /// 按着的键、手柄全部补发松开，摇杆归零（画面没了、窗口失焦时）。
    fn release_input(&mut self) {
        let held = std::mem::take(&mut self.keys);
        for code in held {
            self.forward(code, false, false);
        }
        let changes = self.pads.clear();
        self.forward_pads(changes);
        // 手柄还算连着，只是都松开了
        for input in self.devices.release() {
            self.send(Msg::Command(PAD.name().to_owned(), PAD.encode(&input)));
        }
        // 正在震的停掉
        self.ctx.vibrate_pad(None, 0, 0, 0);
    }

    /// 设备 `device` 的状态变了：按设备发一份（第一次就是接入）。虚拟设备（负的号）不算手柄。
    fn forward_device(&mut self, device: i32) {
        let Some(id) = pad_id(device) else { return };
        let state = self.pads.state(device, &self.devices.keys(id));
        if let Some(input) = self.devices.set(id, state) {
            self.send(Msg::Command(PAD.name().to_owned(), PAD.encode(&input)));
        }
    }

    /// 停下、恢复游戏（声音）：渲染线程停着也当场处理。
    fn suspend(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        if let Some(proxy) = &self.proxy {
            proxy.command(SUSPEND.name(), &SUSPEND.encode(&Suspend { suspended }));
        }
    }

    fn forward_pads(&self, changes: pad::Changes) {
        for code in changes.released {
            self.forward(code, false, false);
        }
        for code in changes.pressed {
            self.forward(code, true, false);
        }
        if let (Some(analog), Some(proxy)) = (changes.analog, &self.proxy) {
            proxy.command(GAMEPAD.name(), &GAMEPAD.encode(&analog.input()));
        }
    }
}

impl NativeComponent for GameView {
    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        if name == CAPTURE_KEYS {
            self.capture = capture_keys(value);
        }
        self.send(Msg::Prop(name.to_owned(), value.cloned()));
    }

    fn command(&mut self, name: &str, args: &JsonValue) {
        match &self.proxy {
            Some(proxy) => proxy.command(name, args),
            None => self.pending.push(Msg::Command(name.to_owned(), args.clone())),
        }
    }

    fn surface(&mut self, surface: Option<Surface>) {
        let Some(Surface { window, width, height }) = surface else {
            // 返回之前渲染线程已经停下，不再碰这个画面
            self.release_input();
            self.window = None;
            if let Some(proxy) = &self.proxy {
                proxy.set_in_window(false);
            }
            self.suspend(true);
            return;
        };
        let view = viewport(width, height, self.ctx.density());
        if self.window == Some(window) {
            // 同一个窗口只是尺寸变了：放掉多拿的引用，按新视口重新配置
            // SAFETY: 这一份引用是 `RustComponent` 给的，归我们
            drop(unsafe { ndk::native_window::NativeWindow::from_ptr(window.cast()) });
            if let Some(proxy) = &self.proxy {
                proxy.tick(view);
            }
            return;
        }
        if self.failed {
            // SAFETY: 同上
            drop(unsafe { ndk::native_window::NativeWindow::from_ptr(window.cast()) });
            return;
        }
        let new = match self::surface(&self.instance, window) {
            Ok(s) => s,
            Err(e) => return self.error(format!("创建 surface 失败: {e}")),
        };
        self.window = Some(window);
        if self.proxy.is_none() {
            self.start(new);
        } else if let Some(proxy) = &self.proxy {
            // 切后台回来：系统给的是新窗口，暂停着换上
            proxy.set_in_window(false);
            proxy.send(Msg::Surface(new));
        }
        self.suspend(false);
        if let Some(proxy) = &self.proxy {
            proxy.tick(view);
            proxy.set_in_window(true);
        }
    }

    fn key(&mut self, key: &Key) -> bool {
        let gamepad = from_gamepad(key.source);
        let Some(code) = key_name(key.code, gamepad) else { return false };
        if code.starts_with("Gamepad") {
            // 手柄的键总是给游戏（不然 B 会被系统当成返回）；自动重复、轴那边已经按着的不再发
            let changes = self.pads.button(key.device, code, key.down);
            self.forward_pads(changes);
            self.forward_device(key.device);
            return true;
        }
        let facts = input::Facts {
            code,
            kind: if key.down { input::Kind::Down } else { input::Kind::Up },
            cmd: shortcut(key.meta),
            repeat: key.repeat > 0,
            modifier_down: key.down,
            eligible: !key.editing && self.proxy.is_some(),
            capture: input::captured(self.capture.as_deref(), code),
        };
        let decision = input::decide(&mut self.keys, facts);
        if let Some((down, repeat)) = decision.forward {
            self.forward(code, down, repeat);
        }
        decision.consume
    }

    fn motion(&mut self, device: i32, axes: &[f32]) -> bool {
        let changes = self.pads.motion(device, axes);
        self.forward_pads(changes);
        self.forward_device(device);
        true
    }

    fn input_lost(&mut self, device: Option<i32>) {
        match device {
            Some(device) => {
                let changes = self.pads.remove(device);
                self.forward_pads(changes);
                if let Some(input) = pad_id(device).and_then(|id| self.devices.remove(id)) {
                    self.send(Msg::Command(PAD.name().to_owned(), PAD.encode(&input)));
                }
            }
            None => self.release_input(),
        }
    }
}

impl Drop for GameView {
    fn drop(&mut self) {
        // 不让全局的资源来源拿着这个视图（和它的 Activity）
        nge_assets::unset_source(&self.assets);
    }
}
