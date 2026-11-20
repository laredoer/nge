//! 瓦片层：一张瓦片地图按块缓存实例，渲染时按相机视野裁剪（见 docs/big-scene-perf.md）。

use bevy_ecs::prelude::Component;
use glam::{Vec2, Vec4};

use crate::assets::TextureId;
use crate::batch::{Instance, LIT};
use crate::camera::Camera2d;
use crate::sprite::SpriteImage;

/// 块的默认边长（格）。
pub const DEFAULT_CHUNK: usize = 32;
/// 一块连续这么多帧不可见就放掉缓存。
pub const KEEP_FRAMES: u32 = 120;

/// 一个视野：相机和视口大小（point）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View2d {
    pub camera: Camera2d,
    pub size: Vec2,
}

impl View2d {
    pub fn new(camera: Camera2d, size: Vec2) -> Self {
        Self { camera, size }
    }

    /// 视野的半宽高（世界坐标，加上外扩）和两个轴。
    fn frame(&self, margin: f32) -> (Vec2, Vec2, Vec2) {
        let half = self.size.max(Vec2::ONE) / (2.0 * self.camera.zoom.max(1e-4)) + Vec2::splat(margin.max(0.0));
        let u = Vec2::from_angle(self.camera.rotation);
        (half, u, u.perp())
    }

    /// 视野（外扩 `margin`）的外接框 (min, max)。
    pub fn bounds(&self, margin: f32) -> (Vec2, Vec2) {
        let (half, u, v) = self.frame(margin);
        let extent = Vec2::new(u.x.abs() * half.x + v.x.abs() * half.y, u.y.abs() * half.x + v.y.abs() * half.y);
        (self.camera.position - extent, self.camera.position + extent)
    }

    /// 轴对齐的框 (min, max) 和视野（外扩 `margin`）相交：分离轴测试，转着的相机不算外接框的角。
    pub fn overlaps(&self, lo: Vec2, hi: Vec2, margin: f32) -> bool {
        if !(lo.cmple(hi).all() && lo.is_finite() && hi.is_finite()) {
            return false;
        }
        let (half, u, v) = self.frame(margin);
        let (center, box_half) = ((lo + hi) / 2.0, (hi - lo) / 2.0);
        let d = center - self.camera.position;
        let view_x = u.x.abs() * half.x + v.x.abs() * half.y;
        let view_y = u.y.abs() * half.x + v.y.abs() * half.y;
        let on = |axis: Vec2| axis.x.abs() * box_half.x + axis.y.abs() * box_half.y;
        d.x.abs() <= box_half.x + view_x && d.y.abs() <= box_half.y + view_y && d.dot(u).abs() <= half.x + on(u) && d.dot(v).abs() <= half.y + on(v)
    }
}

/// 视差偏移：和 [`crate::drawn`] 一样，坏的视差当 1。
pub(crate) fn parallax_offset(parallax: Option<Vec2>, camera: &Camera2d) -> Vec2 {
    parallax.filter(|p| p.is_finite()).map_or(Vec2::ZERO, |p| camera.position * (Vec2::ONE - p))
}

/// 图集换成第 `frame` 帧；单张贴图不变。
pub(crate) fn frame_image(image: SpriteImage, frame: u32) -> SpriteImage {
    match image {
        SpriteImage::Atlas(atlas, _) => SpriteImage::Atlas(atlas, frame),
        texture => texture,
    }
}

/// 动画瓦片：帧号是 `frame` 的格子按 `frames` 以 `fps` 循环。
#[derive(Debug, Clone, PartialEq)]
pub struct TileAnim {
    pub frame: i32,
    pub frames: Vec<u32>,
    pub fps: f32,
}

impl TileAnim {
    /// 场景时钟第 `t` 秒画哪一帧。
    pub fn at(&self, t: f64) -> u32 {
        if self.frames.is_empty() {
            return self.frame.max(0) as u32;
        }
        let i = (t * f64::from(self.fps)).floor();
        let i = if i.is_finite() { i.rem_euclid(self.frames.len() as f64) as usize } else { 0 };
        self.frames[i.min(self.frames.len() - 1)]
    }
}

/// 一块的缓存。
#[derive(Debug, Default)]
struct Chunk {
    /// 改过格子，缓存要重建。
    dirty: bool,
    /// 缓存按哪张贴图建的；`None` 是还没建。
    texture: Option<TextureId>,
    instances: Vec<Instance>,
    /// 每行（块里的行）的实例从哪开始，最后多一个结尾。
    rows: Vec<u32>,
    /// 每个实例在块里的列。
    cols: Vec<u16>,
    /// 动画格子：（实例下标，第几种动画）。
    anims: Vec<(u32, u16)>,
    /// 每种动画在这块里现在画的帧。
    shown: Vec<u32>,
    /// 连续几帧没看见。
    unseen: u32,
}

/// 这一帧画了几块、一共几块、拷了几格。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileDrawStats {
    pub shown: u32,
    pub total: u32,
    pub tiles: u32,
}

/// 组件：一张瓦片地图。格子一行一行从上往下，负数是空格；`origin` 是左上角（y 朝上，行往下排）。
#[derive(Component, Debug)]
pub struct TileLayer {
    image: SpriteImage,
    origin: Vec2,
    tile: f32,
    cols: usize,
    rows: usize,
    chunk: usize,
    pub z: f32,
    /// 视差（见 docs/parallax.md）。
    pub parallax: Option<Vec2>,
    frames: Vec<i32>,
    anims: Vec<TileAnim>,
    chunks: Vec<Chunk>,
    /// 有缓存的块。
    live: Vec<usize>,
    /// 这一帧的动画帧（复用）。
    current: Vec<u32>,
    rebuilt: u64,
    patched: u64,
}

impl TileLayer {
    /// `frames` 不满一行的丢掉；`chunk` 至少 1。图集的帧号在画的时候换（`image` 是图集时帧号不用管）。
    pub fn new(image: SpriteImage, origin: Vec2, tile: f32, cols: usize, mut frames: Vec<i32>, chunk: usize, anims: Vec<TileAnim>, z: f32) -> Self {
        let rows = frames.len().checked_div(cols).unwrap_or(0);
        frames.truncate(rows * cols);
        let chunk = chunk.max(1);
        let count = cols.div_ceil(chunk) * rows.div_ceil(chunk);
        let chunks = (0..count).map(|_| Chunk { dirty: true, ..Default::default() }).collect();
        Self { image, origin, tile, cols, rows, chunk, z, parallax: None, frames, anims, chunks, live: Vec::new(), current: Vec::new(), rebuilt: 0, patched: 0 }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn chunk(&self) -> usize {
        self.chunk
    }

    pub fn origin(&self) -> Vec2 {
        self.origin
    }

    pub fn tile(&self) -> f32 {
        self.tile
    }

    pub fn frames(&self) -> &[i32] {
        &self.frames
    }

    /// 第 `frame` 帧画哪张图。
    pub fn image_for(&self, frame: u32) -> SpriteImage {
        frame_image(self.image, frame)
    }

    /// 块的个数。
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn chunk_cols(&self) -> usize {
        self.cols.div_ceil(self.chunk)
    }

    /// 格子 (列, 行) 的帧号；地图外是 `None`。
    pub fn frame(&self, col: usize, row: usize) -> Option<i32> {
        (col < self.cols && row < self.rows).then(|| self.frames[row * self.cols + col])
    }

    /// 格子在场景时钟第 `t` 秒画哪一帧（动画格子按动画走）；空格、地图外是 `None`。
    pub fn drawn_frame(&self, col: usize, row: usize, t: f64) -> Option<u32> {
        let frame = u32::try_from(self.frame(col, row)?).ok()?;
        Some(self.anims.iter().find(|a| a.frame == frame as i32).map_or(frame, |a| a.at(t)))
    }

    /// 格子在哪一块。
    pub fn chunk_of(&self, col: usize, row: usize) -> usize {
        (row / self.chunk) * self.chunk_cols() + col / self.chunk
    }

    /// 第 `k` 块管的列、行范围。
    pub fn chunk_cells(&self, k: usize) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let (bx, by) = (k % self.chunk_cols(), k / self.chunk_cols());
        let (c0, r0) = (bx * self.chunk, by * self.chunk);
        (c0..(c0 + self.chunk).min(self.cols), r0..(r0 + self.chunk).min(self.rows))
    }

    /// 改一格：变了时标脏它那一块，返回块号；地图外、没变时 `None`。
    pub fn set(&mut self, col: usize, row: usize, frame: i32) -> Option<usize> {
        let old = self.frame(col, row)?;
        if old == frame {
            return None;
        }
        self.frames[row * self.cols + col] = frame;
        let k = self.chunk_of(col, row);
        self.chunks[k].dirty = true;
        Some(k)
    }

    /// 一共重建过几次块的缓存。
    pub fn rebuilt(&self) -> u64 {
        self.rebuilt
    }

    /// 一共改过几个动画格子的 uv。
    pub fn patched(&self) -> u64 {
        self.patched
    }

    /// 现在有缓存的块。
    pub fn cached(&self) -> usize {
        self.live.len()
    }

    /// 第 `k` 块在世界里的框（不含视差偏移）。
    fn chunk_box(&self, k: usize) -> (Vec2, Vec2) {
        let (cols, rows) = self.chunk_cells(k);
        let lo = self.origin + Vec2::new(cols.start as f32 * self.tile, -(rows.end as f32) * self.tile);
        let hi = self.origin + Vec2::new(cols.end as f32 * self.tile, -(rows.start as f32) * self.tile);
        (lo, hi)
    }

    /// 视野外接框（外扩 `margin`，减掉视差偏移）盖住的列、行范围。
    fn cell_range(&self, view: &View2d, offset: Vec2, margin: f32) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let (lo, hi) = view.bounds(margin);
        let (lo, hi) = (lo - offset - self.origin, hi - offset - self.origin);
        let clamp = |v: f32, n: usize| if v.is_finite() { v.clamp(0.0, n as f32) as usize } else { 0 };
        let cols = clamp((lo.x / self.tile).floor(), self.cols)..clamp((hi.x / self.tile).ceil(), self.cols);
        let rows = clamp((-hi.y / self.tile).floor(), self.rows)..clamp((-lo.y / self.tile).ceil(), self.rows);
        (cols, rows)
    }

    /// 和这个视野相交的块，按块号升序追加到 `out`。
    fn visible_in(&self, view: &View2d, parallax: Option<Vec2>, out: &mut Vec<usize>) {
        if self.cols == 0 || !(self.tile > 0.0) {
            return;
        }
        let offset = parallax_offset(parallax, &view.camera);
        let margin = self.tile;
        let (cols, rows) = self.cell_range(view, offset, margin);
        if cols.is_empty() || rows.is_empty() {
            return;
        }
        let per_row = self.chunk_cols();
        for by in rows.start / self.chunk..rows.end.div_ceil(self.chunk) {
            for bx in cols.start / self.chunk..cols.end.div_ceil(self.chunk) {
                let k = by * per_row + bx;
                let (lo, hi) = self.chunk_box(k);
                if view.overlaps(lo + offset, hi + offset, margin) {
                    out.push(k);
                }
            }
        }
    }

    /// 在任意一个视野里看得见的块（外扩一格），升序、不重复。
    pub fn visible(&self, views: &[View2d], parallax: Option<Vec2>) -> Vec<usize> {
        let mut out = Vec::new();
        for view in views {
            self.visible_in(view, parallax, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 重建第 `k` 块的缓存。
    fn build(&mut self, k: usize, texture: TextureId, region: &dyn Fn(u32) -> (TextureId, Vec4)) {
        let (cols, rows) = self.chunk_cells(k);
        let (origin, tile, z) = (self.origin, self.tile, self.z);
        let chunk = &mut self.chunks[k];
        chunk.instances.clear();
        chunk.rows.clear();
        chunk.cols.clear();
        chunk.anims.clear();
        for r in rows.clone() {
            chunk.rows.push(chunk.instances.len() as u32);
            for c in cols.clone() {
                let Ok(frame) = u32::try_from(self.frames[r * self.cols + c]) else { continue };
                let anim = self.anims.iter().position(|a| a.frame == frame as i32);
                let drawn = anim.map_or(frame, |a| self.current[a]);
                if let Some(a) = anim {
                    chunk.anims.push((chunk.instances.len() as u32, a as u16));
                }
                let pos = origin + Vec2::new((c as f32 + 0.5) * tile, -(r as f32 + 0.5) * tile);
                chunk.instances.push(Instance {
                    pos: pos.into(),
                    size: [tile; 2],
                    rotation_z: [0.0, z],
                    uv: region(drawn).1.into(),
                    color: [255; 4],
                    flags: LIT,
                    params: [0.0; 4],
                });
                chunk.cols.push((c - cols.start) as u16);
            }
        }
        chunk.rows.push(chunk.instances.len() as u32);
        chunk.shown.clone_from(&self.current);
        chunk.texture = Some(texture);
        chunk.dirty = false;
        self.rebuilt += 1;
    }

    /// 画一个视野：拷可见块里视野附近的格子进 `out`（加上视差偏移），返回用的贴图和统计。
    /// 按需重建脏块、换动画帧，放掉很久没看见的块的缓存。每帧每个视野调一次。
    pub(crate) fn draw(&mut self, view: &View2d, parallax: Option<Vec2>, elapsed: f64, region: &dyn Fn(u32) -> (TextureId, Vec4), out: &mut Vec<Instance>) -> (TextureId, TileDrawStats) {
        let texture = region(0).0;
        let mut stats = TileDrawStats { total: self.chunks.len() as u32, ..Default::default() };
        self.current.clear();
        self.current.extend(self.anims.iter().map(|a| a.at(elapsed)));
        let mut visible = Vec::new();
        self.visible_in(view, parallax, &mut visible);
        let offset = parallax_offset(parallax, &view.camera);
        let (cols, rows) = self.cell_range(view, offset, self.tile);
        for &k in &visible {
            let chunk = &self.chunks[k];
            if chunk.dirty || chunk.texture != Some(texture) {
                if chunk.texture.is_none() {
                    self.live.push(k);
                }
                self.build(k, texture, region);
            } else if chunk.shown != self.current {
                let chunk = &mut self.chunks[k];
                for &(i, a) in &chunk.anims {
                    let a = a as usize;
                    if chunk.shown[a] != self.current[a] {
                        chunk.instances[i as usize].uv = region(self.current[a]).1.into();
                        self.patched += 1;
                    }
                }
                chunk.shown.clone_from(&self.current);
            }
            let (chunk_cols, chunk_rows) = self.chunk_cells(k);
            let chunk = &mut self.chunks[k];
            chunk.unseen = 0;
            stats.shown += 1;
            let (c0, c1) = (cols.start.max(chunk_cols.start) - chunk_cols.start, cols.end.min(chunk_cols.end).saturating_sub(chunk_cols.start));
            for r in rows.start.max(chunk_rows.start)..rows.end.min(chunk_rows.end) {
                let line = chunk.rows[r - chunk_rows.start] as usize..chunk.rows[r - chunk_rows.start + 1] as usize;
                for i in line {
                    let c = chunk.cols[i] as usize;
                    if c < c0 || c >= c1 {
                        continue;
                    }
                    let mut instance = chunk.instances[i];
                    instance.pos = (Vec2::from(instance.pos) + offset).into();
                    out.push(instance);
                    stats.tiles += 1;
                }
            }
        }
        // 很久没看见的块放掉缓存
        let chunks = &mut self.chunks;
        self.live.retain(|&k| {
            let chunk = &mut chunks[k];
            if visible.binary_search(&k).is_ok() {
                return true;
            }
            chunk.unseen += 1;
            if chunk.unseen <= KEEP_FRAMES {
                return true;
            }
            *chunk = Chunk { dirty: true, ..Default::default() };
            false
        });
        (texture, stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 × 100 格、每格 10、每块 10 × 10 格：一共 10 × 10 块，左上角在 (0, 0)，地图往右下铺。
    fn layer(frames: Vec<i32>, anims: Vec<TileAnim>) -> TileLayer {
        TileLayer::new(SpriteImage::Texture(TextureId::WHITE), Vec2::ZERO, 10.0, 100, frames, 10, anims, 0.0)
    }

    fn full() -> TileLayer {
        layer(vec![0; 100 * 100], Vec::new())
    }

    /// `s` × `s` 的视野。
    fn sized(x: f32, y: f32, s: f32) -> View2d {
        View2d::new(Camera2d { position: Vec2::new(x, y), ..Default::default() }, Vec2::splat(s))
    }

    fn view(x: f32, y: f32) -> View2d {
        sized(x, y, 100.0)
    }

    /// 60 × 60、正好落在块 (1, 1) 里（外扩一格也不出去）。
    fn small() -> View2d {
        sized(150.0, -150.0, 60.0)
    }

    fn region(frame: u32) -> (TextureId, Vec4) {
        (TextureId::WHITE, Vec4::new(frame as f32, 0.0, 1.0, 1.0))
    }

    /// 块号换成 (块列, 块行)。
    fn cells(ks: &[usize]) -> Vec<(usize, usize)> {
        ks.iter().map(|k| (k % 10, k / 10)).collect()
    }

    fn grid(xs: std::ops::RangeInclusive<usize>, ys: std::ops::RangeInclusive<usize>) -> Vec<(usize, usize)> {
        ys.flat_map(|y| xs.clone().map(move |x| (x, y))).collect()
    }

    #[test]
    fn visible_chunks_cover_the_view_plus_one_tile() {
        let l = full();
        // 视野 x 100 … 200、y -200 … -100：正好是块 (1, 1)；外扩一格碰到四周的块
        assert_eq!(cells(&l.visible(&[view(150.0, -150.0)], None)), grid(0..=2, 0..=2));
        assert_eq!(cells(&l.visible(&[small()], None)), [(1, 1)]);
        // y -195 … -135 在块 (1, 1) 里，外扩一格到 -205，碰到下面一块
        assert_eq!(cells(&l.visible(&[sized(150.0, -165.0, 60.0)], None)), [(1, 1), (1, 2)]);
        assert!(l.visible(&[view(-500.0, 0.0)], None).is_empty(), "地图外");
    }

    #[test]
    fn zoom_changes_how_much_is_visible() {
        let l = full();
        let zoomed = |zoom: f32| {
            let mut v = view(500.0, -500.0);
            v.camera.zoom = zoom;
            l.visible(&[v], None).len()
        };
        // 放大两倍：视野 50 × 50（475 … 525），外扩后 465 … 535，横竖各 2 块
        assert_eq!(zoomed(2.0), 4);
        // 缩小一半：视野 200 × 200（400 … 600），外扩后 390 … 610，横竖各 4 块
        assert_eq!(zoomed(0.5), 16);
    }

    #[test]
    fn a_turned_camera_skips_the_corners_of_its_bounding_box() {
        let l = full();
        let mut v = view(500.0, -500.0);
        v.size = Vec2::new(300.0, 10.0);
        v.camera.rotation = std::f32::consts::FRAC_PI_4;
        // 转 45° 的细长视野：外接框是 220 见方（块的第 3 … 6 列、行），视野只是从左下到右上的一条斜线
        let (lo, hi) = v.bounds(0.0);
        assert!((hi - lo - Vec2::splat(155.0 * std::f32::consts::SQRT_2)).length() < 1.0, "{lo} {hi}");
        let seen = cells(&l.visible(&[v], None));
        assert!(seen.contains(&(3, 6)) && seen.contains(&(6, 3)) && seen.contains(&(4, 5)), "斜线经过的块 {seen:?}");
        assert!(!seen.contains(&(3, 3)) && !seen.contains(&(6, 6)), "外接框的另外两个角不算 {seen:?}");
        // 同一个视野不转时是一条横线：只有一行块
        (v.camera.rotation, v.camera.position) = (0.0, Vec2::new(500.0, -550.0));
        assert_eq!(cells(&l.visible(&[v], None)).iter().map(|c| c.1).collect::<std::collections::HashSet<_>>().len(), 1);
    }

    #[test]
    fn parallax_shifts_the_layer_with_the_camera() {
        let l = full();
        // 视差 0.5、相机在 x = 1200：地图挪了 600，相机看到的是地图里 x 550 … 650
        let seen = cells(&l.visible(&[view(1200.0, -500.0)], Some(Vec2::new(0.5, 1.0))));
        assert_eq!(seen.iter().map(|c| c.0).min(), Some(5));
        assert_eq!(seen.iter().map(|c| c.0).max(), Some(6));
        assert!(l.visible(&[view(1200.0, -500.0)], None).is_empty(), "没有视差时 x 1200 在地图外");
        assert!(l.visible(&[view(1200.0, -500.0)], Some(Vec2::new(f32::NAN, 1.0))).is_empty(), "坏的视差当 1");
    }

    #[test]
    fn several_views_see_the_union() {
        let l = full();
        // 分屏两个相机、小地图一个缩小的相机
        let mut minimap = view(500.0, -500.0);
        minimap.camera.zoom = 0.1;
        let a = l.visible(&[view(150.0, -150.0)], None);
        let b = l.visible(&[view(850.0, -850.0)], None);
        let both = l.visible(&[view(150.0, -150.0), view(850.0, -850.0)], None);
        assert_eq!(both.len(), a.len() + b.len());
        assert!(a.iter().chain(&b).all(|k| both.contains(k)));
        assert_eq!(l.visible(&[view(150.0, -150.0), minimap], None).len(), 100, "缩小的相机看到整张图，并起来也不重复");
    }

    #[test]
    fn only_tiles_near_the_view_are_drawn() {
        let mut l = full();
        let mut out = Vec::new();
        let (_, stats) = l.draw(&view(155.0, -155.0), None, 0.0, &region, &mut out);
        // 视野 105 … 205 外扩一格是 95 … 215：第 9 … 21 列、行，13 × 13 格，碰到 3 × 3 块
        assert_eq!((stats.shown, stats.total, stats.tiles), (9, 100, 169));
        assert_eq!(out.len(), 169);
        assert!(out.iter().all(|i| (95.0..=215.0).contains(&i.pos[0]) && (-215.0..=-95.0).contains(&i.pos[1])));
        // 空格不画
        let mut l = layer((0..100 * 100).map(|i| if i % 2 == 0 { 0 } else { -1 }).collect(), Vec::new());
        out.clear();
        let (_, stats) = l.draw(&view(155.0, -155.0), None, 0.0, &region, &mut out);
        assert_eq!((stats.tiles, out.len()), (78, 78), "第 10、12 … 20 列有格子");
    }

    #[test]
    fn a_tile_one_cell_past_the_edge_is_drawn_two_cells_past_is_not() {
        // 视野 x 135 … 195：第 20 列（200 … 210）离右边一格以内，在下一块里；第 21 列远了
        let mut frames = vec![-1; 100 * 100];
        frames[15 * 100 + 20] = 0;
        frames[15 * 100 + 21] = 0;
        let mut l = layer(frames, Vec::new());
        let mut out = Vec::new();
        let (_, stats) = l.draw(&sized(165.0, -150.0, 60.0), None, 0.0, &region, &mut out);
        assert_eq!(stats.tiles, 1);
        assert_eq!(out[0].pos, [205.0, -155.0]);
    }

    #[test]
    fn drawn_tiles_carry_the_parallax_offset() {
        let mut l = full();
        let mut out = Vec::new();
        l.draw(&view(1200.0, -500.0), Some(Vec2::new(0.5, 1.0)), 0.0, &region, &mut out);
        assert!(!out.is_empty());
        assert!(out.iter().all(|i| (1140.0..=1260.0).contains(&i.pos[0])), "画在视野里：地图里 540 … 660 的格子挪了 600");
    }

    #[test]
    fn setting_a_cell_rebuilds_only_its_chunk() {
        let mut l = full();
        let mut out = Vec::new();
        l.draw(&view(150.0, -150.0), None, 0.0, &region, &mut out);
        assert_eq!(l.rebuilt(), 9, "第一次画时建可见的 9 块");
        l.draw(&view(150.0, -150.0), None, 0.0, &region, &mut out);
        assert_eq!(l.rebuilt(), 9, "没变不重建");
        assert_eq!(l.set(15, 15, 3), Some(11));
        assert_eq!(l.set(15, 15, 3), None, "一样的值不算改");
        assert_eq!(l.set(100, 0, 3), None, "地图外");
        out.clear();
        l.draw(&view(150.0, -150.0), None, 0.0, &region, &mut out);
        assert_eq!(l.rebuilt(), 10, "只重建改了的那一块");
        let cell = out.iter().find(|i| i.pos == [155.0, -155.0]).unwrap();
        assert_eq!(cell.uv[0], 3.0, "画的是新帧");
        l.set(15, 15, -1);
        out.clear();
        l.draw(&view(150.0, -150.0), None, 0.0, &region, &mut out);
        assert!(out.iter().all(|i| i.pos != [155.0, -155.0]), "挖掉了");
        // 看不见的块改了不建，等看见了再建
        l.set(95, 95, 2);
        l.draw(&view(150.0, -150.0), None, 0.0, &region, &mut out);
        assert_eq!(l.rebuilt(), 11);
    }

    #[test]
    fn moving_the_camera_builds_only_the_chunks_that_came_into_view() {
        let mut l = full();
        let mut out = Vec::new();
        l.draw(&view(150.0, -150.0), None, 0.0, &region, &mut out);
        assert_eq!(l.rebuilt(), 9);
        // 往右挪一块：新进来右边一列的 3 块
        l.draw(&view(250.0, -150.0), None, 0.0, &region, &mut out);
        assert_eq!(l.rebuilt(), 12);
        // 挪回来：缓存还在
        l.draw(&view(150.0, -150.0), None, 0.0, &region, &mut out);
        assert_eq!((l.rebuilt(), l.cached()), (12, 12));
    }

    #[test]
    fn caches_out_of_sight_for_a_while_are_dropped() {
        let mut l = full();
        let mut out = Vec::new();
        l.draw(&small(), None, 0.0, &region, &mut out);
        let far = sized(850.0, -850.0, 60.0);
        for _ in 0..KEEP_FRAMES {
            l.draw(&far, None, 0.0, &region, &mut out);
        }
        assert_eq!(l.cached(), 2, "还没到时候");
        l.draw(&far, None, 0.0, &region, &mut out);
        assert_eq!(l.cached(), 1, "原来那块放掉了");
        l.draw(&small(), None, 0.0, &region, &mut out);
        assert_eq!(l.rebuilt(), 3, "回来时重建");
    }

    #[test]
    fn animated_tiles_change_uv_without_rebuilding() {
        // 帧 3 的格子按 [3, 4, 5] 以 4 fps 循环；每隔 7 格一个
        let frames: Vec<i32> = (0..100 * 100).map(|i| if i % 7 == 0 { 3 } else { 0 }).collect();
        let mut l = layer(frames, vec![TileAnim { frame: 3, frames: vec![3, 4, 5], fps: 4.0 }]);
        let mut out = Vec::new();
        l.draw(&small(), None, 0.0, &region, &mut out);
        let n = l.chunks[11].anims.len();
        assert!(n > 10);
        assert_eq!((l.rebuilt(), l.patched()), (1, 0));
        out.clear();
        // 0.2 秒：还是第一帧
        l.draw(&small(), None, 0.2, &region, &mut out);
        assert_eq!(l.patched(), 0);
        out.clear();
        // 1 秒：第 4 格，循环到 [3, 4, 5] 的第 1 格 = 4
        l.draw(&small(), None, 1.0, &region, &mut out);
        assert_eq!((l.rebuilt(), l.patched()), (1, n as u64), "只改了动画格子的 uv");
        assert!(out.iter().any(|i| i.uv[0] == 4.0) && out.iter().all(|i| i.uv[0] != 3.0));
        // 看不见的块不改，看见了再按当时的帧建
        out.clear();
        l.draw(&sized(850.0, -850.0, 60.0), None, 1.25, &region, &mut out);
        assert_eq!((l.rebuilt(), l.patched()), (2, n as u64));
        assert!(out.iter().any(|i| i.uv[0] == 5.0) && out.iter().all(|i| i.uv[0] != 3.0));
    }

    #[test]
    fn tile_animations_follow_the_scene_clock() {
        let a = TileAnim { frame: 3, frames: vec![3, 4, 5], fps: 4.0 };
        assert_eq!([0.0, 0.26, 0.5, 0.75, 1.0, 2.5].map(|t| a.at(t)), [3, 4, 5, 3, 4, 4]);
        assert_eq!(a.at(f64::NAN), 3);
    }

    #[test]
    fn a_new_texture_rebuilds_the_caches() {
        let mut l = full();
        let mut out = Vec::new();
        l.draw(&small(), None, 0.0, &region, &mut out);
        let loaded = |f: u32| (TextureId::new(7), Vec4::new(f as f32, 0.0, 1.0, 1.0));
        let (texture, _) = l.draw(&small(), None, 0.0, &loaded, &mut out);
        assert_eq!((texture, l.rebuilt()), (TextureId::new(7), 2), "贴图加载完了：可见的块按新贴图重建");
    }

    #[test]
    fn drawn_frames_follow_the_animations() {
        let anims = vec![TileAnim { frame: 3, frames: vec![3, 4, 5], fps: 4.0 }];
        let l = TileLayer::new(SpriteImage::Texture(TextureId::WHITE), Vec2::ZERO, 10.0, 2, vec![3, 0, -1, 3], 10, anims, 0.0);
        assert_eq!((l.drawn_frame(0, 0, 1.0), l.drawn_frame(1, 0, 1.0), l.drawn_frame(0, 1, 1.0)), (Some(4), Some(0), None));
        assert_eq!(l.drawn_frame(9, 9, 0.0), None);
    }

    #[test]
    fn layer_shape_and_cells() {
        let l = TileLayer::new(SpriteImage::Texture(TextureId::WHITE), Vec2::ZERO, 10.0, 70, vec![0; 70 * 33 + 5], 32, Vec::new(), 0.0);
        assert_eq!((l.cols(), l.rows(), l.chunk_count(), l.chunk_cols()), (70, 33, 6, 3), "不满一行的丢掉，右边、下边不满一块的照样一块");
        assert_eq!(l.chunk_of(69, 32), 5);
        assert_eq!(l.chunk_cells(5), (64..70, 32..33));
        assert_eq!(l.frame(69, 32), Some(0));
        assert_eq!(l.frame(70, 0), None);
    }

    /// 2000 × 200、地面起伏的大地图（同 nge-scene 的性能回归）。
    fn big_map() -> Vec<i32> {
        let (cols, rows) = (2000, 200);
        let mut d = vec![-1; cols * rows];
        for c in 0..cols {
            let ground = 120 + (c * 7 % 23);
            for r in ground..rows {
                d[r * cols + c] = 0;
            }
        }
        d
    }

    fn fastest(runs: usize, mut f: impl FnMut()) -> std::time::Duration {
        (0..runs)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed()
            })
            .min()
            .unwrap()
    }

    /// 镜头移动一帧拷瓦片（1280 × 720 的视野）的阈值；改动前一帧要遍历 40 万个精灵。取最快的一次，放得很宽。
    const MOVE_LIMIT: std::time::Duration = std::time::Duration::from_millis(4);

    #[test]
    fn moving_the_camera_over_a_huge_map_copies_only_what_is_in_view() {
        let mut layer = TileLayer::new(SpriteImage::Texture(TextureId::WHITE), Vec2::ZERO, 16.0, 2000, big_map(), 32, Vec::new(), 0.0);
        let region = |_: u32| (TextureId::WHITE, Vec4::new(0.0, 0.0, 1.0, 1.0));
        let mut out: Vec<Instance> = Vec::new();
        let view = |x: f32| View2d::new(Camera2d { position: Vec2::new(x, -2400.0), ..Default::default() }, Vec2::new(1280.0, 720.0));
        // 先走一遍把缓存建好，再量每帧往右挪半格
        for i in 0..60 {
            out.clear();
            layer.draw(&view(1000.0 + i as f32 * 8.0), None, 0.0, &region, &mut out);
        }
        let mut x = 1480.0;
        let step = fastest(20, || {
            x += 8.0;
            out.clear();
            layer.draw(&view(x), None, 0.0, &region, &mut out);
        });
        let tiles = out.len();
        let mut fresh = 0;
        // 跳到没去过的地方：要现建几块
        let cold = fastest(1, || {
            out.clear();
            fresh = layer.draw(&view(20000.0), None, 0.0, &region, &mut out).1.shown;
        });
        eprintln!("镜头挪一步：{step:?}（{tiles} 格）；跳到新地方：{cold:?}（{fresh} 块）");
        assert!(tiles < 6000, "只拷视野附近的 {tiles}");
        assert!(step < MOVE_LIMIT, "挪一步 {step:?}");
        assert!(cold < MOVE_LIMIT * 5, "跳到新地方 {cold:?}");
    }
}
