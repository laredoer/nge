//! 把效果器链（[`Chain`]）接到 kira 的轨道上：一个 kira 的 `Effect`，链子怎么变都在它里面（kira 的效果器建轨道时就定死了）。
//! 游戏线程和音频线程之间用两个带锁的队列传：音频线程只 `try_lock`，拿不到就下一批再说，不等、不分配、不释放内存。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;

use crate::effects::{Chain, EffectSpec, MAX_EFFECTS, Swapper, same_shape, sanitize_chain};

/// 队列先留这么多位置：正常用不到这么多，音频线程上不会扩容。
const QUEUE: usize = 16;

enum Command {
    /// 形状变了：换成这条建好的链（交叉淡变）。
    Replace(Chain),
    /// 形状没变：参数在这么多秒里过渡过去。定长数组，不带堆内存。
    Retarget([EffectSpec; MAX_EFFECTS], usize, f32),
}

#[derive(Default)]
struct Shared {
    commands: Mutex<VecDeque<Command>>,
    /// 换下来的旧链，游戏线程拿走释放。
    retired: Mutex<Vec<Chain>>,
    /// 音频线程报上来的采样率（0 是还不知道）。
    rate: AtomicU32,
}

/// 游戏线程这边的把手。
pub(crate) struct ChainHandle {
    shared: Arc<Shared>,
    specs: Vec<EffectSpec>,
}

impl ChainHandle {
    #[cfg(test)]
    pub fn specs(&self) -> &[EffectSpec] {
        &self.specs
    }

    /// 换成这条链（已经校验过的）：形状没变就过渡参数，变了就按音频线程的采样率建一条新的发过去。
    pub fn set(&mut self, specs: &[EffectSpec], fade: f32) {
        let command = if same_shape(&self.specs, specs) {
            let mut fixed = [EffectSpec::Limiter { ceiling: 0.0, release: 0.1 }; MAX_EFFECTS];
            fixed[..specs.len()].copy_from_slice(specs);
            Command::Retarget(fixed, specs.len(), fade)
        } else {
            Command::Replace(Chain::new(specs, self.shared.rate.load(Ordering::Relaxed)))
        };
        self.specs = specs.to_vec();
        self.shared.commands.lock().unwrap_or_else(|e| e.into_inner()).push_back(command);
        self.collect();
    }

    /// 测试用：有几条旧链在等着释放。
    #[cfg(test)]
    pub fn retired(&self) -> usize {
        self.shared.retired.lock().unwrap().len()
    }

    /// 释放音频线程换下来的旧链。每帧调一次。
    pub fn collect(&mut self) {
        let retired = std::mem::replace(&mut *self.shared.retired.lock().unwrap_or_else(|e| e.into_inner()), Vec::with_capacity(QUEUE));
        drop(retired);
    }
}

/// 音频线程这边：一条正在用的链，加上换链时的交叉淡变。
pub(crate) struct ChainEffect {
    shared: Arc<Shared>,
    swap: Swapper,
    /// 没送出去的旧链（队列正被游戏线程锁着），下一批再送。
    held: Option<Chain>,
}

/// 建一个效果器和它的把手，一开始是 `specs` 这条链。
pub(crate) fn chain(specs: &[EffectSpec]) -> (ChainEffect, ChainHandle) {
    let specs = sanitize_chain(specs);
    let shared = Arc::new(Shared { commands: Mutex::new(VecDeque::with_capacity(QUEUE)), retired: Mutex::new(Vec::with_capacity(QUEUE)), rate: AtomicU32::new(0) });
    let effect = ChainEffect { shared: shared.clone(), swap: Swapper::new(Chain::new(&specs, 0)), held: None };
    (effect, ChainHandle { shared, specs })
}

impl ChainEffect {
    /// 采样率定了（或者变了）：按新的采样率重建，这里可以分配内存。
    pub(crate) fn set_rate(&mut self, rate: u32) {
        self.shared.rate.store(rate, Ordering::Relaxed);
        if self.swap.chain().rate() != rate.max(1) {
            let specs = self.swap.chain().specs();
            self.swap = Swapper::new(Chain::new(&specs, rate));
        }
    }

    /// 每批采样之前：收命令，把淡完的旧链送回去。
    pub(crate) fn pump(&mut self) {
        let shared = self.shared.clone();
        if let Ok(mut commands) = shared.commands.try_lock() {
            while let Some(command) = commands.pop_front() {
                match command {
                    Command::Replace(chain) => {
                        // 建的时候采样率还不对（很少见）：只能在这儿重建
                        let chain = if chain.rate() == self.swap.chain().rate() { chain } else { Chain::new(&chain.specs(), self.swap.chain().rate()) };
                        if let Some(cut) = self.swap.replace(chain) {
                            self.retire(cut);
                        }
                    }
                    Command::Retarget(specs, len, fade) => {
                        self.swap.chain().retarget(&specs[..len], fade);
                    }
                }
            }
        }
        if let Some(old) = self.swap.take_retired() {
            self.retire(old);
        }
        if let Some(old) = self.held.take() {
            self.retire(old);
        }
    }

    /// 旧链送回游戏线程释放；队列锁着或者满了就先留着。
    fn retire(&mut self, chain: Chain) {
        let mut chain = Some(chain);
        if let Ok(mut retired) = self.shared.retired.try_lock()
            && retired.len() < retired.capacity()
        {
            retired.extend(chain.take());
        }
        // 没送出去：先留着下一批再送（留着的位置也占了才在这儿放掉，连着换好几次链才会走到）
        if chain.is_some() {
            self.held = chain;
        }
    }

    #[inline]
    pub(crate) fn run(&mut self, frame: [f32; 2]) -> [f32; 2] {
        self.swap.process(frame)
    }
}

impl Effect for ChainEffect {
    fn init(&mut self, sample_rate: u32, _internal_buffer_size: usize) {
        self.set_rate(sample_rate);
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.set_rate(sample_rate);
    }

    fn on_start_processing(&mut self) {
        self.pump();
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        for frame in input {
            let [left, right] = self.run([frame.left, frame.right]);
            *frame = Frame::new(left, right);
        }
    }
}

/// 给 kira 的 `TrackBuilder::add_effect` 用。
pub(crate) struct ChainBuilder(pub Vec<EffectSpec>);

impl EffectBuilder for ChainBuilder {
    type Handle = ChainHandle;

    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        let (effect, handle) = chain(&self.0);
        (Box::new(effect), handle)
    }
}
