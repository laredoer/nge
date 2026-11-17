//! 把 PNG 转成 KTX2（BC3 + mipmap，sRGB）：`cargo run -p nge-assets --example ktx2 -- [--no-mips] 输入.png 输出.ktx2`。
//! 图集（精灵表）加 `--no-mips`：缩小后低的级会混进相邻的格子。
//! mipmap 直接在 sRGB 空间对没预乘的颜色求平均，暗部偏暗、半透明边上可能有色边。
//! 做手机游戏用 ASTC 时换成 astcenc / toktx（不带超压缩）。

use glam::UVec2;
use nge_assets::{BlockFormat, ktx2};

/// 缩成一半（2×2 盒式滤波；奇数边丢掉最后一列 / 行，边长是 1 的那边重复用）。
pub fn half(rgba: &[u8], size: UVec2) -> (Vec<u8>, UVec2) {
    let out = UVec2::new((size.x / 2).max(1), (size.y / 2).max(1));
    let px = |x: u32, y: u32, c: usize| u32::from(rgba[((y.min(size.y - 1) * size.x + x.min(size.x - 1)) * 4) as usize + c]);
    let mut pixels = Vec::with_capacity((out.x * out.y * 4) as usize);
    for y in 0..out.y {
        for x in 0..out.x {
            for c in 0..4 {
                let sum = px(2 * x, 2 * y, c) + px(2 * x + 1, 2 * y, c) + px(2 * x, 2 * y + 1, c) + px(2 * x + 1, 2 * y + 1, c);
                pixels.push(((sum + 2) / 4) as u8);
            }
        }
    }
    (pixels, out)
}

/// RGBA8 → BC3 的 KTX2，`mips` 时带一直到 1×1 的 mipmap。宽高不是 4 的倍数时 GPU 用不了，引擎会回退解码。
pub fn convert(rgba: Vec<u8>, size: UVec2, mips: bool) -> Vec<u8> {
    let mut levels = Vec::new();
    let (mut pixels, mut s) = (rgba, size);
    loop {
        let mut out = vec![0; texpresso::Format::Bc3.compressed_size(s.x as usize, s.y as usize)];
        texpresso::Format::Bc3.compress(&pixels, s.x as usize, s.y as usize, texpresso::Params::default(), &mut out);
        levels.push(out);
        if !mips || s == UVec2::ONE {
            break;
        }
        (pixels, s) = half(&pixels, s);
    }
    ktx2::write(Some(BlockFormat::Bc3), true, size, &levels)
}

#[allow(dead_code)]
fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mips = !args.iter().any(|a| a == "--no-mips");
    args.retain(|a| a != "--no-mips");
    let [input, output] = args.as_slice() else {
        eprintln!("用法：cargo run -p nge-assets --example ktx2 -- [--no-mips] 输入.png 输出.ktx2");
        std::process::exit(2);
    };
    let image = match image::open(input) {
        Ok(image) => image.into_rgba8(),
        Err(e) => {
            eprintln!("读不了 {input}: {e}");
            std::process::exit(1);
        }
    };
    let size = UVec2::new(image.width(), image.height());
    if size % 4 != UVec2::ZERO {
        eprintln!("注意：{size} 不是 4 的倍数，GPU 不能直接用，加载时会解成 RGBA8");
    }
    let bytes = convert(image.into_raw(), size, mips);
    if let Err(e) = std::fs::write(output, &bytes) {
        eprintln!("写不了 {output}: {e}");
        std::process::exit(1);
    }
    println!("{output}: {size}，BC3，{} 字节", bytes.len());
}
