//! 浏览器（wasm）：没有线程，引擎跑在页面主线程上。平台层每个 `requestAnimationFrame` 调一次
//! [`Engine::frame`]；GPU 初始化（`request_adapter` / `request_device`）在浏览器的事件循环里异步等，
//! 期间收到的消息先攒着。一帧做什么见 `frame.rs`。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::rc::{Rc, Weak};
use std::sync::mpsc;

use web_time::Instant;

use crate::bridge::{ENGINE_ERROR, EngineError};
use crate::frame::{Core, Flow, Gate, Gpu, Msg, configure, request_gpu};
use crate::game::{Emit, GameFactory};

enum Phase {
    /// 等 GPU。
    Starting(Gate),
    /// GPU 好了，等视口（或者暂停着）。
    Ready(Gpu, Gate),
    Running { core: Core, paused: bool },
    /// 出错了或者收到了 `Quit`，什么都不做。
    Stopped,
}

struct Inner {
    phase: Phase,
    queue: VecDeque<Msg>,
    emit: Rc<Emit>,
    factory: GameFactory,
}

/// 页面上的引擎；接口和原生的一样（`send` / `pause` / `resume`），另外每帧要调 [`Engine::frame`]。
pub struct Engine {
    inner: Rc<RefCell<Inner>>,
}

impl Engine {
    /// 在 `canvas` 上建 WebGPU 画面并开始异步初始化 GPU。浏览器没有 WebGPU 时返回错误。
    pub fn spawn(canvas: web_sys::HtmlCanvasElement, emit: Emit, factory: GameFactory) -> Result<Self, String> {
        Self::spawn_after(canvas, emit, factory, async { Ok(()) })
    }

    /// 同 [`Engine::spawn`]，但先等 `before` 做完（比如把资源取进内存）再初始化 GPU、建游戏；它失败时发 `error`，不再往下走。
    pub fn spawn_after(
        canvas: web_sys::HtmlCanvasElement,
        emit: Emit,
        factory: GameFactory,
        before: impl Future<Output = Result<(), String>> + 'static,
    ) -> Result<Self, String> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        // 只用 WebGPU：3D 的蒙皮和变形在顶点阶段读 storage buffer，WebGL2 没有
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU;
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)).map_err(|e| e.to_string())?;
        let inner = Rc::new(RefCell::new(Inner { phase: Phase::Starting(Gate::default()), queue: VecDeque::new(), emit: Rc::new(emit), factory }));
        let weak = Rc::downgrade(&inner);
        wasm_bindgen_futures::spawn_local(async move {
            let gpu = match before.await {
                Ok(()) => request_gpu(&instance, surface).await.map_err(|e| format!("WebGPU 初始化失败: {e}")),
                Err(e) => Err(e),
            };
            gpu_ready(&weak, gpu);
        });
        Ok(Self { inner })
    }

    pub fn send(&self, msg: Msg) {
        self.inner.borrow_mut().queue.push_back(msg);
    }

    /// 暂停：下一帧起不再更新和画。页面上没有别的线程在画，不用等确认。
    pub fn pause(&self) -> bool {
        let (ack, _) = mpsc::sync_channel(1);
        self.send(Msg::Pause(ack));
        true
    }

    pub fn resume(&self) {
        self.send(Msg::Resume);
    }

    /// 游戏建好了（还没出错、没退出）。
    pub fn running(&self) -> bool {
        matches!(self.inner.borrow().phase, Phase::Running { .. })
    }

    /// 浏览器每帧调一次：处理攒下的消息，跑起来了就更新、画一帧。
    pub fn frame(&self) {
        let start = Instant::now();
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;
        let msgs: Vec<Msg> = inner.queue.drain(..).collect();
        inner.phase = match std::mem::replace(&mut inner.phase, Phase::Stopped) {
            Phase::Starting(mut gate) => {
                if msgs.into_iter().all(|m| gate.push(m)) { Phase::Starting(gate) } else { Phase::Stopped }
            }
            Phase::Ready(gpu, mut gate) => {
                if !msgs.into_iter().all(|m| gate.push(m)) {
                    Phase::Stopped
                } else if let Some(viewport) = gate.ready() {
                    start_game(inner, gpu, gate, viewport)
                } else {
                    Phase::Ready(gpu, gate)
                }
            }
            Phase::Running { mut core, mut paused } => {
                let mut quit = false;
                for msg in msgs {
                    let resume = matches!(msg, Msg::Resume);
                    match core.apply(msg) {
                        Flow::Continue if resume && paused => {
                            paused = false;
                            core.resumed();
                        }
                        Flow::Continue => {}
                        Flow::Pause(ack) => {
                            paused = true;
                            let _ = ack.send(());
                        }
                        Flow::Quit => quit = true,
                    }
                }
                if quit {
                    Phase::Stopped
                } else {
                    if !paused {
                        core.step(start);
                    }
                    Phase::Running { core, paused }
                }
            }
            Phase::Stopped => Phase::Stopped,
        };
    }
}

/// GPU 初始化完了（成功或失败）。引擎已经没了就什么都不做。
fn gpu_ready(inner: &Weak<RefCell<Inner>>, gpu: Result<Gpu, String>) {
    let Some(inner) = inner.upgrade() else { return };
    let mut inner = inner.borrow_mut();
    let Phase::Starting(gate) = std::mem::replace(&mut inner.phase, Phase::Stopped) else { return };
    match gpu {
        Ok(gpu) => inner.phase = Phase::Ready(gpu, gate),
        Err(e) => fail(&inner.emit, e),
    }
}

fn start_game(inner: &Inner, gpu: Gpu, gate: Gate, viewport: crate::Viewport) -> Phase {
    let (gfx, surface) = match configure(gpu, viewport) {
        Ok(ok) => ok,
        Err(e) => {
            fail(&inner.emit, e);
            return Phase::Stopped;
        }
    };
    let game = (inner.factory)(&gfx);
    let mut core = Core::new(game, gfx, surface, inner.emit.clone());
    for msg in gate.take() {
        core.apply(msg);
    }
    Phase::Running { core, paused: false }
}

fn fail(emit: &Emit, message: String) {
    log::error!("{message}");
    emit(ENGINE_ERROR.name(), ENGINE_ERROR.encode(&EngineError { message }));
}
