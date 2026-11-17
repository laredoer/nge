//! Android 游戏视图里不碰 JNI 的部分：视口怎么算、引擎起来之前收到的消息（见 docs/android.md）。

use nge_core::{Msg, Viewport};

/// `SurfaceView` 的像素尺寸和密度（每 dp 多少像素）→ 视口；密度不对时按 1 算。
pub fn viewport(width: u32, height: u32, density: f32) -> Viewport {
    let density = if density.is_finite() && density > 0.0 { density } else { 1.0 };
    Viewport { size: [width as f32 / density, height as f32 / density], pixels: [width, height] }
}

/// 最多攒多少条（一直没有画面时不无限长）。
pub const MAX_PENDING: usize = 256;

/// 引擎起来（第一次有画面）之前收到的属性和 command，起来后按顺序交出去。和引擎里等 GPU 时一样，
/// 同名的 prop 只留最新一份（放在最后一次出现的位置）；超过 [`MAX_PENDING`] 条丢最早的。
#[derive(Default)]
pub struct Pending(Vec<Msg>);

impl Pending {
    pub fn push(&mut self, msg: Msg) {
        if let Msg::Prop(name, _) = &msg {
            self.0.retain(|m| !matches!(m, Msg::Prop(n, _) if n == name));
        }
        if self.0.len() >= MAX_PENDING {
            log::warn!("游戏视图还没有画面，攒的消息超过 {MAX_PENDING} 条，丢掉最早的");
            self.0.remove(0);
        }
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

    #[test]
    fn only_the_latest_value_of_a_prop_waits_and_commands_are_capped() {
        let mut pending = Pending::default();
        for i in 0..100 {
            pending.push(Msg::Prop("scene".into(), Some(json!(i))));
        }
        pending.push(Msg::Command("burst".into(), json!(0)));
        pending.push(Msg::Prop("scene".into(), Some(json!("last"))));
        let got: Vec<_> = pending
            .take()
            .into_iter()
            .map(|m| match m {
                Msg::Prop(n, v) => (n, v.unwrap_or_default()),
                Msg::Command(n, v) => (n, v),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(got, [("burst".to_owned(), json!(0)), ("scene".to_owned(), json!("last"))], "同名 prop 只留最新的，放在最后");
        for i in 0..MAX_PENDING + 10 {
            pending.push(Msg::Command("burst".into(), json!(i)));
        }
        let commands = pending.take();
        assert_eq!(commands.len(), MAX_PENDING, "最多攒这么多");
        assert!(matches!(&commands[0], Msg::Command(_, v) if *v == json!(10)), "丢最早的");
    }
}
