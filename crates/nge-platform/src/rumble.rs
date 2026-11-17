//! 手柄震动里不碰平台的部分：引擎发来的 `nge.rumble` 截下来排队、震哪几个手柄、强度和时长怎么换成各平台的参数（见 docs/input-actions.md）。

use std::sync::{Arc, Mutex};

use nge_core::Emit;
use nge_core::bridge::{RUMBLE_OUT, RumbleOut};

/// 没人取时最多排这么多条。
pub const MAX_QUEUED: usize = 32;
/// 一次最长这么久（毫秒），和引擎那边的上限一样。
const MAX_MILLIS: f32 = 5000.0;

/// 引擎（渲染线程）要震的，等主线程的每帧回调来取。
#[derive(Clone, Default)]
pub struct Queue(Arc<Mutex<Vec<RumbleOut>>>);

impl Queue {
    /// 包住发给 NSH 的 `emit`：`nge.rumble` 截下来排队，别的照常发。
    pub fn wrap(&self, emit: Emit) -> Emit {
        let queue = self.0.clone();
        Box::new(move |name, data| {
            if name != RUMBLE_OUT.name() {
                return emit(name, data);
            }
            match RUMBLE_OUT.decode(&data) {
                Ok(rumble) => {
                    let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
                    if queue.len() < MAX_QUEUED {
                        queue.push(rumble);
                    }
                }
                Err(e) => log::warn!("{e}"),
            }
        })
    }

    pub fn take(&self) -> Vec<RumbleOut> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// 要震的手柄：指定的那个（还连着的话），没指定就是全部。
pub fn targets(pad: Option<u32>, connected: &[u32]) -> Vec<u32> {
    connected.iter().copied().filter(|id| pad.is_none_or(|pad| pad == *id)).collect()
}

/// 强度限制在 0 … 1，不是有限数的当 0。
pub fn unit(strength: f32) -> f32 {
    if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 0.0 }
}

/// 只有一个马达时：两个强度里大的。
pub fn single(low: f32, high: f32) -> f32 {
    unit(low).max(unit(high))
}

/// Android 的振幅：1 … 255，0 是不震。
pub fn amplitude(strength: f32) -> i32 {
    (unit(strength) * 255.0).ceil() as i32
}

/// 时长换成毫秒，最多 5 秒。
pub fn millis(seconds: f32) -> u32 {
    if seconds.is_finite() { (seconds * 1000.0).clamp(0.0, MAX_MILLIS).round() as u32 } else { 0 }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    #[test]
    fn rumble_events_are_taken_out_and_the_rest_passed_on() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let queue = Queue::default();
        let sink = seen.clone();
        let emit = queue.wrap(Box::new(move |name: &str, data| sink.lock().unwrap().push((name.to_owned(), data))));
        emit("fps", json!({ "fps": 60 }));
        emit("nge.rumble", json!({ "pad": 1, "low": 0.5, "high": 0.25, "duration": 0.5 }));
        emit("nge.rumble", json!("坏的"));
        assert_eq!(*seen.lock().unwrap(), [("fps".to_owned(), json!({ "fps": 60 }))], "震动不交给 NSH");
        assert_eq!(queue.take(), [RumbleOut { pad: Some(1), low: 0.5, high: 0.25, duration: 0.5 }]);
        assert!(queue.take().is_empty(), "取走就没了");
    }

    #[test]
    fn the_queue_does_not_grow_without_bound() {
        let queue = Queue::default();
        let emit = queue.wrap(Box::new(|_: &str, _| {}));
        for _ in 0..MAX_QUEUED + 10 {
            emit("nge.rumble", json!({ "pad": null, "low": 1.0, "high": 1.0, "duration": 1.0 }));
        }
        assert_eq!(queue.take().len(), MAX_QUEUED, "没人取（没有每帧回调）时不无限长");
    }

    #[test]
    fn targets_are_one_pad_or_all_of_them() {
        assert_eq!(targets(None, &[2, 5]), [2, 5]);
        assert_eq!(targets(Some(5), &[2, 5]), [5]);
        assert!(targets(Some(7), &[2, 5]).is_empty(), "拔掉了的不震");
        assert!(targets(None, &[]).is_empty());
    }

    #[test]
    fn strength_and_time_in_platform_units() {
        assert_eq!([0.0, 0.001, 0.5, 1.0, 3.0, -1.0, f32::NAN].map(amplitude), [0, 1, 128, 255, 255, 0, 0], "Android 的振幅 1 … 255，0 是不震");
        assert_eq!([0.25, 0.0, -1.0, 100.0, f32::NAN].map(millis), [250, 0, 0, 5000, 0], "最多 5 秒");
        assert_eq!(single(0.25, 0.75), 0.75);
        assert_eq!(single(f32::NAN, 0.5), 0.5, "只有一个马达时取大的");
        assert_eq!(unit(2.0), 1.0);
    }
}
