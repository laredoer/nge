//! 文字渲染器：每段文字缓存一个排好版的 glyphon `Buffer`，内容、字号、字体没变就不重排。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use glam::Vec2;
use glyphon::fontdb;
use glyphon::{Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea, TextAtlas, TextBounds, Viewport};
use nge_assets::Assets;
use nge_core::{Gfx, wgpu};
use nge_ecs::{RenderFrame, Renderer};
use nge_render2d::{Camera2d, ClearColor, Transform2d};

use crate::layout::{to_srgb8, top_left};
use crate::{Font, FontId, ScreenText, Text2d};

/// 行高 = 字号 × 它。
const LINE_HEIGHT: f32 = 1.25;

struct Gpu {
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: glyphon::TextRenderer,
    swash: SwashCache,
}

/// 字体文件在字体库里的样子：同一个家族的不同字重靠后面三项区分。
#[derive(Debug, Clone, PartialEq)]
struct Face {
    family: String,
    weight: fontdb::Weight,
    style: fontdb::Style,
    stretch: fontdb::Stretch,
}

/// 排版用的参数，变了才重排。
#[derive(PartialEq)]
struct Key {
    text: String,
    face: Option<Face>,
    px: f32,
    width: Option<f32>,
}

impl Key {
    fn matches(&self, text: &str, face: Option<&Face>, px: f32, width: Option<f32>) -> bool {
        self.text == text && self.face.as_ref() == face && self.px == px && self.width == width
    }
}

struct Shaped {
    key: Key,
    buffer: Buffer,
    /// 排版后的框大小（像素）。
    size: Vec2,
}

pub(crate) struct TextRenderer {
    gpu: Option<Gpu>,
    fonts: FontSystem,
    /// 已注册进字体库的字体（解析失败的是 None）。
    faces: HashMap<FontId, Option<Face>>,
    system_fonts: bool,
    shaped: HashMap<Entity, Shaped>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        // 空字体库：系统字体等到真的用到时才扫描
        Self {
            gpu: None,
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), fontdb::Database::new()),
            faces: HashMap::new(),
            system_fonts: false,
            shaped: HashMap::new(),
        }
    }
}

/// 这一帧要画的一段文字。
struct Placed {
    entity: Entity,
    left: f32,
    top: f32,
    color: Color,
}

impl Renderer for TextRenderer {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        self.register_fonts(world.resource::<Assets<Font>>());
        let placed = self.layout(world, gfx);

        let gpu = self.gpu.get_or_insert_with(|| {
            let cache = Cache::new(&gfx.device);
            let mut atlas = TextAtlas::new(&gfx.device, &gfx.queue, &cache, gfx.format);
            let renderer = glyphon::TextRenderer::new(&mut atlas, &gfx.device, wgpu::MultisampleState::default(), None);
            Gpu { viewport: Viewport::new(&gfx.device, &cache), atlas, renderer, swash: SwashCache::new() }
        });
        let size = target.texture().size();
        gpu.viewport.update(&gfx.queue, Resolution { width: size.width, height: size.height });
        let bounds = TextBounds { left: 0, top: 0, right: size.width as i32, bottom: size.height as i32 };
        let areas = placed.iter().map(|p| TextArea {
            buffer: &self.shaped[&p.entity].buffer,
            left: p.left,
            top: p.top,
            scale: 1.0,
            bounds,
            default_color: p.color,
            custom_glyphs: &[],
        });
        if let Err(e) = gpu.renderer.prepare(&gfx.device, &gfx.queue, &mut self.fonts, &mut gpu.atlas, &gpu.viewport, areas, &mut gpu.swash) {
            log::warn!("文字排版失败: {e}");
            return;
        }

        let clear = *world.resource::<ClearColor>();
        let load = world.resource_mut::<RenderFrame>().color_load(clear);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("text"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        if let Err(e) = gpu.renderer.render(&gpu.atlas, &gpu.viewport, &mut pass) {
            log::warn!("文字绘制失败: {e}");
        }
        drop(pass);
        gpu.atlas.trim();
    }
}

impl TextRenderer {
    /// 把新加载好的字体注册进字体库。
    fn register_fonts(&mut self, fonts: &Assets<Font>) {
        for (id, font) in fonts.iter() {
            if self.faces.contains_key(&id) {
                continue;
            }
            let db = self.fonts.db_mut();
            let ids = db.load_font_source(fontdb::Source::Binary(font.0.clone()));
            let face = ids.first().and_then(|&face| db.face(face)).and_then(|f| {
                let (family, _) = f.families.first()?;
                Some(Face { family: family.clone(), weight: f.weight, style: f.style, stretch: f.stretch })
            });
            if face.is_none() {
                log::warn!("字体 {id:?} 解析失败，用它的文字不会画出来");
            }
            self.faces.insert(id, face);
        }
    }

    /// 排版并算出每段文字的位置；字体没加载完的跳过。删掉已经不存在的文字的缓存。
    fn layout(&mut self, world: &mut World, gfx: &Gfx) -> Vec<Placed> {
        let camera = world.query::<&Camera2d>().iter(world).next().copied().unwrap_or_default();
        let view = Vec2::from(gfx.viewport.size);
        let scale = gfx.viewport.scale();
        let mut seen = HashSet::new();
        let mut placed = Vec::new();
        let mut texts = world.query::<(Entity, &Text2d, &Transform2d, Has<ScreenText>)>();
        for (entity, text, transform, on_screen) in texts.iter(world) {
            let face = match text.font {
                Some(font) => match self.faces.get(&font) {
                    Some(Some(face)) => Some(face),
                    _ => continue,
                },
                None => {
                    if !std::mem::replace(&mut self.system_fonts, true) {
                        self.fonts.db_mut().load_system_fonts();
                    }
                    None
                }
            };
            let zoom = if on_screen { 1.0 } else { camera.zoom };
            let px = (text.size * zoom * scale).max(1.0);
            let width = text.max_width.map(|w| w * zoom * scale);
            let shaped = match self.shaped.get_mut(&entity) {
                Some(shaped) if shaped.key.matches(&text.text, face, px, width) => shaped,
                _ => {
                    let key = Key { text: text.text.clone(), face: face.cloned(), px, width };
                    let shaped = shape(&mut self.fonts, key);
                    self.shaped.insert(entity, shaped);
                    self.shaped.get_mut(&entity).expect("刚插入")
                }
            };
            let at = if on_screen { transform.translation } else { camera.world_to_screen(view, transform.translation) } * scale;
            let corner = top_left(at, shaped.size, text.anchor);
            let [r, g, b, a] = to_srgb8(text.color);
            placed.push(Placed { entity, left: corner.x, top: corner.y, color: Color::rgba(r, g, b, a) });
            seen.insert(entity);
        }
        self.shaped.retain(|entity, _| seen.contains(entity));
        placed
    }
}

fn shape(fonts: &mut FontSystem, key: Key) -> Shaped {
    let mut buffer = Buffer::new(fonts, Metrics::new(key.px, key.px * LINE_HEIGHT));
    buffer.set_size(key.width, None);
    let attrs = match &key.face {
        Some(face) => Attrs::new().family(Family::Name(&face.family)).weight(face.weight).style(face.style).stretch(face.stretch),
        None => Attrs::new().family(Family::SansSerif),
    };
    buffer.set_text(&key.text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fonts, false);
    let size = buffer.layout_runs().fold(Vec2::ZERO, |size, run| Vec2::new(size.x.max(run.line_w), size.y.max(run.line_top + run.line_height)));
    Shaped { key, buffer, size }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_remember_weight_so_families_can_share_a_name() {
        let mut fonts = Assets::<Font>::default();
        let bold = fonts.add(Font(std::sync::Arc::new(include_bytes!("../tests/data/Inter-Bold.ttf").to_vec())));
        let broken = fonts.add(Font(std::sync::Arc::new(b"true but not a font".to_vec())));
        let mut renderer = TextRenderer::default();
        renderer.register_fonts(&fonts);
        let face = renderer.faces[&bold].clone().unwrap();
        assert_eq!((face.family.as_str(), face.weight), ("Inter", fontdb::Weight::BOLD));
        assert_eq!(renderer.faces[&broken], None);
        let key = Key { text: "a".into(), face: Some(face.clone()), px: 10.0, width: None };
        let regular = Face { weight: fontdb::Weight::NORMAL, ..face };
        assert!(!key.matches("a", Some(&regular), 10.0, None), "同名不同字重要重排");
    }
}
