//! Android 游戏视图里不碰 JNI 的部分：视口怎么算、引擎起来之前收到的消息（见 docs/android.md）。

use nge_core::{Msg, Viewport};

/// `SurfaceView` 的像素尺寸和密度（每 dp 多少像素）→ 视口；密度不对时按 1 算。
pub fn viewport(width: u32, height: u32, density: f32) -> Viewport {
    let density = if density.is_finite() && density > 0.0 { density } else { 1.0 };
    Viewport { size: [width as f32 / density, height as f32 / density], pixels: [width, height] }
}

/// 引擎起来（第一次有画面）之前收到的属性和 command，起来后按顺序交出去。
#[derive(Default)]
pub struct Pending(Vec<Msg>);

impl Pending {
    pub fn push(&mut self, msg: Msg) {
        self.0.push(msg);
    }

    pub fn take(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn viewport_points_are_pixels_over_density() {
        assert_eq!(viewport(1080, 2400, 2.625), Viewport { size: [1080.0 / 2.625, 2400.0 / 2.625], pixels: [1080, 2400] });
        assert_eq!(viewport(800, 600, 0.0).size, [800.0, 600.0], "密度不对时按 1 算");
        assert_eq!(viewport(800, 600, f32::NAN).size, [800.0, 600.0]);
        assert_eq!(viewport(800, 600, f32::INFINITY).size, [800.0, 600.0]);
    }

    #[test]
    fn messages_before_the_engine_come_out_in_order() {
        let mut pending = Pending::default();
        pending.push(Msg::Prop("scene".into(), Some(json!(1))));
        pending.push(Msg::Command("burst".into(), json!({})));
        pending.push(Msg::Prop("captureKeys".into(), None));
        let names: Vec<_> = pending
            .take()
            .into_iter()
            .map(|m| match m {
                Msg::Prop(n, _) | Msg::Command(n, _) => n,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(names, ["scene", "burst", "captureKeys"]);
        assert!(pending.take().is_empty(), "交出去就没了");
    }
}
