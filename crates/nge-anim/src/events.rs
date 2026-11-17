//! 一段时间里经过了哪些事件（纯函数）。

/// 一步最多算几圈：跳帧跨了更多圈时多出来的不发。
pub const MAX_LOOPS: f64 = 8.0;

/// 片段时间从 `a` 走到 `b`（秒，循环的不取模），经过了哪些事件（`events` 的下标，按经过的先后，每圈各算一次）。
/// - 正放：`a < t + k·dur ≤ b`；倒放：`b ≤ t + k·dur < a`。`fresh`（刚进入状态的第一步）时把起点 `a` 也算上。
/// - 不循环的只有 `k = 0`；`dur` 不是正数时只在 `fresh` 时发时间 0 的事件。
pub fn crossed(events: &[f32], dur: f32, looping: bool, a: f64, b: f64, fresh: bool) -> Vec<usize> {
    if !(dur > 0.0 && a.is_finite() && b.is_finite()) {
        return if fresh { (0..events.len()).filter(|&i| events[i] <= 0.0).collect() } else { Vec::new() };
    }
    let d = f64::from(dur);
    let forward = b >= a;
    let (mut lo, mut hi) = if forward { (a, b) } else { (b, a) };
    if looping && hi - lo > MAX_LOOPS * d {
        // 只留最后经过的几圈
        if forward {
            lo = hi - MAX_LOOPS * d;
        } else {
            hi = lo + MAX_LOOPS * d;
        }
    }
    let inside = |x: f64| {
        if fresh && x == a {
            return true;
        }
        if forward { lo < x && x <= hi } else { lo <= x && x < hi }
    };
    let mut hits: Vec<(f64, usize)> = Vec::new();
    for (i, &t) in events.iter().enumerate() {
        let t = f64::from(t).clamp(0.0, d);
        if !looping {
            if inside(t) {
                hits.push((t, i));
            }
            continue;
        }
        let first = ((lo - t) / d).floor() as i64 - 1;
        let last = ((hi - t) / d).ceil() as i64 + 1;
        for k in first..=last {
            let x = t + k as f64 * d;
            if inside(x) {
                hits.push((x, i));
            }
        }
    }
    // 按经过的先后；同一时刻的按事件的先后
    hits.sort_by(|p, q| if forward { p.0.total_cmp(&q.0) } else { q.0.total_cmp(&p.0) }.then(p.1.cmp(&q.1)));
    hits.into_iter().map(|(_, i)| i).collect()
}
