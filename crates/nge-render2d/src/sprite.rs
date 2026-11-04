//! 精灵组件。

use bevy_ecs::prelude::{Component, Resource};
use glam::{Vec2, Vec4};

use crate::assets::{AtlasId, TextureId};

/// 画在 [`crate::Transform2d`] 位置上的一张图。`size` 为空时按图片像素大小（1 像素 = 1 point）。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub image: SpriteImage,
    pub size: Option<Vec2>,
    /// 与纹理相乘的颜色（线性 RGBA）。
    pub color: Vec4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteImage {
    Texture(TextureId),
    Atlas(AtlasId, u32),
}

impl Sprite {
    pub fn from_texture(texture: TextureId) -> Self {
        Self { image: SpriteImage::Texture(texture), size: None, color: Vec4::ONE }
    }

    pub fn from_atlas(atlas: AtlasId, index: u32) -> Self {
        Self { image: SpriteImage::Atlas(atlas, index), size: None, color: Vec4::ONE }
    }

    /// 纯色矩形。
    pub fn color(size: Vec2, color: Vec4) -> Self {
        Self { image: SpriteImage::Texture(TextureId::WHITE), size: Some(size), color }
    }

    pub fn with_size(self, size: Vec2) -> Self {
        Self { size: Some(size), ..self }
    }

    pub fn with_color(self, color: Vec4) -> Self {
        Self { color, ..self }
    }
}

/// 不是实体的精灵（粒子之类）的一份数据。坐标是世界坐标，`size` 是最终大小。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteInstance {
    pub position: Vec2,
    pub size: Vec2,
    pub rotation: f32,
    pub z: f32,
    /// 线性 RGBA。
    pub color: Vec4,
}

impl Default for SpriteInstance {
    fn default() -> Self {
        Self { position: Vec2::ZERO, size: Vec2::ONE, rotation: 0.0, z: 0.0, color: Vec4::ONE }
    }
}

/// 每帧额外要画的精灵：系统在 Update / PostUpdate 里往里放，和实体精灵一起合批；每帧在 PreUpdate 里清空（别在 PreUpdate 里放）。
#[derive(Resource, Debug, Default)]
pub struct SpriteInstances {
    pub(crate) items: Vec<(SpriteImage, SpriteInstance)>,
}

impl SpriteInstances {
    pub fn push(&mut self, image: SpriteImage, instance: SpriteInstance) {
        self.items.push((image, instance));
    }

    pub fn extend(&mut self, image: SpriteImage, instances: impl IntoIterator<Item = SpriteInstance>) {
        self.items.extend(instances.into_iter().map(|i| (image, i)));
    }

    /// 这一帧放进来的实例。
    pub fn iter(&self) -> impl Iterator<Item = &(SpriteImage, SpriteInstance)> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
}
