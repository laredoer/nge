# 压缩纹理：KTX2、ASTC / BC / ETC2

> 前置：资源加载（[m5-extensions.md](m5-extensions.md)）。

## 目标

贴图现在只认 PNG / JPEG，加载后解成 RGBA8 上传：一张 2048×2048 的图占 16 MB 显存，手机上贴图一多就紧张，解码 PNG 也慢。GPU 能直接用的块压缩格式（手机上是 ASTC 和 ETC2，桌面是 BC）只有它的 1/4 到 1/8，还自带 mipmap。

**验收标准**：
- 贴图可以是 KTX2 文件（和 PNG 一样用 `asset:/` 路径，按文件头认）：ASTC（4×4、6×6、8×8）、BC1、BC3、BC7、ETC2 RGBA8，以及不压缩的 RGBA8；带 mipmap 的全部用上。
- 设备支持这种格式时直接上传压缩数据；不支持时（比如不支持 BC 的手机、Intel Mac 上的 ASTC）在 CPU 上把每一级解成 RGBA8 再上传，画出来一样（mipmap 也在），只是没省显存。
- 文件坏了、格式不支持（超压缩、立方体贴图、3D 贴图、数组、不带透明度的 BC1）时报清楚的错，不崩；块数据本身坏了，回退解码时用白色代替，也不崩。
- 有一个把 PNG 转成 KTX2（BC3 + mipmap）的小工具。
- 平台跳跃的远山贴图换成 KTX2。

## 做法

### `nge-assets`

- `Image` 多一个字段 `compressed: Option<Compressed>`：
  - `Compressed { format: BlockFormat, srgb: bool, levels: Vec<Vec<u8>> }`，`levels[0]` 是原尺寸，往后每级边长减半（不小于 1）。
  - `BlockFormat`：`Bc1`、`Bc3`、`Bc7`、`Etc2Rgba8`、`Astc4x4`、`Astc6x6`、`Astc8x8`；能查块的宽高和字节数。
  - 有 `compressed` 时 `rgba` 是空的；`size` 照样是像素尺寸，精灵按它算大小。
- `Image::decode` 看文件头：KTX2 走 `ktx2::parse`，别的交给 `image` 解 PNG / JPEG。
- `ktx2::parse(bytes)`：
  - 读 80 字节的头和每一级的索引，按 `vkFormat` 认格式（RGBA8、BC1、BC3、BC7、ETC2 RGBA8、ASTC 4×4 / 6×6 / 8×8，各自的 UNORM 和 SRGB）。
  - 不支持：超压缩（`supercompressionScheme` 不是 0）、`pixelDepth` / `layerCount` 不是 0、`faceCount` 不是 1、别的格式。错误信息带上是哪一项。
  - 每级数据的位置和长度都检查：越界、长度和块数对不上都报错。`levelCount` 是 0（要求加载时生成 mipmap）当 1 级；比完整的 mipmap 链（减半到 1×1）还多也报错。
  - RGBA8 的 KTX2 变成普通的 RGBA 图（只取第 0 级）；不论 UNORM 还是 SRGB 都当 sRGB 用，和 PNG 一样。
  - 不带透明度的 BC1（`vkFormat` 131 / 132）不收：它三色模式下的第 4 种颜色是不透明黑，wgpu 只有带透明度的 BC1（那里是透明黑），会在暗处变成透明的洞。报错让改成 BC1 RGBA 或 BC7。
  - 压缩格式的 UNORM / SRGB 照文件来：直接上传和回退解码都用对应的 UNORM 或 sRGB 纹理，两条路画出来一样。UNORM 的压缩贴图打一条警告（颜色贴图当线性的采会发白；以后法线贴图要用 UNORM）。
  - 级数先和完整的 mipmap 链比，再算索引有多长，32 位平台上不会溢出。
- `ktx2::write(format, srgb, size, levels)`：写一个引擎能读的 KTX2（工具和测试用）。每级数据按规范对齐（压缩格式按块字节数，RGBA8 按 4），从最小的一级往大排；不写数据格式描述（DFD），`ktx validate` 这类工具不认。
- `Compressed::decode_level(size, level)`：用 `texture2ddecoder` 把某一级解成 RGBA8（`decode_rgba` 是第 0 级）。
  - 一行块一行块地解，内存只比结果多一行块。
  - `texture2ddecoder` 遇到坏的 ASTC 块会 panic（读到块后面、减法溢出），用 `catch_unwind` 接住，变成错误（panic 的信息还会打到 stderr）。
  - 没有这一级、数据不够时返回错误。
- 上传（`GpuImages`）：
  - 压缩格式、设备有对应的特性（`TEXTURE_COMPRESSION_ASTC` / `_BC` / `_ETC2`）、宽高是块的整数倍：建对应格式的纹理，每一级按块写进去。
  - 否则在上传时把每一级解成 RGBA8，建同样级数的 RGBA8 纹理，打一条信息日志说明用了回退。解码在渲染线程上，大图第一次画时会卡一下（8192² 的 ASTC 要解好几秒，内存多出 256 MB 的结果）。解不出来时记一条错误，用白色代替。
- 设备（`nge-core`）：建设备时把适配器支持的这三个压缩特性都要上（`compression_features`）。
- 采样器没改：mipmap 级之间不插值（取最近的一级），缩小时已经比没有 mipmap 平滑。
- 图集（`grid` 的精灵表）带 mipmap 时，缩小后低的级会混进相邻的格子：转图集用 `--no-mips`。

### 工具

- `cargo run -p nge-assets --example ktx2 -- [--no-mips] 输入.png 输出.ktx2`：缩成一直到 1×1 的 mipmap 链（2×2 盒式滤波、四舍五入；奇数边丢掉最后一列 / 行，边长是 1 的那边重复用），每级用 `texpresso` 压成 BC3（带透明度），写成 KTX2（sRGB）。宽高不是 4 的倍数时提示一句（GPU 用不了，加载时会回退解码）。
- `--no-mips` 只要第 0 级（图集用）。
- mipmap 直接在 sRGB 空间对没预乘的颜色求平均：暗部偏暗、半透明边上可能有色边。远山是纯白剪影，看不出来。
- 同一张 PNG 转出来的文件每次一样。
- 正式做手机游戏要用 ASTC：用 `astcenc` 或 `toktx` 生成（不带超压缩），引擎直接读。

### 示例（平台跳跃）

- `assets/hills.ktx2`：远山贴图转出来的 BC3（256×160，9 级 mipmap），Mac 上直接用压缩纹理，iOS 上回退解码。场景里的路径换成 `.ktx2`。
- PNG 源图挪到 `tools/hills.png`（`tools/make_hills.py` 生成），不打包。
- 文件比 PNG 大（55 KB 对 1.5 KB）：块压缩是给 GPU 用的固定码率，不像 PNG 那样按内容压；省的是显存（160 KB 的 RGBA8 变成 40 KB 加 mipmap）和加载时的解码。

## 不做

- Basis Universal / UASTC 转码、zstd 超压缩。
- 立方体贴图、数组贴图、3D 贴图。
- 设备不支持时转成另一种压缩格式（只回退到 RGBA8）。
- 在加载线程上按设备挑格式、回退解码放到加载线程（现在在渲染线程上）、同一张图带好几种格式。
- 写 DFD、生成别的工具能验证通过的 KTX2。
- 线性空间、预乘透明度的 mipmap。
- 给 PNG 自动生成 mipmap。

## 修过的问题

- 坏的 ASTC 块在回退解码时让 `texture2ddecoder` panic，渲染线程跟着崩；用 `catch_unwind` 接住，变成错误，用白色代替。
- 不带透明度的 BC1 按带透明度的解，暗处成了透明的洞；不收了。
- 回退解码只解第 0 级，缩小时和直接上传的不一样；每一级都解。
- 回退解码先解成一整张 `u32` 再换成字节，峰值内存是结果的两倍；改成一行块一行块地解。
- 级数很大时先算索引长度，32 位平台上会溢出；先和完整的 mipmap 链比。
- 写出的 KTX2 每级没对齐、顺序反了；按规范对齐、从小到大排。
- 工具对图集也生成 mipmap；加 `--no-mips`。

## 验收

- `ktx2` 单元测试：
  - 读写往返（各格式、多级）；每个 `vkFormat` 编号对应的格式和 sRGB。
  - 头不对、超压缩、立方体、深度、数组、格式不支持、不带透明度的 BC1、级数比完整链多、索引超出文件、级数据越界、长度短了或长了都报错；`levelCount` 0 当 1 级；RGBA8 变成普通图。
  - 写出的每级对齐、从小到大排。
- 解码回退：纯色的 ASTC 块（void-extent）、BC1、BC3 块解出来的颜色对；第 1 级的尺寸对；没有的级、数据不够、坏的 ASTC 块返回错误不 panic。
- 格式选择：有特性、尺寸对齐才直接上传；每种格式映射到块大小、字节数、sRGB 一样的 wgpu 格式；设备只要适配器有的压缩特性。
- 离屏测试（有 GPU 时，本机有 BC，两条路都跑到）：要了特性的设备和没要的设备画出来一样；UNORM 的回退也按 UNORM；缩小时回退那边也用 mipmap。
- 工具：PNG 转出来的 KTX2 能读回来，级数和尺寸对，高的图也减半到 1×1；`--no-mips` 只有一级；缩小时四舍五入、边长是 1 的那边重复用。
- 平台跳跃：远山是 9 级的 BC3，PNG 源图不打包。
- 变异检查：块大小、BC1 的 8 字节、不满一块按一块、各 `vkFormat` 的 UNORM / SRGB、头的每项检查、级数上限、长度不对、偏移溢出、级尺寸不小于 1、BC1 解码、BGRA 换序、写出的 sRGB、对齐、要特性、回退的 sRGB、回退的 mipmap、解不出来时用白色、接住 panic、数据不够、最后一行块的高度、不收不带透明度的 BC1、写出的对齐和顺序、`--no-mips`、mip 级数、只要适配器有的特性、工具的四舍五入 / 边 / 停在 1×1，改掉都有测试报错。
- 还没手测：不支持 BC 的设备上回退解码的样子和耗时、远山在画面上的样子。
