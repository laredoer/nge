//! 序列帧（见 docs/particles3d-more.md）：贴图按格子切帧，按寿命播完或按固定 fps 循环。

/// 怎么播。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SheetMode {
    /// 一个寿命里播完一遍。
    #[default]
    Lifetime,
    /// 每秒这么多帧，循环。
    Fps(f32),
}

/// 序列帧贴图：从左上角按行切成 `columns × rows` 格。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteSheet {
    pub columns: u32,
    pub rows: u32,
    /// 用前几格；0 是全部。
    pub frames: u32,
    pub mode: SheetMode,
    /// 出生时随机挑起始帧。
    pub random_start: bool,
    /// 相邻两帧按进度混合。
    pub blend: bool,
}

impl Default for SpriteSheet {
    fn default() -> Self {
        Self { columns: 1, rows: 1, frames: 0, mode: SheetMode::Lifetime, random_start: false, blend: false }
    }
}

impl SpriteSheet {
    fn grid(&self) -> (u32, u32) {
        (self.columns.max(1), self.rows.max(1))
    }

    /// 帧数。
    pub fn count(&self) -> u32 {
        let (c, r) = self.grid();
        let cells = c.saturating_mul(r);
        if self.frames == 0 { cells } else { self.frames.min(cells) }
    }

    /// 第 `frame` 帧（按帧数取模）在贴图上的范围 `[u0, v0, u1, v1]`。
    pub fn uv(&self, frame: u32) -> [f32; 4] {
        let (c, r) = self.grid();
        let frame = frame % self.count();
        let (x, y) = ((frame % c) as f32, (frame / c) as f32);
        let (w, h) = (1.0 / c as f32, 1.0 / r as f32);
        [x * w, y * h, (x + 1.0) * w, (y + 1.0) * h]
    }

    /// 年龄 `age`、寿命 `lifetime`、起始帧 `start` 时：（这一帧，下一帧，混合比例）。
    pub(crate) fn pick(&self, age: f32, lifetime: f32, start: f32) -> (u32, u32, f32) {
        let n = self.count();
        let progress = match self.mode {
            SheetMode::Lifetime => (age / lifetime.max(f32::EPSILON)).clamp(0.0, 1.0) * n as f32,
            SheetMode::Fps(fps) => age.max(0.0) * if fps.is_finite() { fps.max(0.0) } else { 0.0 },
        };
        // 按寿命播、不随机起始时停在最后一帧，不绕回第一帧
        let once = self.mode == SheetMode::Lifetime && !self.random_start;
        let f = if once { progress.min(n as f32 - 1.0) } else { (start + progress).rem_euclid(n as f32) };
        let f = if f.is_finite() { f } else { 0.0 };
        let frame = (f.floor() as u32).min(n - 1);
        let next = if once { (frame + 1).min(n - 1) } else { (frame + 1) % n };
        let mix = if self.blend && next != frame { f - f.floor() } else { 0.0 };
        (frame, next, mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(mode: SheetMode, blend: bool) -> SpriteSheet {
        SpriteSheet { columns: 4, rows: 2, frames: 0, mode, random_start: false, blend }
    }

    #[test]
    fn cells_are_cut_row_by_row_from_the_top_left() {
        let s = sheet(SheetMode::Lifetime, false);
        assert_eq!(s.count(), 8);
        assert_eq!(s.uv(0), [0.0, 0.0, 0.25, 0.5]);
        assert_eq!(s.uv(5), [0.25, 0.5, 0.5, 1.0], "第 5 帧在第二行第二列");
        assert_eq!(s.uv(13), s.uv(5), "超出的按帧数取模");
        assert_eq!(SpriteSheet { frames: 6, ..s }.count(), 6);
        assert_eq!(SpriteSheet { frames: 99, ..s }.count(), 8, "多写的帧数截到格子数");
        let zero = SpriteSheet { columns: 0, rows: 0, ..s };
        assert_eq!((zero.count(), zero.uv(0)), (1, [0.0, 0.0, 1.0, 1.0]), "0 格当一格");
    }

    #[test]
    fn lifetime_mode_plays_through_once() {
        let s = sheet(SheetMode::Lifetime, true);
        assert_eq!(s.pick(0.0, 2.0, 0.0), (0, 1, 0.0));
        assert_eq!(s.pick(1.0, 2.0, 0.0), (4, 5, 0.0), "一半是第 4 帧");
        assert_eq!(s.pick(1.125, 2.0, 0.0), (4, 5, 0.5), "相邻两帧混合");
        assert_eq!(s.pick(1.99, 2.0, 0.0).0, 7);
        assert_eq!(s.pick(2.0, 2.0, 0.0), (7, 7, 0.0), "最后停在最后一帧，不绕回第一帧");
        assert_eq!(s.pick(1.99, 2.0, 0.0).1, 7, "最后一帧不往第一帧混");
        assert_eq!(sheet(SheetMode::Lifetime, false).pick(1.125, 2.0, 0.0), (4, 5, 0.0), "不混时比例是 0");
        // 随机起始：从第 6 帧开始，过一半寿命绕回来；抽到第 0 帧时也照样绕，最后一帧往第一帧混
        let random = SpriteSheet { random_start: true, ..s };
        assert_eq!(random.pick(1.0, 2.0, 6.0), (2, 3, 0.0));
        assert_eq!(random.pick(1.99, 2.0, 0.0).1, 0);
    }

    #[test]
    fn fps_mode_loops() {
        let s = sheet(SheetMode::Fps(10.0), true);
        assert_eq!(s.pick(0.35, 100.0, 0.0), (3, 4, 0.5));
        assert_eq!(s.pick(0.75, 100.0, 0.0), (7, 0, 0.5), "最后一帧往第一帧混");
        assert_eq!(s.pick(0.95, 100.0, 0.0), (1, 2, 0.5));
        assert_eq!(s.pick(0.0, 100.0, 3.0), (3, 4, 0.0));
        assert_eq!(sheet(SheetMode::Fps(f32::NAN), false).pick(1.0, 1.0, 0.0), (0, 1, 0.0), "NaN 的 fps 当 0");
    }
}
