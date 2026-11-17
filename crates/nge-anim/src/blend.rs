//! 混合树的权重（纯函数）。

/// 一维：`at` 是每个片段的位置（不用排好），`x` 在两个相邻的点之间线性分给这两个，超出两头的全给最近的那个。
/// 位置重复时给写在前面的；`x` 是 NaN 时当最小。返回和 `at` 一一对应的权重，加起来是 1。
pub fn blend1d(at: &[f32], x: f32) -> Vec<f32> {
    let mut w = vec![0.0; at.len()];
    let mut order: Vec<usize> = (0..at.len()).collect();
    // 稳定排序：位置一样的保持原来的先后
    order.sort_by(|&a, &b| at[a].total_cmp(&at[b]));
    let (Some(&first), Some(&last)) = (order.first(), order.last()) else { return w };
    if x.is_nan() || x <= at[first] {
        w[first] = 1.0;
        return w;
    }
    if x >= at[last] {
        w[last] = 1.0;
        return w;
    }
    for pair in order.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if x <= at[b] {
            let span = at[b] - at[a];
            let t = if span > 0.0 { (x - at[a]) / span } else { 1.0 };
            w[a] = 1.0 - t;
            w[b] = t;
            break;
        }
    }
    w
}

/// 二维：梯度带插值（freeform cartesian）：`w_i = min_{j≠i} clamp(1 − (p − p_i)·(p_j − p_i) / |p_j − p_i|², 0, 1)`，再归一化。
/// 正好在某个点上时那个是 1，加起来是 1；`p` 不是有限数时当原点。
pub fn blend2d(at: &[[f32; 2]], p: [f32; 2]) -> Vec<f32> {
    let p = if p[0].is_finite() && p[1].is_finite() { p } else { [0.0, 0.0] };
    let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
    let sub = |a: [f32; 2], b: [f32; 2]| [a[0] - b[0], a[1] - b[1]];
    let mut w: Vec<f32> = (0..at.len())
        .map(|i| {
            let mut wi = 1.0f32;
            for (j, pj) in at.iter().enumerate() {
                let ij = sub(*pj, at[i]);
                let len2 = dot(ij, ij);
                // 重复的点不互相压
                if j == i || len2 <= 0.0 {
                    continue;
                }
                wi = wi.min((1.0 - dot(sub(p, at[i]), ij) / len2).clamp(0.0, 1.0));
            }
            wi
        })
        .collect();
    let sum: f32 = w.iter().sum();
    if sum > 0.0 {
        w.iter_mut().for_each(|x| *x /= sum);
    } else if let Some(nearest) = (0..at.len()).min_by(|&a, &b| dot(sub(p, at[a]), sub(p, at[a])).total_cmp(&dot(sub(p, at[b]), sub(p, at[b])))) {
        w[nearest] = 1.0;
    }
    w
}
