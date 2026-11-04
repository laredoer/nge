//! 文字渲染器：每段文字缓存一个排好版的 glyphon `Buffer`，内容、字号、字体没变就不重排。

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, TryRecvError};

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
    /// 后台扫描中的系统字体；`None` 是已经并进字体库了。
    system_fonts: Option<Receiver<Vec<fontdb::FaceInfo>>>,
    shaped: HashMap<Entity, Shaped>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        // 扫描系统字体在 Mac 上要几百毫秒：一开始就放到后台线程，不卡渲染线程
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("nge-system-fonts".into()).spawn(move || {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            let _ = tx.send(db.faces().cloned().collect());
        });
        let system_fonts = match spawned {
            Ok(_) => Some(rx),
            Err(e) => {
                log::warn!("起不了扫描系统字体的线程，不指定字体的文字不会画出来: {e}");
                None
            }
        };
        Self {
            gpu: None,
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), fontdb::Database::new()),
            faces: HashMap::new(),
            system_fonts,
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

    /// 后台扫好的系统字体并进字体库（只读元数据，不重新解析），再排一段字预热：第一次用默认字体找字形要几十毫秒，在开头做掉。
    fn system_fonts_ready(&mut self) -> bool {
        let Some(rx) = &self.system_fonts else { return true };
        match rx.try_recv() {
            Ok(faces) => {
                let db = self.fonts.db_mut();
                for face in faces {
                    db.push_face_info(face);
                }
                shape(&mut self.fonts, Key { text: "0123456789+-".into(), face: None, px: 16.0, width: None });
            }
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => log::warn!("扫描系统字体失败，不指定字体的文字不会画出来"),
        }
        self.system_fonts = None;
        true
    }

    /// 排版并算出每段文字的位置；字体没加载完的跳过。删掉已经不存在的文字的缓存。
    fn layout(&mut self, world: &mut World, gfx: &Gfx) -> Vec<Placed> {
        // 每帧都看一眼：扫完就马上合并、预热，不等到第一段用默认字体的文字出现（那往往是游戏中途）
        let system_ready = self.system_fonts_ready();
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
                // 系统字体还在后台扫描：先不画，免得落到空字体库里排出一片空白
                None if !system_ready => continue,
                None => None,
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

    #[test]
    fn system_fonts_load_in_the_background() {
        let started = std::time::Instant::now();
        let mut renderer = TextRenderer::default();
        assert!(started.elapsed() < std::time::Duration::from_millis(50), "建渲染器不等扫描系统字体：{:?}", started.elapsed());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !renderer.system_fonts_ready() {
            assert!(std::time::Instant::now() < deadline, "系统字体一直没加载完");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        #[cfg(target_os = "macos")]
        {
            assert!(renderer.fonts.db().len() > 0, "扫到的系统字体并进了字体库");
            let shaped = shape(&mut renderer.fonts, Key { text: "+1".into(), face: None, px: 16.0, width: None });
            assert!(shaped.buffer.layout_runs().map(|run| run.glyphs.len()).sum::<usize>() > 0, "默认字体排得出字形");
        }
        assert!(renderer.system_fonts_ready(), "加载完以后一直是 ready");
    }
}
