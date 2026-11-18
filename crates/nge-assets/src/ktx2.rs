//! KTX2 贴图（见 docs/compressed-textures.md）：读 GPU 块压缩格式（ASTC / BC / ETC2）和 RGBA8，带 mipmap；不支持超压缩。

use glam::UVec2;

use crate::image::{Image, MAX_TEXTURE_SIDE};

/// KTX2 文件开头的 12 个字节。
pub const MAGIC: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
/// 头（含 DFD / KVD / SGD 的位置）的长度，后面跟着每一级的索引。
const HEADER: usize = 80;
/// 每一级的索引：偏移、长度、解压后长度，各 8 字节。
const LEVEL_INDEX: usize = 24;

const VK_R8G8B8A8_UNORM: u32 = 37;
const VK_R8G8B8A8_SRGB: u32 = 43;
/// BC1 不带透明度（RGB）：三色模式下第 4 种颜色是不透明黑，wgpu 只有带透明度的 BC1（那里是透明黑），不收。
const VK_BC1_RGB: [u32; 2] = [131, 132];

/// GPU 能直接用的块压缩格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockFormat {
    Bc1,
    Bc3,
    Bc7,
    Etc2Rgba8,
    Astc4x4,
    Astc6x6,
    Astc8x8,
}

impl BlockFormat {
    /// 块的宽高（像素）。
    pub fn block(self) -> UVec2 {
        match self {
            Self::Astc6x6 => UVec2::splat(6),
            Self::Astc8x8 => UVec2::splat(8),
            _ => UVec2::splat(4),
        }
    }

    /// 每块多少字节。
    pub fn block_bytes(self) -> usize {
        if self == Self::Bc1 { 8 } else { 16 }
    }

    /// 宽高为 `size` 时横竖各多少块（不满一块的按一块算）。
    pub fn blocks(self, size: UVec2) -> UVec2 {
        let b = self.block();
        UVec2::new(size.x.div_ceil(b.x), size.y.div_ceil(b.y))
    }

    /// 宽高为 `size` 的一级要多少字节。
    pub fn level_bytes(self, size: UVec2) -> usize {
        let blocks = self.blocks(size).as_u64vec2();
        (blocks.x * blocks.y) as usize * self.block_bytes()
    }

    /// KTX2 里的 `vkFormat`（UNORM, SRGB）。
    fn vk(self) -> (u32, u32) {
        match self {
            Self::Bc1 => (133, 134),
            Self::Bc3 => (137, 138),
            Self::Bc7 => (145, 146),
            Self::Etc2Rgba8 => (151, 152),
            Self::Astc4x4 => (157, 158),
            Self::Astc6x6 => (165, 166),
            Self::Astc8x8 => (171, 172),
        }
    }

    const ALL: [Self; 7] = [Self::Bc1, Self::Bc3, Self::Bc7, Self::Etc2Rgba8, Self::Astc4x4, Self::Astc6x6, Self::Astc8x8];

    /// `vkFormat` → (格式, 是不是 sRGB)。
    fn from_vk(vk: u32) -> Option<(Self, bool)> {
        Self::ALL.into_iter().find_map(|f| {
            let (unorm, srgb) = f.vk();
            (vk == unorm || vk == srgb).then_some((f, vk == srgb))
        })
    }
}

/// 块压缩的像素：`levels[0]` 是原尺寸，往后每级边长减半（不小于 1）。
#[derive(Debug, Clone, PartialEq)]
pub struct Compressed {
    pub format: BlockFormat,
    pub srgb: bool,
    pub levels: Vec<Vec<u8>>,
}

/// 第 `level` 级的尺寸。
pub fn level_size(size: UVec2, level: usize) -> UVec2 {
    UVec2::new((size.x >> level).max(1), (size.y >> level).max(1))
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("长度已检查"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("长度已检查"))
}

/// 读一个 KTX2 文件。
pub fn parse(bytes: &[u8]) -> Result<Image, String> {
    let (size, block, levels) = read(bytes, 1)?;
    match block {
        None => Image::from_rgba(size, levels.into_iter().next().expect("至少一级")),
        Some((format, srgb)) => {
            if !srgb {
                log::warn!("KTX2 是 UNORM 的 {format:?}，按线性数据采样；颜色贴图请导出成 SRGB，不然会发白");
            }
            Ok(Image { size, rgba: Vec::new(), compressed: Some(Compressed { format, srgb, levels }), mipmaps: false })
        }
    }
}

/// 读一个 KTX2 立方体贴图（`faceCount = 6`）：只要第 0 级，6 个面按 +X、-X、+Y、-Y、+Z、-Z 解成 RGBA8（压缩的在 CPU 上解开）。
pub fn parse_cube(bytes: &[u8]) -> Result<CubeImage, String> {
    let (size, block, levels) = read(bytes, 6)?;
    if size.x != size.y {
        return Err(format!("KTX2 立方体贴图的面要是正方形，收到 {size}"));
    }
    let level = levels.into_iter().next().expect("至少一级");
    let per_face = level.len() / 6;
    let faces = (0..6)
        .map(|f| {
            let data = level[f * per_face..(f + 1) * per_face].to_vec();
            match block {
                None => Ok(data),
                Some((format, srgb)) => Compressed { format, srgb, levels: vec![data] }.decode_rgba(size),
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(CubeImage { size: size.x, faces: faces.try_into().expect("6 个面") })
}

/// 6 个面一样大的立方体贴图，RGBA8（按 sRGB 用），面的顺序 +X、-X、+Y、-Y、+Z、-Z。
#[derive(Debug, Clone, PartialEq)]
pub struct CubeImage {
    pub size: u32,
    pub faces: [Vec<u8>; 6],
}

impl crate::Asset for CubeImage {
    fn decode(bytes: Vec<u8>, _ctx: &crate::LoadContext) -> Result<Self, String> {
        parse_cube(&bytes)
    }
}

/// 读头和每一级（立方体的每级里 `faces` 个面连着放）。
type Levels = (UVec2, Option<(BlockFormat, bool)>, Vec<Vec<u8>>);

fn read(bytes: &[u8], want_faces: u32) -> Result<Levels, String> {
    if bytes.len() < HEADER || bytes[..12] != MAGIC {
        return Err("不是 KTX2 文件".into());
    }
    let field = |at: usize| u32_at(bytes, at);
    let (vk, width, height, depth, layers, faces, level_count, scheme) = (field(12), field(20), field(24), field(28), field(32), field(36), field(40), field(44));
    if scheme != 0 {
        return Err(format!("KTX2 用了超压缩（supercompressionScheme {scheme}），不支持；生成时不要加 zstd / Basis"));
    }
    if depth != 0 {
        return Err(format!("KTX2 是 3D 贴图（pixelDepth {depth}），不支持"));
    }
    if layers != 0 {
        return Err(format!("KTX2 是数组贴图（layerCount {layers}），不支持"));
    }
    if faces != want_faces {
        return Err(if want_faces == 1 {
            format!("KTX2 是立方体贴图（faceCount {faces}），不能当普通贴图用；天空用 Sky 的立方体贴图")
        } else {
            format!("KTX2 不是立方体贴图（faceCount {faces}，要 6）")
        });
    }
    let size = UVec2::new(width, height);
    if size.min_element() == 0 || size.max_element() > MAX_TEXTURE_SIDE {
        return Err(format!("KTX2 尺寸 {size} 不合法（每边 1..={MAX_TEXTURE_SIDE}）"));
    }
    if VK_BC1_RGB.contains(&vk) {
        return Err(format!("KTX2 是不带透明度的 BC1（vkFormat {vk}），不支持；导出成 BC1 RGBA 或 BC7"));
    }
    let block = match vk {
        VK_R8G8B8A8_UNORM | VK_R8G8B8A8_SRGB => None,
        _ => Some(BlockFormat::from_vk(vk).ok_or_else(|| format!("KTX2 的格式 vkFormat {vk} 不支持（支持 RGBA8、BC1、BC3、BC7、ETC2 RGBA8、ASTC 4×4 / 6×6 / 8×8）"))?),
    };
    let format = block.map(|(f, _)| f);
    // 0 是要求加载时生成 mipmap：只有一级
    let count = level_count.max(1) as usize;
    // 完整的 mipmap 链：一直减半到 1×1。先查级数再算索引的长度，32 位上不会溢出
    let full = (size.max_element().ilog2() + 1) as usize;
    if count > full {
        return Err(format!("KTX2 的级数 {count} 不对（尺寸 {size} 最多 {full} 级）"));
    }
    if bytes.len() < HEADER + count * LEVEL_INDEX {
        return Err(format!("KTX2 的 {count} 级索引超出文件"));
    }
    let levels = (0..count)
        .map(|level| {
            let at = HEADER + level * LEVEL_INDEX;
            let (offset, length) = (u64_at(bytes, at), u64_at(bytes, at + 8));
            let s = level_size(size, level);
            let need = format.map_or((s.x * s.y * 4) as usize, |f| f.level_bytes(s)) * want_faces as usize;
            if length != need as u64 {
                return Err(format!("KTX2 第 {level} 级有 {length} 字节，应该是 {need}"));
            }
            let end = offset.checked_add(length).filter(|&end| end <= bytes.len() as u64).ok_or_else(|| format!("KTX2 第 {level} 级的数据超出文件"))?;
            Ok(bytes[offset as usize..end as usize].to_vec())
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((size, block, levels))
}

/// 写一个 RGBA8（sRGB）、只有一级的 KTX2 立方体贴图（工具和测试用）。面的长度不对时 panic。
pub fn write_cube(size: u32, faces: &[Vec<u8>; 6]) -> Vec<u8> {
    let mut out = write(None, true, UVec2::splat(size), &[faces.concat()[..(size * size * 4) as usize].to_vec()]);
    for f in faces {
        assert_eq!(f.len(), (size * size * 4) as usize, "面的长度不对");
    }
    // faceCount = 6，第 0 级的长度换成 6 个面的
    out[36..40].copy_from_slice(&6u32.to_le_bytes());
    let data = faces.concat();
    let at = HEADER;
    let offset = u64_at(&out, at) as usize;
    out.truncate(offset);
    out.extend(&data);
    out[at + 8..at + 16].copy_from_slice(&(data.len() as u64).to_le_bytes());
    out[at + 16..at + 24].copy_from_slice(&(data.len() as u64).to_le_bytes());
    out
}

/// 写一个引擎能读的 KTX2（工具和测试用）。每级数据按规范对齐、从最小的一级往大排；
/// 不写数据格式描述（DFD），别的工具（`ktx validate`）不认。
/// `format` 是 `None` 时是 RGBA8。`levels` 的长度不对时 panic。
pub fn write(format: Option<BlockFormat>, srgb: bool, size: UVec2, levels: &[Vec<u8>]) -> Vec<u8> {
    let vk = match format {
        Some(f) if srgb => f.vk().1,
        Some(f) => f.vk().0,
        None if srgb => VK_R8G8B8A8_SRGB,
        None => VK_R8G8B8A8_UNORM,
    };
    let mut out = MAGIC.to_vec();
    // typeSize：压缩格式和 RGBA8 都是 1
    for v in [vk, 1, size.x, size.y, 0, 0, 1, levels.len() as u32, 0, 0, 0, 0, 0] {
        out.extend(v.to_le_bytes());
    }
    out.extend([0u8; 16]); // SGD 的偏移和长度
    debug_assert_eq!(out.len(), HEADER);
    // 每级的起点按 lcm(块字节数, 4) 对齐：压缩格式是 8 或 16，RGBA8 是 4
    let align = format.map_or(4, BlockFormat::block_bytes);
    let mut offsets = vec![0; levels.len()];
    let mut end = HEADER + levels.len() * LEVEL_INDEX;
    for (level, data) in levels.iter().enumerate().rev() {
        let s = level_size(size, level);
        let need = format.map_or((s.x * s.y * 4) as usize, |f| f.level_bytes(s));
        assert_eq!(data.len(), need, "第 {level} 级的长度不对");
        offsets[level] = end.next_multiple_of(align);
        end = offsets[level] + data.len();
    }
    for (offset, data) in offsets.iter().zip(levels) {
        for v in [*offset as u64, data.len() as u64, data.len() as u64] {
            out.extend(v.to_le_bytes());
        }
    }
    for (level, data) in levels.iter().enumerate().rev() {
        out.resize(offsets[level], 0);
        out.extend(data);
    }
    out
}

impl Compressed {
    /// 把第 0 级在 CPU 上解成 RGBA8。
    pub fn decode_rgba(&self, size: UVec2) -> Result<Vec<u8>, String> {
        self.decode_level(size, 0)
    }

    /// 把第 `level` 级在 CPU 上解成 RGBA8（设备不支持这种格式时用）。`size` 是第 0 级的尺寸。
    /// 一行块一行块地解，内存只比结果多一行块；数据坏了返回错误，不 panic。
    pub fn decode_level(&self, size: UVec2, level: usize) -> Result<Vec<u8>, String> {
        let data = self.levels.get(level).ok_or_else(|| format!("没有第 {level} 级"))?;
        let s = level_size(size, level);
        let (w, h) = (s.x as usize, s.y as usize);
        let (block, blocks) = (self.format.block().as_usizevec2(), self.format.blocks(s).as_usizevec2());
        let row_bytes = blocks.x * self.format.block_bytes();
        if data.len() < row_bytes * blocks.y {
            return Err(format!("{:?} 第 {level} 级的数据不够", self.format));
        }
        let mut out = Vec::with_capacity(w * h * 4);
        let mut strip = vec![0u32; w * block.y];
        for by in 0..blocks.y {
            let rows = block.y.min(h - by * block.y);
            self.decode_strip(&data[by * row_bytes..(by + 1) * row_bytes], w, rows, &mut strip[..w * rows])?;
            // texture2ddecoder 的像素是小端的 BGRA
            out.extend(strip[..w * rows].iter().flat_map(|p| {
                let [b, g, r, a] = p.to_le_bytes();
                [r, g, b, a]
            }));
        }
        Ok(out)
    }

    /// 解一行块。texture2ddecoder 遇到坏的 ASTC 块会 panic（读到块后面、减法溢出），这里接住当成错误。
    fn decode_strip(&self, data: &[u8], w: usize, h: usize, pixels: &mut [u32]) -> Result<(), String> {
        use texture2ddecoder as d;
        let format = self.format;
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match format {
            BlockFormat::Bc1 => d::decode_bc1a(data, w, h, pixels),
            BlockFormat::Bc3 => d::decode_bc3(data, w, h, pixels),
            BlockFormat::Bc7 => d::decode_bc7(data, w, h, pixels),
            BlockFormat::Etc2Rgba8 => d::decode_etc2_rgba8(data, w, h, pixels),
            BlockFormat::Astc4x4 | BlockFormat::Astc6x6 | BlockFormat::Astc8x8 => {
                let b = format.block().as_usizevec2();
                d::decode_astc(data, w, h, b.x, b.y, pixels)
            }
        }));
        match decoded {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("解压 {format:?} 失败: {e}")),
            Err(_) => Err(format!("{format:?} 的数据坏了，解不出来")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个纯色的 ASTC 块（void-extent，颜色是 16 位 UNORM）。
    fn astc_solid(rgba: [u16; 4]) -> Vec<u8> {
        let mut block = vec![0xFC, 0xFD, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        for c in rgba {
            block.extend(c.to_le_bytes());
        }
        block
    }

    fn header_patch(bytes: &mut [u8], at: usize, v: u32) {
        bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn round_trips_every_format_with_mips() {
        let size = UVec2::new(9, 5);
        for format in BlockFormat::ALL {
            let levels: Vec<Vec<u8>> = (0..4).map(|l| vec![l as u8; format.level_bytes(level_size(size, l))]).collect();
            for srgb in [false, true] {
                let image = parse(&write(Some(format), srgb, size, &levels)).unwrap();
                assert_eq!((image.size, image.rgba.len()), (size, 0));
                assert_eq!(image.compressed, Some(Compressed { format, srgb, levels: levels.clone() }), "{format:?}");
            }
        }
        assert_eq!(BlockFormat::Astc6x6.level_bytes(UVec2::new(9, 5)), 2 * 16, "不满一块按一块算");
        assert_eq!(BlockFormat::Bc1.level_bytes(UVec2::new(8, 4)), 2 * 8);
        assert_eq!(level_size(UVec2::new(9, 5), 3), UVec2::new(1, 1));
    }

    #[test]
    fn cube_maps_round_trip_and_split_into_faces() {
        let faces: [Vec<u8>; 6] = std::array::from_fn(|f| (0..2 * 2 * 4).map(|i| (f * 16 + i) as u8).collect());
        let bytes = write_cube(2, &faces);
        let cube = parse_cube(&bytes).unwrap();
        assert_eq!((cube.size, &cube.faces), (2, &faces));
        assert!(parse(&bytes).unwrap_err().contains("立方体"), "普通贴图不收立方体");
        let mut wide = bytes.clone();
        header_patch(&mut wide, 20, 4);
        assert!(parse_cube(&wide).is_err(), "面要是正方形");
        // 压缩的面在 CPU 上解开
        let block = astc_solid([0xFFFF, 0, 0, 0xFFFF]);
        let mut astc = write(Some(BlockFormat::Astc4x4), true, UVec2::splat(4), &[block.repeat(6)[..16].to_vec()]);
        header_patch(&mut astc, 36, 6);
        let at = u64_at(&astc, HEADER) as usize;
        astc.truncate(at);
        astc.extend(block.repeat(6));
        astc[HEADER + 8..HEADER + 16].copy_from_slice(&96u64.to_le_bytes());
        let cube = parse_cube(&astc).unwrap();
        assert_eq!(cube.size, 4);
        assert!(cube.faces.iter().all(|f| f.len() == 64 && f[..4] == [255, 0, 0, 255]), "{:?}", &cube.faces[0][..4]);
    }

    #[test]
    fn rgba8_becomes_a_plain_image() {
        let size = UVec2::new(2, 1);
        let image = parse(&write(None, true, size, &[vec![1, 2, 3, 4, 5, 6, 7, 8], vec![9, 9, 9, 9]])).unwrap();
        assert_eq!((image.size, image.rgba, image.compressed), (size, vec![1, 2, 3, 4, 5, 6, 7, 8], None));
        assert!(parse(&write(None, false, size, &[vec![0; 8]])).is_ok(), "UNORM 也收，当 sRGB 用");
        let mut rgb = write(Some(BlockFormat::Bc1), true, UVec2::new(4, 4), &[vec![0; 8]]);
        for vk in VK_BC1_RGB {
            header_patch(&mut rgb, 12, vk);
            assert!(parse(&rgb).unwrap_err().contains("不带透明度的 BC1"), "{vk}");
        }
    }

    #[test]
    fn rejects_what_it_does_not_support() {
        let size = UVec2::new(4, 4);
        let good = write(Some(BlockFormat::Bc7), true, size, &[vec![0; 16]]);
        let patched = |at: usize, v: u32| {
            let mut b = good.clone();
            header_patch(&mut b, at, v);
            parse(&b).unwrap_err()
        };
        assert!(parse(b"not a texture at all, definitely not").unwrap_err().contains("不是 KTX2"));
        assert!(parse(&good[..40]).unwrap_err().contains("不是 KTX2"), "头不全");
        assert!(patched(44, 2).contains("超压缩"));
        assert!(patched(28, 1).contains("3D"));
        assert!(patched(32, 2).contains("数组"));
        assert!(patched(36, 6).contains("立方体"));
        assert!(parse_cube(&good).unwrap_err().contains("不是立方体"));
        assert!(patched(12, 999).contains("999"));
        assert!(patched(20, 0).contains("尺寸"));
        assert!(patched(20, MAX_TEXTURE_SIDE + 1).contains("尺寸"));
        assert!(patched(40, 5).contains("最多 3 级"), "4×4 最多 3 级");
        assert!(patched(40, 3).contains("索引超出文件"), "级数合法，但索引比文件长");
        let mut short = good.clone();
        short.truncate(short.len() - 1);
        assert!(parse(&short).unwrap_err().contains("超出"));
        let mut far = good.clone();
        far[HEADER..HEADER + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(parse(&far).unwrap_err().contains("超出"), "偏移加长度溢出");
        let mut wrong = good.clone();
        wrong[HEADER + 8..HEADER + 16].copy_from_slice(&8u64.to_le_bytes());
        assert!(parse(&wrong).unwrap_err().contains("应该是 16"));
        let mut long = good.clone();
        long.extend([0; 16]);
        long[HEADER + 8..HEADER + 16].copy_from_slice(&32u64.to_le_bytes());
        assert!(parse(&long).unwrap_err().contains("应该是 16"), "长了也不行");
        // 4×4 最多 3 级（4、2、1）；第 4 级哪怕数据齐了也不收
        let levels: Vec<Vec<u8>> = vec![vec![0; 16]; 3];
        assert!(parse(&write(Some(BlockFormat::Bc7), true, size, &levels)).is_ok());
        let mut four = write(Some(BlockFormat::Bc7), true, size, &levels);
        header_patch(&mut four, 40, 4);
        four.extend([0; LEVEL_INDEX + 16]);
        assert!(parse(&four).unwrap_err().contains("最多 3 级"));
    }

    #[test]
    fn reads_the_vk_format_numbers_from_the_spec() {
        let table = [
            (37, None), (43, None),
            (133, Some((BlockFormat::Bc1, false))), (134, Some((BlockFormat::Bc1, true))),
            (137, Some((BlockFormat::Bc3, false))), (138, Some((BlockFormat::Bc3, true))),
            (145, Some((BlockFormat::Bc7, false))), (146, Some((BlockFormat::Bc7, true))),
            (151, Some((BlockFormat::Etc2Rgba8, false))), (152, Some((BlockFormat::Etc2Rgba8, true))),
            (157, Some((BlockFormat::Astc4x4, false))), (158, Some((BlockFormat::Astc4x4, true))),
            (165, Some((BlockFormat::Astc6x6, false))), (166, Some((BlockFormat::Astc6x6, true))),
            (171, Some((BlockFormat::Astc8x8, false))), (172, Some((BlockFormat::Astc8x8, true))),
        ];
        for (vk, want) in table {
            // 24×24：每种块都对齐，每级字节数够用（取最大的 RGBA8）
            let size = UVec2::splat(24);
            let level = want.map_or(24 * 24 * 4, |(f, _): (BlockFormat, bool)| f.level_bytes(size));
            let mut bytes = write(None, true, UVec2::new(level as u32 / 4, 1), &[vec![0; level]]);
            header_patch(&mut bytes, 12, vk);
            header_patch(&mut bytes, 20, 24);
            header_patch(&mut bytes, 24, 24);
            let image = parse(&bytes).unwrap_or_else(|e| panic!("{vk}: {e}"));
            assert_eq!(image.compressed.map(|c| (c.format, c.srgb)), want, "vkFormat {vk}");
        }
    }

    #[test]
    fn broken_astc_is_an_error_not_a_panic() {
        // texture2ddecoder 解这块会越界 / 溢出 panic
        let bad = [0xb4, 0xf9, 0x74, 0x9e, 0x5d, 0xe5, 0x7e, 0x26, 0xe4, 0x3d, 0x3d, 0xc1, 0xf6, 0x19, 0x87, 0x0e];
        for format in [BlockFormat::Astc4x4, BlockFormat::Astc6x6, BlockFormat::Astc8x8] {
            let c = Compressed { format, srgb: true, levels: vec![bad.repeat(4)] };
            let size = format.block() * 2;
            // 有的格式能解出（错的）颜色，有的会 panic；都不能把调用方带崩
            if let Err(e) = c.decode_rgba(size) {
                assert!(e.contains("坏了") || e.contains("失败"), "{e}");
            }
        }
        let panics = Compressed { format: BlockFormat::Astc4x4, srgb: true, levels: vec![bad.to_vec()] };
        assert!(panics.decode_rgba(UVec2::splat(4)).unwrap_err().contains("坏了"));
    }

    #[test]
    fn writes_levels_aligned_from_smallest_to_largest() {
        // 三级：索引到 152 结束，不是 16 的倍数，要补齐
        let size = UVec2::new(8, 4);
        let levels: Vec<Vec<u8>> = (0..3).map(|l| vec![l as u8 + 1; BlockFormat::Bc3.level_bytes(level_size(size, l))]).collect();
        let bytes = write(Some(BlockFormat::Bc3), true, size, &levels);
        let offsets: Vec<u64> = (0..3).map(|l| u64_at(&bytes, HEADER + l * LEVEL_INDEX)).collect();
        assert!(offsets.iter().all(|o| o % 16 == 0), "BC3 按 16 字节对齐：{offsets:?}");
        assert!(offsets.windows(2).all(|w| w[0] > w[1]), "大的级在后面：{offsets:?}");
        assert_eq!(offsets[2], 160);
        assert_eq!(bytes.len() as u64, offsets[0] + 32);
        let rgba = write(None, true, UVec2::new(1, 1), &[vec![9; 4]]);
        assert_eq!(u64_at(&rgba, HEADER), (HEADER + LEVEL_INDEX) as u64, "RGBA8 按 4 字节对齐");
    }

    #[test]
    fn level_count_zero_means_one_level() {
        let mut bytes = write(Some(BlockFormat::Bc1), false, UVec2::new(4, 4), &[vec![7; 8]]);
        header_patch(&mut bytes, 40, 0);
        assert_eq!(parse(&bytes).unwrap().compressed.unwrap().levels, vec![vec![7; 8]]);
    }

    #[test]
    fn decodes_on_the_cpu_when_the_gpu_cannot() {
        let red = Compressed { format: BlockFormat::Astc4x4, srgb: true, levels: vec![astc_solid([0xFFFF, 0, 0, 0xFFFF])] };
        let rgba = red.decode_rgba(UVec2::new(3, 2)).unwrap();
        assert_eq!(rgba.len(), 3 * 2 * 4);
        assert!(rgba.chunks(4).all(|p| p == [255, 0, 0, 255]), "{rgba:?}");
        let big = Compressed { format: BlockFormat::Astc8x8, srgb: true, levels: vec![astc_solid([0, 0xFFFF, 0, 0x8080])] };
        assert!(big.decode_rgba(UVec2::new(8, 8)).unwrap().chunks(4).all(|p| p == [0, 255, 0, 128]));
        // BC3：alpha 两个端点都是 0x80、颜色两个端点都是纯蓝（RGB565 0x001F）
        let bc3 = [0x80, 0x80, 0, 0, 0, 0, 0, 0, 0x1F, 0x00, 0x1F, 0x00, 0, 0, 0, 0];
        let blue = Compressed { format: BlockFormat::Bc3, srgb: true, levels: vec![bc3.to_vec()] };
        assert!(blue.decode_rgba(UVec2::new(4, 4)).unwrap().chunks(4).all(|p| p == [0, 0, 255, 128]));
        // BC1：两个端点都是纯红（RGB565 0xF800），不透明
        let bc1 = Compressed { format: BlockFormat::Bc1, srgb: true, levels: vec![vec![0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0]] };
        assert!(bc1.decode_rgba(UVec2::new(4, 4)).unwrap().chunks(4).all(|p| p == [255, 0, 0, 255]));
        // 第 1 级：2×2 的 BC1 也是一整块
        let mips = Compressed { format: BlockFormat::Bc1, srgb: true, levels: vec![vec![0; 8], vec![0xE0, 0x07, 0xE0, 0x07, 0, 0, 0, 0]] };
        assert_eq!(mips.decode_level(UVec2::new(4, 3), 1).unwrap(), [0, 255, 0, 255].repeat(2), "4×3 的第 1 级是 2×1");
        assert!(mips.decode_level(UVec2::new(4, 3), 2).unwrap_err().contains("没有第 2 级"));
        let empty = Compressed { format: BlockFormat::Bc1, srgb: true, levels: vec![] };
        assert!(empty.decode_rgba(UVec2::ONE).is_err(), "没有级不 panic");
        let short = Compressed { format: BlockFormat::Bc7, srgb: true, levels: vec![vec![0; 8]] };
        assert!(short.decode_rgba(UVec2::new(4, 4)).is_err());
        let one_row = Compressed { format: BlockFormat::Bc7, srgb: true, levels: vec![vec![0; 16]] };
        assert!(one_row.decode_rgba(UVec2::new(4, 8)).unwrap_err().contains("不够"), "两行块只给了一行");
    }
}
