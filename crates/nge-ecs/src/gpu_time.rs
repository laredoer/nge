//! GPU 时间：每个 pass 前后写时间戳，帧末解析拷进读回缓冲，提交后异步映射，几帧后再读（不等 GPU，见 docs/big-scene-perf.md）。
//! 查询的下标在 pass 真的开始（[`PassTimer::writes`]）时才分，拿了计时器没用上的不占位置、不解析。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use bevy_ecs::prelude::{Resource, World};
use nge_core::{Gfx, wgpu};

/// 一帧最多量几个 pass，多出的不量，这一帧记成「不全」。
pub const MAX_PASSES: u32 = 64;
/// 读回缓冲的个数：GPU 落后几帧也不用等。
const SLOTS: usize = 3;

const FREE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;

/// 这一帧各 pass 在 GPU 上花了多久（毫秒）。`truncated` 是 pass 超过 [`MAX_PASSES`]，后面的没量（总时间偏小）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuTimes {
    pub total_ms: f32,
    pub passes: Vec<(&'static str, f32)>,
    pub truncated: bool,
}

/// 一帧里用到的查询：按 pass 开始的顺序排。
#[derive(Default)]
struct Used {
    labels: Vec<&'static str>,
    truncated: bool,
}

struct Slot {
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
    labels: Vec<&'static str>,
    truncated: bool,
    /// 第几帧写的：几帧同时读回来时只要最新的。
    frame: u64,
}

struct Clock {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    period: f32,
    /// 这一帧写进哪个读回缓冲；`None` 是都还没读完，这一帧不量。
    current: Option<usize>,
    used: Arc<Mutex<Used>>,
    frame: u64,
}

/// 资源：GPU 时间戳。设备不支持时间戳时 [`GpuClock::supported`] 是 `false`，什么也不量。
#[derive(Resource, Default)]
pub struct GpuClock {
    clock: Option<Clock>,
    /// 还没见过设备。
    started: bool,
    last: Option<GpuTimes>,
    /// `last` 上次取走以后又读回过。
    fresh: bool,
}

/// 一个 pass 的计时器（[`GpuClock::pass`] 给的），开 pass 时调 [`PassTimer::writes`]。
pub struct PassTimer {
    set: wgpu::QuerySet,
    used: Arc<Mutex<Used>>,
    label: &'static str,
}

impl PassTimer {
    /// 给 `begin_render_pass` 的 `timestamp_writes`：这时才分一对查询；这一帧已经用满时是 `None`（记成不全）。每个 pass 调一次。
    pub fn writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let mut used = self.used.lock().ok()?;
        let n = used.labels.len() as u32;
        if n >= MAX_PASSES {
            used.truncated = true;
            return None;
        }
        used.labels.push(self.label);
        Some(wgpu::RenderPassTimestampWrites { query_set: &self.set, beginning_of_pass_write_index: Some(n * 2), end_of_pass_write_index: Some(n * 2 + 1) })
    }
}

/// 一个 pass 的计时器；开 pass 时 `timestamp_writes: timer.as_ref().and_then(PassTimer::writes)`。
pub fn pass_timer(world: &mut World, label: &'static str) -> Option<PassTimer> {
    world.get_resource_mut::<GpuClock>()?.pass(label)
}

/// 起止时间戳 → 毫秒；倒着走的（换了时钟域、没写上）当 0。
pub fn span_ms(begin: u64, end: u64, period: f32) -> f32 {
    end.checked_sub(begin).map_or(0.0, |ticks| (ticks as f64 * f64::from(period) / 1e6) as f32)
}

/// 读回的时间戳按 pass 两两一组算毫秒。
pub fn times(stamps: &[u64], labels: &[&'static str], period: f32) -> GpuTimes {
    let passes: Vec<(&'static str, f32)> = labels.iter().zip(stamps.chunks_exact(2)).map(|(&l, s)| (l, span_ms(s[0], s[1], period))).collect();
    GpuTimes { total_ms: passes.iter().map(|p| p.1).sum(), passes, truncated: false }
}

/// 几个读回好的帧里挑最新的那一帧的下标。
pub fn latest(frames: &[(usize, u64)]) -> Option<usize> {
    frames.iter().max_by_key(|f| f.1).map(|f| f.0)
}

impl GpuClock {
    /// 设备支持时间戳。
    pub fn supported(&self) -> bool {
        self.clock.is_some()
    }

    /// 最近读回的一帧；不支持、还没读回时是 `None`。
    pub fn last(&self) -> Option<&GpuTimes> {
        self.last.as_ref()
    }

    /// 上次取以后新读回的一帧（总时间、是不是不全）；没有新的是 `None`（每帧取一次，读回的每帧只算一次）。
    pub fn take_fresh(&mut self) -> Option<(f32, bool)> {
        std::mem::take(&mut self.fresh).then(|| self.last.as_ref().map(|t| (t.total_ms, t.truncated))).flatten()
    }

    /// 帧开始：读回已经映射好的（几帧一起好了时取最新的），挑一个空的读回缓冲给这一帧。不阻塞。
    pub fn begin(&mut self, gfx: &Gfx) {
        if !std::mem::replace(&mut self.started, true) && gfx.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            self.clock = Some(Clock::new(gfx));
        }
        let Some(clock) = self.clock.as_mut() else { return };
        // 只轮询一下，回调里改状态
        let _ = gfx.device.poll(wgpu::PollType::Poll);
        let ready: Vec<(usize, u64)> = clock.slots.iter().enumerate().filter(|(_, s)| s.state.load(Ordering::Acquire) == READY).map(|(i, s)| (i, s.frame)).collect();
        let newest = latest(&ready);
        for (i, _) in ready {
            let slot = &clock.slots[i];
            let read = (Some(i) == newest)
                .then(|| slot.readback.get_mapped_range(..).ok().map(|data| data.chunks_exact(8).take(slot.labels.len() * 2).map(|b| u64::from_le_bytes(b.try_into().expect("8 字节"))).collect::<Vec<u64>>()))
                .flatten();
            slot.readback.unmap();
            if let Some(stamps) = read {
                self.last = Some(GpuTimes { truncated: slot.truncated, ..times(&stamps, &slot.labels, clock.period) });
                self.fresh = true;
            }
            slot.state.store(FREE, Ordering::Release);
        }
        clock.current = clock.slots.iter().position(|s| s.state.load(Ordering::Acquire) == FREE);
        if let Ok(mut used) = clock.used.lock() {
            *used = Used::default();
        }
    }

    /// 给一个 pass 一个计时器；不支持、这一帧不量时是 `None`。
    pub fn pass(&mut self, label: &'static str) -> Option<PassTimer> {
        let clock = self.clock.as_ref()?;
        clock.current?;
        Some(PassTimer { set: clock.set.clone(), used: clock.used.clone(), label })
    }

    /// 帧末：只解析这一帧真的写了的时间戳，拷进读回缓冲，提交以后自动映射。
    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(clock) = self.clock.as_mut() else { return };
        let Some(i) = clock.current.take() else { return };
        let Some(used) = clock.used.lock().ok().map(|mut u| std::mem::take(&mut *u)) else { return };
        clock.frame += 1;
        let count = used.labels.len() as u32 * 2;
        if count == 0 {
            return;
        }
        let slot = &mut clock.slots[i];
        (slot.labels, slot.truncated, slot.frame) = (used.labels, used.truncated, clock.frame);
        encoder.resolve_query_set(&clock.set, 0..count, &clock.resolve, 0);
        encoder.copy_buffer_to_buffer(&clock.resolve, 0, &slot.readback, 0, u64::from(count) * 8);
        slot.state.store(PENDING, Ordering::Release);
        let state = slot.state.clone();
        encoder.map_buffer_on_submit(&slot.readback, wgpu::MapMode::Read, .., move |r| state.store(if r.is_ok() { READY } else { FREE }, Ordering::Release));
    }
}

impl Clock {
    fn new(gfx: &Gfx) -> Self {
        let device = &gfx.device;
        let count = MAX_PASSES * 2;
        let size = u64::from(count) * 8;
        let set = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("gpu time"), ty: wgpu::QueryType::Timestamp, count });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu time resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = (0..SLOTS)
            .map(|_| Slot {
                readback: device.create_buffer(&wgpu::BufferDescriptor { label: Some("gpu time readback"), size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
                state: Arc::new(AtomicU8::new(FREE)),
                labels: Vec::new(),
                truncated: false,
                frame: 0,
            })
            .collect();
        Self { set, resolve, slots, period: gfx.queue.get_timestamp_period(), current: None, used: Arc::default(), frame: 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_turn_ticks_into_milliseconds_and_never_go_negative() {
        assert!((span_ms(1_000, 3_000_000, 1.0) - 2.999).abs() < 1e-4);
        assert!((span_ms(0, 1_000, 1000.0) - 1.0).abs() < 1e-6, "period 是每个 tick 几纳秒");
        assert_eq!(span_ms(5_000, 10, 1.0), 0.0, "倒着走的当 0");
    }

    #[test]
    fn times_pair_up_the_stamps_with_their_passes() {
        let t = times(&[0, 1_000_000, 2_000_000, 2_500_000], &["精灵", "文字"], 1.0);
        assert_eq!(t.passes, vec![("精灵", 1.0), ("文字", 0.5)]);
        assert!((t.total_ms - 1.5).abs() < 1e-6);
    }

    #[test]
    fn the_newest_of_several_read_backs_wins() {
        assert_eq!(latest(&[(0, 7), (1, 9), (2, 8)]), Some(1));
        assert_eq!(latest(&[]), None);
    }

    #[test]
    fn without_a_device_nothing_is_timed() {
        let mut clock = GpuClock::default();
        assert!(!clock.supported() && clock.pass("精灵").is_none() && clock.last().is_none());
        let mut world = World::new();
        assert!(pass_timer(&mut world, "精灵").is_none(), "没有资源也不崩");
    }
}
