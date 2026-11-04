//! 和 GPU 无关的部分：锚点、颜色换算、文字框位置。

use glam::{Vec2, Vec4};

/// 位置对准文字框的哪一点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// 锚点在文字框里的比例位置（左上角 (0, 0)，y 朝下）。
    pub fn fraction(self) -> Vec2 {
        let (x, y) = match self {
            Self::TopLeft => (0.0, 0.0),
            Self::Top => (0.5, 0.0),
            Self::TopRight => (1.0, 0.0),
            Self::Left => (0.0, 0.5),
            Self::Center => (0.5, 0.5),
            Self::Right => (1.0, 0.5),
            Self::BottomLeft => (0.0, 1.0),
            Self::Bottom => (0.5, 1.0),
            Self::BottomRight => (1.0, 1.0),
        };
        Vec2::new(x, y)
    }
}

/// 文字框左上角（像素）：`at` 是锚点在视图里的像素位置，`size` 是排版后的框大小。
pub(crate) fn top_left(at: Vec2, size: Vec2, anchor: Anchor) -> Vec2 {
    at - size * anchor.fraction()
}

/// 线性 RGBA → sRGB 的 8 位颜色（glyphon 的颜色是 sRGB）。
pub(crate) fn to_srgb8(color: Vec4) -> [u8; 4] {
    let encode = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0 + 0.5) as u8
    };
    [encode(color.x), encode(color.y), encode(color.z), (color.w.clamp(0.0, 1.0) * 255.0 + 0.5) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_offset_the_box() {
        let (at, size) = (Vec2::new(100.0, 50.0), Vec2::new(40.0, 20.0));
        assert_eq!(top_left(at, size, Anchor::TopLeft), at);
        assert_eq!(top_left(at, size, Anchor::Center), Vec2::new(80.0, 40.0));
        assert_eq!(top_left(at, size, Anchor::BottomRight), Vec2::new(60.0, 30.0));
        assert_eq!(top_left(at, size, Anchor::Left), Vec2::new(100.0, 40.0));
    }

    #[test]
    fn linear_colors_become_srgb() {
        assert_eq!(to_srgb8(Vec4::new(1.0, 0.0, 0.2158, 0.5)), [255, 0, 128, 128]);
        assert_eq!(to_srgb8(Vec4::new(2.0, -1.0, 0.0, 1.0)), [255, 0, 0, 255], "超出范围的截断");
    }
}
