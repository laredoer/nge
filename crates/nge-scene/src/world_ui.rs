//! 世界里的 UI（见 docs/world-ui.md）：跟着精灵走的进度条 `bars`、带背景框的气泡 `bubbles`，气泡的字可以一个一个打出来。

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_core::EventKey;
use nge_ecs::{Outbox, Screen, Time};
use std::sync::LazyLock;

use nge_render2d::{Camera2d, DrawAt, Parallax, Sprite, Transform2d, Unlit, drawn};
use nge_render3d::{Camera3d, Transform3d};
use nge_text::{Anchor, ScreenText, Text2d, TextBox};
use serde::{Deserialize, Serialize};

use crate::SceneCamera;
use crate::nodes::ModelNode;
use crate::protocol::{Color, Scene, SpriteNode};
use crate::resolve::SceneAssets;
use crate::sync::{Declared, Node, SceneId, sync_list};

pub const TYPED: EventKey<Typed> = EventKey::new("typed");

/// 气泡的字打完了。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Typed {
    pub id: String,
}

/// 进度条（血条）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BarNode {
    pub id: String,
    /// 0 到 1。不写或 `null` 当 0：JS 里算出 NaN 时 JSON 里是 `null`，不能让整个场景解析失败。
    #[serde(default)]
    pub value: Option<f32>,
    /// 跟着哪个精灵或 3D 模型；x / y 变成相对它的偏移。
    pub follow: Option<String>,
    /// 跟随 3D 模型时在世界空间往上抬的高度。
    #[serde(default)]
    pub lift: f32,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default = "default_bar_w")]
    pub w: f32,
    #[serde(default = "default_bar_h")]
    pub h: f32,
    pub color: Option<Color>,
    pub back: Option<Color>,
    #[serde(default = "default_bar_z")]
    pub z: f32,
    /// 数值变化的过渡时间（秒）。
    #[serde(default = "default_smooth")]
    pub smooth: f32,
}

/// 带背景框的一段文字。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BubbleNode {
    pub id: String,
    pub text: String,
    pub follow: Option<String>,
    /// 跟随 3D 模型时在世界空间往上抬的高度。
    #[serde(default)]
    pub lift: f32,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    /// x / y 是屏幕坐标（左上角原点，y 朝下），不跟相机。
    #[serde(default)]
    pub screen: bool,
    #[serde(default = "default_bubble_size")]
    pub size: f32,
    pub color: Option<Color>,
    pub back: Option<Color>,
    #[serde(default = "default_padding")]
    pub padding: f32,
    #[serde(default = "default_max_width")]
    pub max_width: f32,
    /// 每秒出几个字，0 是一下全出来。
    #[serde(default)]
    pub typing: f32,
    pub font: Option<String>,
    #[serde(default = "default_bubble_z")]
    pub z: f32,
}

fn default_bar_w() -> f32 {
    32.0
}
fn default_bar_h() -> f32 {
    4.0
}
fn default_bar_z() -> f32 {
    50.0
}
fn default_smooth() -> f32 {
    0.15
}
fn default_bubble_size() -> f32 {
    14.0
}
fn default_padding() -> f32 {
    6.0
}
fn default_max_width() -> f32 {
    160.0
}
fn default_bubble_z() -> f32 {
    60.0
}

impl Node for BarNode {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Node for BubbleNode {
    fn id(&self) -> &str {
        &self.id
    }
}

/// 默认颜色：放置系统每帧都用，只解析一次。
static BAR_COLOR: LazyLock<Vec4> = LazyLock::new(|| hex("#4ade80"));
static BAR_BACK: LazyLock<Vec4> = LazyLock::new(|| hex("#000000a0"));
static BUBBLE_BACK: LazyLock<Vec4> = LazyLock::new(|| hex("#000000b0"));
/// 看不见的条和气泡挪到这里。
const OFFSCREEN: Vec2 = Vec2::splat(-1e6);
/// 填充画在底上面一点。
const FILL_ABOVE: f32 = 0.01;

/// 条和气泡的子精灵（填充、背景框）。
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct UiPart;

/// 条现在画到的值（往 `value` 过渡）和填充精灵。
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct BarState {
    shown: f32,
    fill: Entity,
}

/// 气泡的背景框精灵。
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct BubbleBack(Entity);

/// 打字进度：已经出了几个字（带小数）。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct Typing {
    cps: f32,
    chars: f32,
    total: usize,
    sent: bool,
}

/// 写了就用写的颜色，没写用默认的。
fn color_or(color: Option<Color>, default: &LazyLock<Vec4>) -> Vec4 {
    color.map_or(**default, |c| c.0)
}

fn hex(s: &str) -> Vec4 {
    Color::try_from(s.to_owned()).expect("默认颜色合法").0
}

/// 不是有限数的当 0，截到 0 到 1。
fn fraction(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 }
}

pub(crate) fn sync_bars(mut commands: Commands, scene: Res<Scene>, nodes: Query<(Entity, &Declared<BarNode>)>) {
    sync_list(&mut commands, &scene.bars, &nodes, |commands, entity, before, node| {
        let back = Sprite::color(Vec2::new(node.w.max(0.0), node.h.max(0.0)), color_or(node.back, &BAR_BACK));
        if before.is_none() {
            // 新建时直接是目标值，不从 0 涨上来
            let fill = commands.spawn((Sprite::color(Vec2::ZERO, color_or(node.color, &BAR_COLOR)), Transform2d::default(), UiPart, Unlit, ChildOf(entity))).id();
            commands.entity(entity).insert((Transform2d::from_xy(node.x, node.y).with_z(node.z), BarState { shown: fraction(node.value.unwrap_or(0.0)), fill }));
        }
        // 和字一样不受 2D 光照：夜里那关血条不变暗
        commands.entity(entity).insert((back, Unlit));
    });
}

pub(crate) fn sync_bubbles(mut commands: Commands, scene: Res<Scene>, mut assets: SceneAssets, nodes: Query<(Entity, &Declared<BubbleNode>)>) {
    sync_list(&mut commands, &scene.bubbles, &nodes, |commands, entity, before, node| {
        let text = Text2d {
            text: node.text.clone(),
            font: assets.font(node.font.as_deref()),
            size: node.size,
            color: node.color.map_or(Vec4::ONE, |c| c.0),
            // 世界里的气泡往上长，屏幕上的往下长
            anchor: if node.screen { Anchor::Top } else { Anchor::Bottom },
            max_width: Some(node.max_width.max(0.0)),
            visible: None,
        };
        let restart = before.is_none_or(|b| b.text != node.text || b.typing <= 0.0);
        let mut entity_commands = commands.entity(entity);
        entity_commands.remove::<ScreenText>();
        if node.screen {
            entity_commands.insert(ScreenText);
        }
        if before.is_none() {
            entity_commands.insert(Transform2d::from_xy(node.x, node.y));
        }
        let typing = Typing { cps: node.typing, chars: 0.0, total: node.text.chars().count(), sent: false };
        // 打到一半改成不打字（跳过）：算打完了，还没发过 `typed` 的照样发一次，靠它收尾的游戏不会卡住
        let skipped = before.is_some_and(|b| b.typing > 0.0 && b.text == node.text);
        match (node.typing > 0.0, restart) {
            (false, _) if skipped => {
                entity_commands.queue(move |mut e: EntityWorldMut| {
                    if let Some(mut t) = e.get_mut::<Typing>() {
                        (t.cps, t.chars) = (0.0, t.total as f32);
                    }
                    e.insert(text);
                });
            }
            (false, _) => {
                entity_commands.remove::<Typing>().insert(text);
            }
            (true, true) => {
                entity_commands.insert((Text2d { visible: Some(0), ..text }, typing));
            }
            // 只改了位置、颜色这些：接着打，换上新的速度
            (true, false) => {
                let cps = node.typing;
                entity_commands.queue(move |mut e: EntityWorldMut| {
                    let visible = e.get::<Typing>().map(|t| shown(t));
                    if let Some(mut t) = e.get_mut::<Typing>() {
                        t.cps = cps;
                    }
                    e.insert(Text2d { visible: visible.unwrap_or(Some(0)), ..text });
                });
            }
        }
        if before.is_none() {
            let back = commands.spawn((Sprite::color(Vec2::ZERO, Vec4::ZERO), Transform2d::default(), UiPart, Unlit, ChildOf(entity))).id();
            commands.entity(entity).insert(BubbleBack(back));
        }
        let color = color_or(node.back, &BUBBLE_BACK);
        commands.entity(entity).queue(move |e: EntityWorldMut| {
            let Some(&BubbleBack(back)) = e.get::<BubbleBack>() else { return };
            let world = e.into_world_mut();
            if let Some(mut sprite) = world.get_mut::<Sprite>(back) {
                sprite.color = color;
            }
        });
    });
}

/// 打出来的字数：`None` 是全出来了。
fn shown(t: &Typing) -> Option<usize> {
    let n = t.chars.floor() as usize;
    (n < t.total).then_some(n)
}

/// 打字：按游戏时间往前走，打完发一次 `typed`。
pub(crate) fn type_text(time: Res<Time>, mut texts: Query<(&SceneId, &mut Typing, &mut Text2d)>, mut outbox: ResMut<Outbox>) {
    for (id, mut typing, mut text) in &mut texts {
        typing.chars = (typing.chars + typing.cps * time.delta).min(typing.total as f32);
        let visible = shown(&typing);
        if text.visible != visible {
            text.visible = visible;
        }
        if visible.is_none() && !typing.sent {
            typing.sent = true;
            outbox.send(TYPED, &Typed { id: id.0.clone() });
        }
    }
}

type Followed<'w, 's> =
    Query<'w, 's, (&'static SceneId, &'static Transform2d, Option<&'static DrawAt>, Option<&'static Parallax>), (With<Declared<SpriteNode>>, Without<UiPart>)>;
type Followed3d<'w, 's> = Query<'w, 's, (&'static SceneId, &'static Transform3d), With<Declared<ModelNode>>>;

/// 先查 2D 精灵（含插值和视差），找不到再把 3D 模型（加 `lift` 抬高）投影到屏幕并换算到 2D 世界坐标。
fn follow_at(sprites: &Followed, models: &Followed3d, camera2d: &Camera2d, camera3d: Option<(&Camera3d, &Transform3d)>, screen: Vec2, id: &str, lift: f32) -> Option<Vec2> {
    if let Some(p) = sprites.iter().find(|(s, ..)| s.0 == id).map(|(_, t, d, p)| drawn(t, d, p, camera2d).translation) {
        return Some(p);
    }
    if screen.y <= 0.0 {
        return None;
    }
    let (cam3d, cam3d_t) = camera3d?;
    let (_, model_t) = models.iter().find(|(s, _)| s.0 == id)?;
    let vp = cam3d.view_proj(cam3d_t, screen.x / screen.y);
    let clip = vp * (model_t.translation + glam::Vec3::Y * lift).extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    let screen_pt = Vec2::new((ndc.x + 1.0) * 0.5 * screen.x, (1.0 - ndc.y) * 0.5 * screen.y);
    Some(camera2d.screen_to_world(screen, screen_pt))
}

/// 条的位置跟着精灵或 3D 模型，值往目标过渡，填充靠左。
pub(crate) fn place_bars(
    time: Res<Time>,
    screen: Res<Screen>,
    camera: Query<&Camera2d>,
    camera3d: Query<(&Camera3d, &Transform3d), With<SceneCamera>>,
    sprites: Followed,
    models: Followed3d,
    mut bars: Query<(&Declared<BarNode>, &mut BarState, &mut Transform2d), (Without<UiPart>, Without<Declared<SpriteNode>>)>,
    mut parts: Query<(&mut Sprite, &mut Transform2d), (With<UiPart>, Without<Declared<SpriteNode>>)>,
) {
    let camera = camera.iter().next().copied().unwrap_or_default();
    let cam3d = camera3d.iter().next();
    for (Declared(node), mut state, mut t) in &mut bars {
        let target = fraction(node.value.unwrap_or(0.0));
        let k = if node.smooth > 0.0 { 1.0 - (-time.delta / node.smooth).exp() } else { 1.0 };
        state.shown += (target - state.shown) * k;
        let offset = Vec2::new(node.x, node.y);
        let at = match &node.follow {
            Some(id) => follow_at(&sprites, &models, &camera, cam3d, screen.size, id, node.lift).map(|p| p + offset),
            None => Some(offset),
        }
        .unwrap_or(OFFSCREEN);
        (t.translation, t.z) = (at, node.z);
        if let Ok((mut sprite, mut fill)) = parts.get_mut(state.fill) {
            let (full, h) = (node.w.max(0.0), node.h.max(0.0));
            let w = full * state.shown;
            sprite.size = Some(Vec2::new(w, h));
            sprite.color = color_or(node.color, &BAR_COLOR);
            (fill.translation, fill.z) = (at + Vec2::new((w - full) / 2.0, 0.0), node.z + FILL_ABOVE);
        }
    }
}

/// 气泡：字放在跟随点（2D 精灵或 3D 模型投影）上方，背景框按量出来的文字框撑开。屏幕气泡的背景框按相机换到世界里。
pub(crate) fn place_bubbles(
    screen: Res<Screen>,
    camera: Query<&Camera2d>,
    camera3d: Query<(&Camera3d, &Transform3d), With<SceneCamera>>,
    sprites: Followed,
    models: Followed3d,
    mut bubbles: Query<(&Declared<BubbleNode>, &BubbleBack, Option<&TextBox>, &mut Transform2d), (Without<UiPart>, Without<Declared<SpriteNode>>)>,
    mut parts: Query<(&mut Sprite, &mut Transform2d), (With<UiPart>, Without<Declared<SpriteNode>>)>,
) {
    let camera = camera.iter().next().copied().unwrap_or_default();
    let cam3d = camera3d.iter().next();
    for (Declared(node), back, text_box, mut t) in &mut bubbles {
        let padding = node.padding.max(0.0);
        let frame = text_box.map_or(Vec2::ZERO, |b| b.0 + Vec2::splat(padding * 2.0));
        let offset = Vec2::new(node.x, node.y);
        let at = match (&node.follow, node.screen) {
            (_, true) => Some(offset),
            (Some(id), false) => follow_at(&sprites, &models, &camera, cam3d, screen.size, id, node.lift).map(|p| p + offset),
            (None, false) => Some(offset),
        };
        let Ok((mut sprite, mut back_t)) = parts.get_mut(back.0) else { continue };
        let Some(at) = at else {
            t.translation = OFFSCREEN;
            back_t.translation = OFFSCREEN;
            continue;
        };
        let (center, size, rotation) = if node.screen {
            // 屏幕坐标 y 朝下
            t.translation = at + Vec2::new(0.0, padding);
            let center = camera.screen_to_world(screen.size, at + Vec2::new(0.0, frame.y / 2.0));
            (center, frame / camera.zoom.max(1e-4), camera.rotation)
        } else {
            t.translation = at + Vec2::new(0.0, padding);
            (at + Vec2::new(0.0, frame.y / 2.0), frame, 0.0)
        };
        sprite.size = Some(size);
        (back_t.translation, back_t.z, back_t.rotation) = (center, node.z, rotation);
    }
}

#[cfg(test)]
mod tests {
    use nge_render2d::Camera2d;
    use serde_json::json;

    use super::*;
    use crate::testing::{VIEWPORT, app, frame};

    fn bar(app: &mut nge_ecs::App, id: &str) -> (Vec2, f32, Vec2, Vec2) {
        let world = app.world_mut();
        let (state, t) = world.query::<(&SceneId, &BarState, &Transform2d)>().iter(world).find(|(s, ..)| s.0 == id).map(|(_, b, t)| (*b, *t)).unwrap();
        let (fill, ft) = world.query::<(&Sprite, &Transform2d)>().get(world, state.fill).map(|(s, t)| (s.size.unwrap(), t.translation)).unwrap();
        (t.translation, state.shown, fill, ft)
    }

    fn tick(app: &mut nge_ecs::App, dt: f32) -> Vec<(String, serde_json::Value)> {
        app.tick(VIEWPORT, dt)
    }

    #[test]
    fn bars_follow_the_drawn_sprite_and_fill_from_the_left() {
        let mut app = app();
        let scene = |value: f32| {
            json!({
                "sprites": [{ "id": "p", "x": 10, "y": 20 }],
                "bars": [
                    { "id": "hp", "value": value, "follow": "p", "y": 30, "w": 40, "h": 6, "smooth": 0 },
                    { "id": "fixed", "value": 1, "x": 5, "y": 6 },
                    { "id": "lost", "value": 1, "follow": "nobody" },
                ],
            })
        };
        frame(&mut app, Some(scene(0.25)));
        let (at, shown, fill, fill_at) = bar(&mut app, "hp");
        assert_eq!((at, shown), (Vec2::new(10.0, 50.0), 0.25));
        assert_eq!(fill, Vec2::new(10.0, 6.0), "填充宽 = w × 值");
        assert_eq!(fill_at, Vec2::new(10.0 - 15.0, 50.0), "填充靠左：左边和底对齐");
        assert_eq!(bar(&mut app, "fixed").0, Vec2::new(5.0, 6.0), "不跟随时是世界坐标");
        assert!(bar(&mut app, "lost").0.x < -1e5, "找不到精灵就藏起来");

        let world = app.world_mut();
        let p = world.query::<(Entity, &SceneId)>().iter(world).find(|(_, s)| s.0 == "p").unwrap().0;
        world.entity_mut(p).insert(DrawAt { translation: Vec2::new(12.0, 21.0), rotation: 0.0 });
        tick(&mut app, 0.016);
        assert_eq!(bar(&mut app, "hp").0, Vec2::new(12.0, 51.0), "跟着画的位置（插值过的）");

        frame(&mut app, Some(json!({ "sprites": [{ "id": "p" }], "bars": [{ "id": "hp", "value": null, "follow": "p", "w": 40, "h": 6, "smooth": 0 }] })));
        let world = app.world_mut();
        world.entity_mut(p).remove::<DrawAt>();
        assert_eq!(bar(&mut app, "hp").2, Vec2::new(0.0, 6.0), "null（JS 的 NaN）当 0");
        assert_eq!(fraction(f32::NAN), 0.0);
        assert_eq!(fraction(-1.0), 0.0);
        frame(&mut app, Some(scene(3.0)));
        assert_eq!(bar(&mut app, "hp").1, 1.0, "截到 1");
    }

    #[test]
    fn bars_ease_toward_the_value_but_start_at_it() {
        let mut app = app();
        let scene = |value: f32, smooth: f32| json!({ "bars": [{ "id": "hp", "value": value, "smooth": smooth }] });
        frame(&mut app, Some(scene(0.8, 1.0)));
        assert_eq!(bar(&mut app, "hp").1, 0.8, "新建时直接是目标值");
        app.prop("scene", Some(&scene(0.2, 1.0)));
        tick(&mut app, 1.0);
        let shown = bar(&mut app, "hp").1;
        assert!((shown - (0.8 - 0.6 * (1.0 - (-1.0f32).exp()))).abs() < 1e-4, "按 smooth 过渡 {shown}");
        app.prop("scene", Some(&scene(0.2, 0.0)));
        tick(&mut app, 0.016);
        assert_eq!(bar(&mut app, "hp").1, 0.2, "smooth 0 立刻到");
    }

    #[test]
    fn defaults() {
        let b: BarNode = serde_json::from_value(json!({ "id": "hp" })).unwrap();
        assert_eq!((b.value, b.w, b.h, b.z, b.smooth, b.x, b.y), (None, 32.0, 4.0, 50.0, 0.15, 0.0, 0.0));
        let u: BubbleNode = serde_json::from_value(json!({ "id": "s", "text": "hi", "maxWidth": 90 })).unwrap();
        assert_eq!((u.size, u.padding, u.max_width, u.typing, u.z, u.screen), (14.0, 6.0, 90.0, 0.0, 60.0, false));
        assert!(serde_json::from_value::<BubbleNode>(json!({ "id": "s" })).is_err(), "text 必填");
        assert_eq!(color_or(None, &BAR_COLOR), Color::try_from("#4ade80".to_owned()).unwrap().0);
    }

    #[test]
    fn removing_a_bar_removes_its_fill() {
        let mut app = app();
        frame(&mut app, Some(json!({ "bars": [{ "id": "hp", "value": 1 }] })));
        let count = |app: &mut nge_ecs::App| app.world_mut().query::<&Sprite>().iter(app.world()).count();
        assert_eq!(count(&mut app), 2, "底和填充");
        let world = app.world_mut();
        assert_eq!(world.query_filtered::<&Sprite, With<Unlit>>().iter(world).count(), 2, "不受 2D 光照");
        frame(&mut app, Some(json!({ "bars": [] })));
        assert_eq!(count(&mut app), 0);
    }

    fn bubble(app: &mut nge_ecs::App, id: &str) -> (Text2d, Vec2, Vec2, Vec2) {
        let world = app.world_mut();
        let (text, t, back) =
            world.query::<(&SceneId, &Text2d, &Transform2d, &BubbleBack)>().iter(world).find(|(s, ..)| s.0 == id).map(|(_, x, t, b)| (x.clone(), t.translation, b.0)).unwrap();
        let (size, at) = world.query::<(&Sprite, &Transform2d)>().get(world, back).map(|(s, t)| (s.size.unwrap(), t.translation)).unwrap();
        (text, t, size, at)
    }

    fn measure(app: &mut nge_ecs::App, id: &str, size: Vec2) {
        let world = app.world_mut();
        let e = world.query::<(Entity, &SceneId, &Text2d)>().iter(world).find(|(_, s, _)| s.0 == id).unwrap().0;
        world.entity_mut(e).insert(TextBox(size));
    }

    #[test]
    fn bubbles_sit_on_the_sprite_and_the_frame_wraps_the_text() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "sprites": [{ "id": "p", "x": 10, "y": 20 }],
            "bubbles": [{ "id": "hi", "text": "你好", "follow": "p", "y": 16, "padding": 4, "back": "#ff0000" }, { "id": "lost", "text": "?", "follow": "nobody" }],
        })));
        let (text, at, size, _) = bubble(&mut app, "hi");
        assert_eq!((text.text.as_str(), text.anchor, text.max_width, text.visible), ("你好", Anchor::Bottom, Some(160.0), None));
        assert_eq!(at, Vec2::new(10.0, 40.0), "字的底边在跟随点上面 padding 处");
        assert_eq!(size, Vec2::ZERO, "还没量出来时没有背景框");
        measure(&mut app, "hi", Vec2::new(30.0, 12.0));
        tick(&mut app, 0.016);
        let (_, _, size, back_at) = bubble(&mut app, "hi");
        assert_eq!(size, Vec2::new(38.0, 20.0), "文字框加两倍 padding");
        assert_eq!(back_at, Vec2::new(10.0, 46.0), "背景框底边在跟随点上");
        let world = app.world_mut();
        let red = world.query_filtered::<&Sprite, With<UiPart>>().iter(world).any(|s| s.color.x == 1.0 && s.color.y == 0.0);
        assert!(red, "背景框用 back 的颜色");
        assert_eq!(world.query_filtered::<&Sprite, (With<UiPart>, With<Unlit>)>().iter(world).count(), 2, "背景框不受 2D 光照");
        let (_, lost, _, lost_back) = bubble(&mut app, "lost");
        assert!(lost.x < -1e5 && lost_back.x < -1e5, "找不到精灵就藏起来");
    }

    #[test]
    fn negative_sizes_are_clamped() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "bars": [{ "id": "hp", "value": 0.5, "w": -40, "h": -6 }],
            "bubbles": [{ "id": "say", "text": "hi", "padding": -5, "maxWidth": -1 }],
        })));
        measure(&mut app, "say", Vec2::new(30.0, 10.0));
        tick(&mut app, 0.016);
        let (_, _, fill, _) = bar(&mut app, "hp");
        assert_eq!(fill, Vec2::ZERO, "负的宽高当 0");
        let (text, at, size, _) = bubble(&mut app, "say");
        assert_eq!((text.max_width, at, size), (Some(0.0), Vec2::ZERO, Vec2::new(30.0, 10.0)), "负的 padding、maxWidth 当 0");
    }

    #[test]
    fn screen_bubbles_stay_on_screen() {
        let mut app = app();
        frame(&mut app, Some(json!({ "bubbles": [{ "id": "say", "text": "hi", "screen": true, "x": 50, "y": 70, "padding": 5 }] })));
        let world = app.world_mut();
        world.query::<&mut Camera2d>().single_mut(world).unwrap().position = Vec2::new(100.0, 200.0);
        world.query::<&mut Camera2d>().single_mut(world).unwrap().zoom = 2.0;
        measure(&mut app, "say", Vec2::new(40.0, 10.0));
        tick(&mut app, 0.016);
        let (text, at, size, back_at) = bubble(&mut app, "say");
        assert_eq!((text.anchor, at), (Anchor::Top, Vec2::new(50.0, 75.0)), "屏幕坐标，往下长");
        let world = app.world_mut();
        let e = world.query::<(Entity, &SceneId)>().iter(world).find(|(_, s)| s.0 == "say").unwrap().0;
        assert!(world.get::<ScreenText>(e).is_some());
        assert_eq!(size, Vec2::new(25.0, 10.0), "背景框是世界精灵，大小除以缩放");
        // 屏幕 (50, 80) 在 100×100 的视图里是中心往下 30，缩放 2 → 世界里往下 15
        assert_eq!(back_at, Vec2::new(100.0, 185.0), "背景框按相机换到世界里");
    }

    fn typed(events: &[(String, serde_json::Value)]) -> usize {
        events.iter().filter(|(name, v)| name == "typed" && v["id"] == "say").count()
    }

    #[test]
    fn typing_reveals_characters_and_reports_once() {
        let mut app = app();
        let scene = |text: &str, typing: f32, x: f32| json!({ "bubbles": [{ "id": "say", "text": text, "typing": typing, "x": x }] });
        let events = frame(&mut app, Some(scene("一二三四", 4.0, 0.0)));
        assert_eq!(bubble(&mut app, "say").0.visible, Some(2), "0.5 秒出 2 个字");
        assert_eq!(typed(&events), 0);
        let events = frame(&mut app, Some(scene("一二三四", 4.0, 9.0)));
        assert_eq!(bubble(&mut app, "say").0.visible, None, "只挪位置接着打，打完了");
        assert_eq!(typed(&events), 1);
        assert_eq!(typed(&frame(&mut app, None)), 0, "只发一次");

        let events = frame(&mut app, Some(scene("五六七八九十", 2.0, 9.0)));
        assert_eq!(bubble(&mut app, "say").0.visible, Some(1), "改了文字从头打");
        assert_eq!(typed(&events), 0);
        let events = frame(&mut app, Some(scene("五六七八九十", 0.0, 9.0)));
        assert_eq!(bubble(&mut app, "say").0.visible, None, "typing 改 0 一下全出来");
        assert_eq!(typed(&events), 1, "打到一半跳过算打完，发一次");
        assert_eq!(typed(&frame(&mut app, None)), 0);
        frame(&mut app, Some(scene("一", 0.0, 9.0)));
        let events = frame(&mut app, Some(scene("一", 0.0, 3.0)));
        assert_eq!(typed(&events), 0, "不打字时不发");
        frame(&mut app, Some(scene("一二三四", 2.0, 3.0)));
        let events = frame(&mut app, Some(scene("换了", 0.0, 3.0)));
        assert_eq!((bubble(&mut app, "say").0.visible, typed(&events)), (None, 0), "换了文字又不打字：是新的一段，不算打完旧的");
        frame(&mut app, Some(scene("五六七八九十", 2.0, 9.0)));
        assert_eq!(bubble(&mut app, "say").0.visible, Some(1), "从不打字改成打字，从头打");

        app.world_mut().resource_mut::<Time>().scale = 0.0;
        frame(&mut app, None);
        assert_eq!(bubble(&mut app, "say").0.visible, Some(1), "暂停时不走");
    }

    #[test]
    fn bars_and_bubbles_project_3d_models_and_hide_behind_camera() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "camera3d": { "x": 0, "y": 0, "z": 10 },
            "models": [
                { "id": "boss", "model": "asset:/@t/crate.glb", "x": 0, "y": 0, "z": 0 },
                { "id": "behind", "model": "asset:/@t/crate.glb", "x": 0, "y": 0, "z": 20 },
            ],
            "bars": [
                { "id": "hp", "follow": "boss", "lift": 2, "value": 0.75, "smooth": 0 },
                { "id": "back_hp", "follow": "behind", "value": 1, "smooth": 0 },
            ],
            "bubbles": [
                { "id": "say", "follow": "boss", "lift": 2, "text": "hello", "padding": 0 },
                { "id": "back_say", "follow": "behind", "text": "hidden" },
            ],
        })));
        tick(&mut app, 0.016);
        let (hp_pos, ..) = bar(&mut app, "hp");
        let (back_hp_pos, ..) = bar(&mut app, "back_hp");
        assert!(hp_pos.x.abs() < 1e-3 && hp_pos.y > 0.0, "3D 模型加 lift 后应投影到屏幕上方：{hp_pos:?}");
        assert!(back_hp_pos.x < -1e5, "相机背后的 3D 模型血条应隐藏：{back_hp_pos:?}");
        let (_, say_pos, _, _) = bubble(&mut app, "say");
        let (_, back_say_pos, _, _) = bubble(&mut app, "back_say");
        assert!(say_pos.x.abs() < 1e-3 && say_pos.y > 0.0, "3D 模型气泡应投影到屏幕上方：{say_pos:?}");
        assert!(back_say_pos.x < -1e5, "相机背后的 3D 模型气泡应隐藏：{back_say_pos:?}");
    }
}
