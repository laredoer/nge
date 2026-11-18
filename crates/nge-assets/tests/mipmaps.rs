//! PNG 自动 mipmap：在 GPU 上生成 mip 链再读回来，和 CPU 上按同样规则算的比。没有 GPU 时跳过。

use glam::UVec2;
use nge_assets::{Assets, BlockFormat, GpuImages, Image, ImageId, ktx2};
use nge_core::{Gfx, Viewport, wgpu};

fn gfx() -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC, ..Default::default() })).ok()?;
    let viewport = Viewport { size: [16.0; 2], pixels: [16; 2] };
    Some(Gfx { screen: None, pipeline_cache: None, device, queue, format: wgpu::TextureFormat::Rgba8UnormSrgb, viewport })
}

fn png(size: UVec2, rgba: &[u8]) -> Image {
    let mut bytes = Vec::new();
    image::RgbaImage::from_raw(size.x, size.y, rgba.to_vec()).unwrap().write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    Image::decode(&bytes).unwrap()
}

/// 上传后读回第 `level` 级的像素（原样的字节，sRGB 贴图就是 sRGB 编码）。
fn read_level(gfx: &Gfx, images: &GpuImages, id: ImageId, level: u32) -> Vec<[u8; 4]> {
    let texture = images.view(id).expect("上传了").texture().clone();
    let size = UVec2::new((texture.width() >> level).max(1), (texture.height() >> level).max(1));
    let row = (size.x * 4).div_ceil(256) * 256;
    let buffer = gfx.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(row * size.y), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = gfx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
        wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
    );
    gfx.queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    gfx.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = buffer.get_mapped_range(..).expect("mapped");
    (0..size.y).flat_map(|y| (0..size.x).map(move |x| (y, x))).map(|(y, x)| {
        let at = (y * row + x * 4) as usize;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }).collect()
}

fn to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(c: f32) -> u8 {
    let c = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// CPU 上的参考：2×2 盒式（越界的取边上的），线性空间里按 alpha 加权。
fn reference(size: UVec2, rgba: &[[u8; 4]]) -> (UVec2, Vec<[u8; 4]>) {
    let next = UVec2::new((size.x / 2).max(1), (size.y / 2).max(1));
    let px = |x: u32, y: u32| rgba[(y.min(size.y - 1) * size.x + x.min(size.x - 1)) as usize];
    let out = (0..next.y)
        .flat_map(|y| (0..next.x).map(move |x| (x, y)))
        .map(|(x, y)| {
            let four = [px(2 * x, 2 * y), px(2 * x + 1, 2 * y), px(2 * x, 2 * y + 1), px(2 * x + 1, 2 * y + 1)];
            let a: f32 = four.iter().map(|p| f32::from(p[3]) / 255.0).sum();
            let channel = |i: usize| {
                if a > 0.0 {
                    four.iter().map(|p| to_linear(p[i]) * f32::from(p[3]) / 255.0).sum::<f32>() / a
                } else {
                    four.iter().map(|p| to_linear(p[i])).sum::<f32>() / 4.0
                }
            };
            [to_srgb(channel(0)), to_srgb(channel(1)), to_srgb(channel(2)), ((a / 4.0) * 255.0).round() as u8]
        })
        .collect();
    (next, out)
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2)
}

fn upload(gfx: &Gfx, image: Image, setup: impl FnOnce(&mut GpuImages, ImageId)) -> (GpuImages, ImageId) {
    let mut images = Assets::<Image>::with_white();
    let id = images.add(image);
    let mut gpu = GpuImages::default();
    setup(&mut gpu, id);
    gpu.sync(gfx, &images);
    (gpu, id)
}

#[test]
fn png_textures_get_a_full_mip_chain_matching_the_cpu_reference() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    // 6×3：奇数边、各种颜色和透明度
    let size = UVec2::new(6, 3);
    let pixels: Vec<[u8; 4]> = (0..18u32).map(|i| [(i * 37 % 256) as u8, (i * 91 % 256) as u8, (i * 13 % 256) as u8, if i % 5 == 0 { 0 } else { (i * 53 % 256) as u8 }]).collect();
    let (gpu, id) = upload(&gfx, png(size, pixels.concat().as_slice()), |_, _| {});
    let texture = gpu.view(id).unwrap().texture().clone();
    assert_eq!(texture.mip_level_count(), 3, "6×3 → 3×1 → 1×1");
    assert_eq!(read_level(&gfx, &gpu, id, 0), pixels, "第 0 级原样");
    let (mut s, mut expected) = (size, pixels);
    for level in 1..3 {
        (s, expected) = reference(s, &expected);
        let got = read_level(&gfx, &gpu, id, level);
        assert_eq!(got.len(), expected.len());
        for (g, e) in got.iter().zip(&expected) {
            assert!(close(*g, *e), "第 {level} 级：GPU {g:?}，参考 {e:?}");
        }
    }
}

#[test]
fn averaging_happens_in_linear_space() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let (gpu, id) = upload(&gfx, png(UVec2::new(2, 1), &[0, 0, 0, 255, 255, 255, 255, 255]), |_, _| {});
    let p = read_level(&gfx, &gpu, id, 1)[0];
    assert!((186..=190).contains(&p[0]), "黑白平均在线性空间是 0.5，sRGB 编码后约 188，不是 128：{p:?}");
}

#[test]
fn transparent_pixels_do_not_darken_the_edges() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    // 一个不透明的红，三个全透明的蓝（透明处的颜色是随便存的）
    let (gpu, id) = upload(&gfx, png(UVec2::new(2, 2), &[255, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0]), |_, _| {});
    let p = read_level(&gfx, &gpu, id, 1)[0];
    assert!(p[0] >= 253 && p[2] <= 2 && (62..=66).contains(&p[3]), "颜色只算不透明的，alpha 平均：{p:?}");
}

#[test]
fn mipmaps_can_be_turned_off_per_texture() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let image = || png(UVec2::splat(8), &[200; 8 * 8 * 4]);
    let (gpu, id) = upload(&gfx, image(), |gpu, id| gpu.set_mipmaps(id, Some(false)));
    assert_eq!(gpu.view(id).unwrap().texture().mip_level_count(), 1, "像素风：关掉");
    let mut plain = image();
    plain.mipmaps = false;
    let (gpu, id) = upload(&gfx, plain, |_, _| {});
    assert_eq!(gpu.view(id).unwrap().texture().mip_level_count(), 1, "Image 自己不要");
    let mut plain = image();
    plain.mipmaps = false;
    let (gpu, id) = upload(&gfx, plain, |gpu, id| gpu.set_mipmaps(id, Some(true)));
    assert_eq!(gpu.view(id).unwrap().texture().mip_level_count(), 4, "设置优先");
}

#[test]
fn generated_and_white_textures_stay_single_level() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let (gpu, id) = upload(&gfx, Image::from_rgba(UVec2::splat(4), vec![255; 64]).unwrap(), |_, _| {});
    assert_eq!(gpu.view(id).unwrap().texture().mip_level_count(), 1, "程序生成的（字形图集）不动");
    assert_eq!(gpu.view(ImageId::WHITE).unwrap().texture().mip_level_count(), 1);
}

#[test]
fn compressed_textures_keep_their_own_levels() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let bytes = ktx2::write(Some(BlockFormat::Bc3), true, UVec2::splat(8), &[vec![0; 64], vec![0; 16]]);
    let (gpu, id) = upload(&gfx, Image::decode(&bytes).unwrap(), |gpu, id| gpu.set_mipmaps(id, Some(true)));
    assert_eq!(gpu.view(id).unwrap().texture().mip_level_count(), 2, "文件里两级，不补");
    let one = ktx2::write(Some(BlockFormat::Bc3), true, UVec2::splat(8), &[vec![0; 64]]);
    let (gpu, id) = upload(&gfx, Image::decode(&one).unwrap(), |_, _| {});
    assert_eq!(gpu.view(id).unwrap().texture().mip_level_count(), 1, "没带 mip 的压缩贴图也不生成");
}

#[test]
fn changing_the_switch_after_the_upload_reuploads_and_bumps_the_generation() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let mut images = Assets::<Image>::with_white();
    let id = images.add(png(UVec2::splat(4), &[9; 64]));
    let mut gpu = GpuImages::default();
    gpu.sync(&gfx, &images);
    let (before, generation) = (gpu.view(id).unwrap().texture().clone(), gpu.generation());
    assert_eq!(before.mip_level_count(), 3);
    gpu.set_mipmaps(id, Some(true));
    gpu.sync(&gfx, &images);
    assert_eq!((gpu.view(id).unwrap().texture(), gpu.generation()), (&before, generation), "和原来一样的不重传");
    gpu.set_mipmaps(id, Some(false));
    gpu.sync(&gfx, &images);
    assert_eq!(gpu.view(id).unwrap().texture().mip_level_count(), 1, "关掉以后重传");
    assert_ne!(gpu.generation(), generation, "渲染器看到代数变了就丢掉缓存的绑定组");
}

#[test]
fn normal_maps_average_the_stored_values() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let image = || png(UVec2::new(2, 1), &[0, 0, 0, 255, 255, 255, 255, 255]);
    let (gpu, id) = upload(&gfx, image(), |gpu, id| gpu.set_linear(id, true));
    let p = read_level(&gfx, &gpu, id, 1)[0];
    assert!((126..=129).contains(&p[0]), "法线图存的是数据，不按 sRGB 解码再平均：{p:?}");
    assert_eq!(read_level(&gfx, &gpu, id, 0), [[0, 0, 0, 255], [255, 255, 255, 255]], "第 0 级原样");
    // 上传以后才知道是法线图：重传
    let mut images = Assets::<Image>::with_white();
    let id = images.add(image());
    let mut gpu = GpuImages::default();
    gpu.sync(&gfx, &images);
    gpu.set_linear(id, true);
    gpu.sync(&gfx, &images);
    let p = read_level(&gfx, &gpu, id, 1)[0];
    assert!((126..=129).contains(&p[0]), "{p:?}");
}
