//! PNG 转 KTX2 的小工具（examples/ktx2.rs）。

#[path = "../examples/ktx2.rs"]
mod tool;

use glam::UVec2;
use nge_assets::{BlockFormat, Image, ktx2::level_size};

#[test]
fn converted_files_load_back_with_every_mip_level() {
    let size = UVec2::new(12, 5);
    let rgba: Vec<u8> = (0..size.x * size.y).flat_map(|i| [200, 40, (i * 4) as u8, 255]).collect();
    let image = Image::decode(&tool::convert(rgba, size, true)).unwrap();
    assert_eq!(image.size, size);
    let c = image.compressed.unwrap();
    assert_eq!((c.format, c.srgb, c.levels.len()), (BlockFormat::Bc3, true, 4), "12×5 → 6×2 → 3×1 → 1×1");
    for (level, data) in c.levels.iter().enumerate() {
        assert_eq!(data.len(), BlockFormat::Bc3.level_bytes(level_size(size, level)));
    }
    let back = c.decode_rgba(size).unwrap();
    assert!(back.chunks(4).all(|p| p[0].abs_diff(200) < 12 && p[1].abs_diff(40) < 12 && p[3] == 255), "BC3 有损但颜色接近");
}

#[test]
fn tall_images_keep_halving_until_one_by_one() {
    let size = UVec2::new(4, 16);
    let image = Image::decode(&tool::convert(vec![128; (size.x * size.y * 4) as usize], size, true)).unwrap();
    assert_eq!(image.compressed.unwrap().levels.len(), 5, "4×16 → 2×8 → 1×4 → 1×2 → 1×1");
    let atlas = Image::decode(&tool::convert(vec![128; (size.x * size.y * 4) as usize], size, false)).unwrap();
    assert_eq!(atlas.compressed.unwrap().levels.len(), 1, "图集不要 mipmap");
}

#[test]
fn halving_averages_two_by_two_and_repeats_the_odd_edge() {
    // 3×1：10, 30, 90 → 宽 1，丢掉最后一列；高是 1，那一行重复用 → (10+30+10+30)/4 = 20
    let rgba = [10, 0, 0, 0, 30, 0, 0, 0, 90, 0, 0, 0];
    let (out, size) = tool::half(&rgba, UVec2::new(3, 1));
    assert_eq!((out, size), (vec![20, 0, 0, 0], UVec2::ONE));
    // 1×2：宽是 1，那一列重复用 → (10+10+30+30)/4 = 20
    assert_eq!(tool::half(&[10, 0, 0, 0, 30, 0, 0, 0], UVec2::new(1, 2)), (vec![20, 0, 0, 0], UVec2::ONE));
    let rgba: Vec<u8> = [0u8, 4, 8, 14].iter().flat_map(|&v| [v, v, v, v]).collect();
    assert_eq!(tool::half(&rgba, UVec2::splat(2)), (vec![7; 4], UVec2::ONE), "26 / 4 = 6.5，四舍五入是 7（直接截断是 6）");
}
