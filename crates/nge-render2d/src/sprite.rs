//! 精灵组件。

use bevy_ecs::prelude::Component;
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
