//! 3D 空间音频的纯函数：衰减、声像、锥形、多普勒、遮挡的平滑和节流（见 docs/audio-3d.md）。
//! 都不碰声卡，单独测。

use std::f32::consts::TAU;

use glam::Vec3;

/// 默认的最近、最远距离（point，一米 100）。
pub const DEFAULT_MIN: f32 = 100.0;
pub const DEFAULT_MAX: f32 = 2000.0;
/// 反比、指数衰减在最远距离前这一段（占最远距离的比例）里线性收到 0。
const EDGE: f32 = 0.1;
/// 多普勒变调的上下限。
pub const MIN_DOPPLER: f32 = 0.5;
pub const MAX_DOPPLER: f32 = 2.0;
/// 多普勒系数的上限（夸张多少倍）。
pub const MAX_DOPPLER_FACTOR: f32 = 100.0;
/// 速度分量最多算到声速的几倍：再快分母就到 0 了。
const MAX_MACH: f32 = 0.9;
/// 速度平滑的时间常数（秒）。
pub const VELOCITY_SMOOTHING: f32 = 0.1;
/// 全挡住时音量乘多少、低通压到多少赫兹；没挡时低通开到头。
pub const OCCLUDED_GAIN: f32 = 0.35;
pub const OCCLUDED_CUTOFF: f32 = 800.0;
pub const OPEN_CUTOFF: f32 = 20000.0;

/// 距离衰减的模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rolloff {
    /// `1 - (d - min) / (max - min)`。
    Linear,
    /// `min / (min + factor × (d - min))`。
    #[default]
    Inverse,
    /// `(d / min) ^ -factor`。
    Exponential,
}

/// 锥形指向：内锥里原音量，外锥外乘 `outer_gain`，中间线性过渡。角度是整个锥的张角（弧度）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cone {
    pub inner: f32,
    pub outer: f32,
    pub outer_gain: f32,
}

impl Cone {
    /// 角度截到 0 到一整圈（不是数的当一整圈，等于没有锥），外锥不比内锥小，`outer_gain` 截到 0 到 1。
    pub fn sane(self) -> Self {
        let angle = |a: f32| if a.is_nan() { TAU } else { a.clamp(0.0, TAU) };
        let inner = angle(self.inner);
        Self { inner, outer: angle(self.outer).max(inner), outer_gain: if self.outer_gain.is_nan() { 0.0 } else { self.outer_gain.clamp(0.0, 1.0) } }
    }
}

/// 一个带位置的声音怎么发声：衰减、锥形只对 3D 的起作用；多普勒、遮挡 2D、3D 都管用。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emit {
    pub rolloff: Rolloff,
    /// 这个距离以内不衰减。
    pub min: f32,
    /// 这个距离以外听不见。
    pub max: f32,
    /// 反比、指数衰减得多快，1 是照物理的。
    pub factor: f32,
    pub cone: Option<Cone>,
    /// 锥形朝哪（世界坐标）。
    pub forward: Vec3,
    /// 多普勒：0 关，1 真实，大于 1 夸张。
    pub doppler: f32,
    /// 查不查遮挡。
    pub occlude: bool,
}

impl Default for Emit {
    fn default() -> Self {
        Self { rolloff: Rolloff::Inverse, min: DEFAULT_MIN, max: DEFAULT_MAX, factor: 1.0, cone: None, forward: Vec3::NEG_Z, doppler: 0.0, occlude: false }
    }
}

impl Emit {
    /// 不合法的参数换成默认值。
    pub fn sane(self) -> Self {
        let (min, max, factor) = sane_range(self.min, self.max, self.factor);
        let doppler = if self.doppler.is_finite() { self.doppler.clamp(0.0, MAX_DOPPLER_FACTOR) } else { 0.0 };
        Self { min, max, factor, doppler, cone: self.cone.map(Cone::sane), ..self }
    }
}

/// `min` 要是正的有限数（不然用默认）；`max` 要比 `min` 大（不然按 `min` 的 20 倍）；`factor` 不能是负数。
fn sane_range(min: f32, max: f32, factor: f32) -> (f32, f32, f32) {
    let min = if min.is_finite() && min > 0.0 { min } else { DEFAULT_MIN };
    let max = if max.is_finite() && max > min { max } else { min * (DEFAULT_MAX / DEFAULT_MIN) };
    let factor = if factor.is_finite() && factor >= 0.0 { factor } else { 1.0 };
    (min, max, factor)
}

/// 距离衰减，0 到 1：`min` 以内是 1，`max` 以外是 0。
pub fn attenuation(rolloff: Rolloff, distance: f32, min: f32, max: f32, factor: f32) -> f32 {
    let (min, max, factor) = sane_range(min, max, factor);
    if distance.is_nan() || distance >= max {
        return 0.0;
    }
    if distance <= min {
        return 1.0;
    }
    // 反比和指数到最远距离还不是 0：最后一段线性收掉
    let edge = ((max - distance) / (max * EDGE)).clamp(0.0, 1.0);
    match rolloff {
        Rolloff::Linear => 1.0 - (distance - min) / (max - min),
        Rolloff::Inverse => min / (min + factor * (distance - min)) * edge,
        Rolloff::Exponential => (distance / min).powf(-factor) * edge,
    }
}

/// 左右声像（-1 左、1 右）：声源的方向在听的人右手方向上的投影。重合、坏输入时居中。
pub fn pan3(listener: Vec3, right: Vec3, source: Vec3) -> f32 {
    let (Some(dir), Some(right)) = ((source - listener).try_normalize(), right.try_normalize()) else { return 0.0 };
    let pan = dir.dot(right);
    if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 }
}

/// 锥形指向的音量倍数。重合、没有朝向时不衰减。
pub fn cone_gain(cone: Cone, forward: Vec3, source: Vec3, listener: Vec3) -> f32 {
    let cone = cone.sane();
    let (Some(to), Some(forward)) = ((listener - source).try_normalize(), forward.try_normalize()) else { return 1.0 };
    let angle = forward.dot(to).clamp(-1.0, 1.0).acos();
    let (inner, outer) = (cone.inner / 2.0, cone.outer / 2.0);
    if angle <= inner {
        1.0
    } else if angle >= outer {
        cone.outer_gain
    } else {
        1.0 + (cone.outer_gain - 1.0) * (angle - inner) / (outer - inner)
    }
}

/// 多普勒变调的倍数：`(c + 听的人朝声源的速度) / (c - 声源朝听的人的速度)`，速度先乘 `factor`、
/// 各截到 ±0.9 倍声速，结果截到 [`MIN_DOPPLER`] 到 [`MAX_DOPPLER`]。重合、坏输入时是 1。
pub fn doppler(source_velocity: Vec3, listener_velocity: Vec3, source: Vec3, listener: Vec3, speed_of_sound: f32, factor: f32) -> f32 {
    if !(speed_of_sound.is_finite() && speed_of_sound > 0.0 && factor.is_finite() && factor > 0.0) {
        return 1.0;
    }
    let Some(dir) = (listener - source).try_normalize() else { return 1.0 };
    let limit = speed_of_sound * MAX_MACH;
    let toward = |v: f32| (v * factor).clamp(-limit, limit);
    let (vs, vl) = (toward(source_velocity.dot(dir)), toward(-listener_velocity.dot(dir)));
    let ratio = (speed_of_sound + vl) / (speed_of_sound - vs);
    if ratio.is_finite() { ratio.clamp(MIN_DOPPLER, MAX_DOPPLER) } else { 1.0 }
}

/// 两帧的位置差算速度。比 `limit` 快的当瞬移（速度按 0）；暂停（`dt` 不是正数）、坏输入也是 0。
pub fn velocity(previous: Vec3, now: Vec3, dt: f32, limit: f32) -> Vec3 {
    if !(dt > 0.0) {
        return Vec3::ZERO;
    }
    let v = (now - previous) / dt;
    if v.is_finite() && v.length() <= limit { v } else { Vec3::ZERO }
}

/// 速度的平滑：按 [`VELOCITY_SMOOTHING`] 的时间常数靠近新的速度。
pub fn smooth(current: Vec3, target: Vec3, dt: f32) -> Vec3 {
    if !(dt > 0.0) {
        return current;
    }
    current + (target - current) * (1.0 - (-dt / VELOCITY_SMOOTHING).exp())
}

/// 按「`seconds` 秒走完 0 到 1」的速度靠近目标，不冲过头。
pub fn approach(current: f32, target: f32, dt: f32, seconds: f32) -> f32 {
    if !(dt >= 0.0) {
        return current;
    }
    if !(seconds > 0.0) {
        return target;
    }
    let step = dt / seconds;
    if (target - current).abs() <= step { target } else { current + step * (target - current).signum() }
}

/// 遮挡量（0 没挡、1 全挡）→ 音量倍数和低通的截止频率：音量线性，截止频率按对数。
pub fn occlusion_mix(amount: f32) -> (f32, f32) {
    let t = if amount.is_nan() { 0.0 } else { amount.clamp(0.0, 1.0) };
    (1.0 + (OCCLUDED_GAIN - 1.0) * t, OPEN_CUTOFF * (OCCLUDED_CUTOFF / OPEN_CUTOFF).powf(t))
}

/// 遮挡射线的节流：`ages` 是每个声音离上一次查过了多久；返回这一帧查哪几个的下标——
/// 至少隔了 `interval` 的里面等得最久的先查，最多 `budget` 个。
pub fn pick_probes(ages: &[f32], budget: usize, interval: f32) -> Vec<usize> {
    let mut due: Vec<usize> = (0..ages.len()).filter(|&i| ages[i] >= interval).collect();
    due.sort_by(|&a, &b| ages[b].total_cmp(&ages[a]).then(a.cmp(&b)));
    due.truncate(budget);
    due
}

/// 一个 3D 声音的音量和声像：音量乘距离衰减和锥形，声像加上按听的人右手方向算的左右。
pub fn mix3(volume: f32, pan: f32, at: Vec3, emit: &Emit, listener: Vec3, right: Vec3) -> (f32, f32) {
    let pan = if pan.is_nan() { 0.0 } else { pan };
    if !at.is_finite() || !listener.is_finite() {
        return (0.0, pan);
    }
    let mut gain = attenuation(emit.rolloff, at.distance(listener), emit.min, emit.max, emit.factor);
    if let Some(cone) = emit.cone {
        gain *= cone_gain(cone, emit.forward, at, listener);
    }
    (volume * gain, (pan + pan3(listener, right, at)).clamp(-1.0, 1.0))
}
