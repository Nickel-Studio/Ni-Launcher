//! 离线皮肤的有界 PNG 解码与 UV 适配；复用 flate2，不引入图片依赖。
//! 资源包不能改变玩家几何模型，因此分别生成经典 / 纤细纹理，不改 UUID。

use std::fs::File;
use std::io::Write;
use std::path::Path;

use flate2::{write::ZlibEncoder, Compression, Decompress, FlushDecompress, Status};

use crate::net::transfer::CancellationToken;
use crate::utils::archive::{copy_archive_entry, validate_relative_path};

pub const MAX_SKIN_PNG_BYTES: usize = 1024 * 1024;
pub const STEVE_TEXTURE: &str = "assets/minecraft/textures/entity/steve.png";
pub const ALEX_TEXTURE: &str = "assets/minecraft/textures/entity/alex.png";
pub const MODERN_STEVE_TEXTURE: &str = "assets/minecraft/textures/entity/player/wide/steve.png";
pub const MODERN_ALEX_TEXTURE: &str = "assets/minecraft/textures/entity/player/slim/alex.png";
const PLAYER_WIDE_PREFIX: &str = "assets/minecraft/textures/entity/player/wide/";
const PLAYER_SLIM_PREFIX: &str = "assets/minecraft/textures/entity/player/slim/";
const MAX_VERSION_JSON_BYTES: u64 = 64 * 1024;
const MAX_DEFAULT_SKIN_TEXTURES: usize = 128;
const WIDTH: usize = 64;
const MODERN_HEIGHT: usize = 64;
const LEGACY_HEIGHT: usize = 32;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinImage {
    height: usize,
    pixels: Vec<[u8; 4]>,
}

impl SkinImage {
    pub fn height(&self) -> usize {
        self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkinPackFormat {
    pub major: u32,
    pub minor: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinTextureTarget {
    pub path: String,
    pub slim: bool,
}

#[derive(Debug)]
pub struct ClientSkinAssets {
    pub pack_format: Option<SkinPackFormat>,
    pub targets: Vec<SkinTextureTarget>,
}

fn read_client_jar_entry(
    archive: &mut zip::ZipArchive<File>,
    entry: &str,
    limit: u64,
    cancel: &CancellationToken,
) -> Result<Option<Vec<u8>>, String> {
    cancel.check()?;
    let mut file = match archive.by_name(entry) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(format!("客户端 JAR 条目 {entry} 无法读取：{e}")),
    };
    if file.is_dir() {
        return Err(format!("客户端 JAR 条目 {entry} 不是文件"));
    }
    let expected = file.size();
    if expected > limit {
        return Err(format!(
            "客户端 JAR 条目 {entry} 超过大小限制（{limit} 字节）"
        ));
    }
    let mut bytes = Vec::new();
    // 不能只信任目录声明；读到 EOF 以校验 CRC，同时约束实际解压输出。
    let actual = copy_archive_entry(&mut file, &mut bytes, limit, cancel)
        .map_err(|e| format!("客户端 JAR 条目 {entry} 读取失败：{e}"))?;
    if actual != expected {
        return Err(format!("客户端 JAR 条目 {entry} 长度与目录声明不一致"));
    }
    Ok(Some(bytes))
}

fn open_client_jar(client_jar: &Path) -> Result<zip::ZipArchive<File>, String> {
    let file = File::open(client_jar).map_err(|e| format!("无法打开客户端 JAR：{e}"))?;
    zip::ZipArchive::new(file).map_err(|e| format!("客户端 JAR 目录无效：{e}"))
}

/// 仅读取精确纹理条目；混淆类名可含 Windows 设备名，不能套用整包落盘路径校验。
/// 异步调用方须使用 spawn_blocking，并显式携带当前启动任务的取消令牌。
pub fn read_skin_png_from_jar(
    client_jar: &Path,
    entry: &str,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, String> {
    cancel.check()?;
    let mut archive = open_client_jar(client_jar)?;
    let bytes = read_client_jar_entry(&mut archive, entry, MAX_SKIN_PNG_BYTES as u64, cancel)?
        .ok_or_else(|| format!("客户端 JAR 缺少皮肤条目 {entry}"))?;
    decode_skin_png(&bytes)?;
    Ok(bytes)
}

fn skin_pack_format(bytes: &[u8]) -> Result<SkinPackFormat, String> {
    let version: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("客户端 version.json 损坏：{e}"))?;
    let pack = &version["pack_version"];
    let number = |value: &serde_json::Value| {
        value
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| "客户端 version.json 的资源包版本无效".to_string())
    };
    // 客户端元数据经历整数、resource/data 分离、major/minor 三种实际格式。
    let format = if pack.is_number() {
        SkinPackFormat {
            major: number(pack)?,
            minor: None,
        }
    } else if pack.get("resource_major").is_some() {
        SkinPackFormat {
            major: number(&pack["resource_major"])?,
            minor: Some(number(&pack["resource_minor"])?),
        }
    } else {
        SkinPackFormat {
            major: number(&pack["resource"])?,
            minor: None,
        }
    };
    if format.major == 0 {
        return Err("客户端 version.json 的资源包版本必须大于零".into());
    }
    Ok(format)
}

/// 从安装的客户端读取真实格式和默认玩家纹理，现代版本与快照不依赖版本号表。
pub fn read_client_skin_assets(
    client_jar: &Path,
    cancel: &CancellationToken,
) -> Result<ClientSkinAssets, String> {
    cancel.check()?;
    let mut archive = open_client_jar(client_jar)?;
    let pack_format =
        read_client_jar_entry(&mut archive, "version.json", MAX_VERSION_JSON_BYTES, cancel)?
            .as_deref()
            .map(skin_pack_format)
            .transpose()?;
    let mut targets = Vec::new();
    for name in archive.file_names() {
        cancel.check()?;
        let name = name.map_err(|e| format!("客户端 JAR 条目名称无效：{e}"))?;
        let slim = if name == STEVE_TEXTURE {
            false
        } else if name == ALEX_TEXTURE {
            true
        } else if let Some((file, slim)) = name
            .strip_prefix(PLAYER_WIDE_PREFIX)
            .map(|f| (f, false))
            .or_else(|| name.strip_prefix(PLAYER_SLIM_PREFIX).map(|f| (f, true)))
        {
            let Some(stem) = file.strip_suffix(".png") else {
                continue;
            };
            if stem.is_empty()
                || !stem
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            {
                continue;
            }
            slim
        } else {
            continue;
        };
        // 只有这些将成为输出路径的纹理需要落盘路径校验；无关类文件完全不提取。
        let path = validate_relative_path(&name)?;
        targets.push(SkinTextureTarget { path, slim });
        if targets.len() > MAX_DEFAULT_SKIN_TEXTURES {
            return Err("客户端默认皮肤条目过多，请重新安装原版客户端".into());
        }
    }
    targets.sort_by(|a, b| a.path.cmp(&b.path));
    if targets.is_empty() {
        return Err(
            "客户端 JAR 中没有默认玩家皮肤纹理，请重新安装原版客户端或将离线皮肤改为“随机”".into(),
        );
    }
    Ok(ClientSkinAssets {
        pack_format,
        targets,
    })
}

fn invalid(reason: &str) -> String {
    format!("皮肤 PNG 无效（{reason}），请重新导出 64×64 或 64×32 的 PNG 图片")
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn u32_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn channels(color: u8) -> Option<usize> {
    match color {
        0 | 3 => Some(1),
        2 => Some(3),
        4 => Some(2),
        6 => Some(4),
        _ => None,
    }
}

fn valid_depth(color: u8, depth: u8) -> bool {
    match color {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(depth, 8 | 16),
        _ => false,
    }
}

fn pass_len(size: usize, start: usize, step: usize) -> usize {
    size.saturating_sub(start).div_ceil(step)
}

fn paeth(left: u8, up: u8, corner: u8) -> u8 {
    let p = i32::from(left) + i32::from(up) - i32::from(corner);
    let a = (p - i32::from(left)).abs();
    let b = (p - i32::from(up)).abs();
    let c = (p - i32::from(corner)).abs();
    if a <= b && a <= c {
        left
    } else if b <= c {
        up
    } else {
        corner
    }
}

fn unfilter(filter: u8, row: &mut [u8], previous: &[u8], bpp: usize) -> Result<(), String> {
    for i in 0..row.len() {
        let left = if i >= bpp { row[i - bpp] } else { 0 };
        let up = previous[i];
        let corner = if i >= bpp { previous[i - bpp] } else { 0 };
        let predictor = match filter {
            0 => 0,
            1 => left,
            2 => up,
            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
            4 => paeth(left, up, corner),
            _ => return Err(invalid("未知扫描行滤波器")),
        };
        row[i] = row[i].wrapping_add(predictor);
    }
    Ok(())
}

fn sample(row: &[u8], index: usize, depth: u8) -> u16 {
    if depth == 16 {
        u16::from_be_bytes([row[index * 2], row[index * 2 + 1]])
    } else if depth == 8 {
        u16::from(row[index])
    } else {
        let bit = index * usize::from(depth);
        let shift = 8 - usize::from(depth) - bit % 8;
        u16::from((row[bit / 8] >> shift) & ((1 << depth) - 1))
    }
}

fn decode_pixel(
    row: &[u8],
    x: usize,
    color: u8,
    depth: u8,
    palette: &[[u8; 3]],
    transparency: &[u8],
) -> Result<[u8; 4], String> {
    let count = channels(color).ok_or_else(|| invalid("颜色类型不支持"))?;
    let value = |channel| sample(row, x * count + channel, depth);
    let scale = |v: u16| ((u32::from(v) * 255) / ((1_u32 << depth) - 1)) as u8;
    let transparent_sample =
        |offset| u16::from_be_bytes([transparency[offset], transparency[offset + 1]]);
    Ok(match color {
        0 => {
            let grey = value(0);
            let alpha = if transparency.len() == 2 && grey == transparent_sample(0) {
                0
            } else {
                255
            };
            [scale(grey), scale(grey), scale(grey), alpha]
        }
        2 => {
            let alpha = if transparency.len() == 6
                && (0..3).all(|i| value(i) == transparent_sample(i * 2))
            {
                0
            } else {
                255
            };
            [scale(value(0)), scale(value(1)), scale(value(2)), alpha]
        }
        3 => {
            let index = usize::from(value(0));
            let rgb = palette
                .get(index)
                .ok_or_else(|| invalid("调色板索引越界"))?;
            [
                rgb[0],
                rgb[1],
                rgb[2],
                transparency.get(index).copied().unwrap_or(255),
            ]
        }
        4 => [
            scale(value(0)),
            scale(value(0)),
            scale(value(0)),
            scale(value(1)),
        ],
        6 => [
            scale(value(0)),
            scale(value(1)),
            scale(value(2)),
            scale(value(3)),
        ],
        _ => return Err(invalid("颜色类型不支持")),
    })
}

/// 校验全部 chunk CRC、zlib 数据和像素布局，不以 PNG 魔数代替有效性验证。
pub fn decode_skin_png(bytes: &[u8]) -> Result<SkinImage, String> {
    if bytes.len() > MAX_SKIN_PNG_BYTES {
        return Err("皮肤图片超过 1 MiB，请重新导出 64×64 或 64×32 的 PNG 图片".into());
    }
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(invalid("文件头不正确"));
    }
    let mut pos = PNG_SIGNATURE.len();
    let mut header = None;
    let mut palette = Vec::new();
    let mut transparency = Vec::new();
    let mut palette_seen = false;
    let mut transparency_seen = false;
    let mut idat_seen = false;
    let mut idat_ended = false;
    let mut ended = false;
    let mut compressed = Vec::new();
    while pos < bytes.len() {
        if bytes.len() - pos < 12 {
            return Err(invalid("数据块被截断"));
        }
        let length = u32_be(&bytes[pos..pos + 4]) as usize;
        let end = pos
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .filter(|&n| n <= bytes.len())
            .ok_or_else(|| invalid("数据块长度越界"))?;
        let kind = &bytes[pos + 4..pos + 8];
        let data = &bytes[pos + 8..end - 4];
        if !kind.iter().all(u8::is_ascii_alphabetic) || kind[2] & 0x20 != 0 {
            return Err(invalid("数据块名称不正确"));
        }
        if crc32(&bytes[pos + 4..end - 4]) != u32_be(&bytes[end - 4..end]) {
            return Err(invalid("数据块校验失败"));
        }
        if header.is_none() && kind != b"IHDR" {
            return Err(invalid("缺少图片头"));
        }
        if idat_seen && kind != b"IDAT" {
            idat_ended = true;
        }
        match kind {
            b"IHDR" => {
                if header.is_some() || data.len() != 13 {
                    return Err(invalid("图片头重复或长度错误"));
                }
                let width = u32_be(&data[..4]);
                let height = u32_be(&data[4..8]) as usize;
                if width != WIDTH as u32 || !matches!(height, LEGACY_HEIGHT | MODERN_HEIGHT) {
                    return Err(invalid("尺寸必须为 64×64 或 64×32"));
                }
                if !valid_depth(data[9], data[8]) || data[10] != 0 || data[11] != 0 || data[12] > 1
                {
                    return Err(invalid("颜色深度或压缩格式不支持"));
                }
                header = Some((height, data[8], data[9], data[12]));
            }
            b"PLTE" => {
                if palette_seen
                    || idat_seen
                    || transparency_seen
                    || data.is_empty()
                    || data.len() > 768
                    || data.len() % 3 != 0
                {
                    return Err(invalid("调色板不正确"));
                }
                palette_seen = true;
                palette = data.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            }
            b"tRNS" => {
                if transparency_seen || idat_seen {
                    return Err(invalid("透明信息顺序不正确"));
                }
                transparency_seen = true;
                transparency.extend_from_slice(data);
            }
            b"IDAT" => {
                if idat_ended {
                    return Err(invalid("像素数据块不连续"));
                }
                idat_seen = true;
                compressed.extend_from_slice(data);
            }
            b"IEND" => {
                if !data.is_empty() || !idat_seen || end != bytes.len() {
                    return Err(invalid("结束块不正确"));
                }
                ended = true;
            }
            b"acTL" => return Err(invalid("请选择静态 PNG，而不是动画 PNG")),
            _ if kind[0] & 0x20 == 0 => return Err(invalid("未知关键数据块")),
            _ => {}
        }
        pos = end;
    }
    if !ended {
        return Err(invalid("缺少结束块"));
    }
    let (height, depth, color, interlace) = header.ok_or_else(|| invalid("缺少图片头"))?;
    if color == 3 && (palette.is_empty() || palette.len() > (1_usize << depth)) {
        return Err(invalid("缺少有效调色板"));
    }
    if matches!(color, 0 | 4) && palette_seen {
        return Err(invalid("灰度图片不能包含调色板"));
    }
    if transparency_seen
        && !match color {
            0 => transparency.len() == 2,
            2 => transparency.len() == 6,
            3 => palette_seen && !transparency.is_empty() && transparency.len() <= palette.len(),
            _ => false,
        }
    {
        return Err(invalid("透明信息不正确"));
    }
    if matches!(color, 0 | 2)
        && transparency
            .chunks_exact(2)
            .any(|pair| u32::from(u16::from_be_bytes([pair[0], pair[1]])) >= 1_u32 << depth)
    {
        return Err(invalid("透明颜色超出位深范围"));
    }
    let bits = channels(color).ok_or_else(|| invalid("颜色类型不支持"))? * usize::from(depth);
    let plain = [(0, 0, 1, 1)];
    let passes: &[(usize, usize, usize, usize)] = if interlace == 0 { &plain } else { &ADAM7 };
    let expected: usize = passes
        .iter()
        .map(|&(x, y, dx, dy)| {
            let w = pass_len(WIDTH, x, dx);
            let h = pass_len(height, y, dy);
            if w == 0 {
                0
            } else {
                ((w * bits).div_ceil(8) + 1) * h
            }
        })
        .sum();
    // 明确要求 StreamEnd；仅检查解压后字节数会误接收缺少 zlib 尾部的截断文件。
    let mut decoder = Decompress::new(true);
    let mut raw = vec![0; expected + 1];
    let status = decoder
        .decompress(&compressed, &mut raw, FlushDecompress::Finish)
        .map_err(|_| invalid("像素数据损坏"))?;
    if status != Status::StreamEnd
        || decoder.total_out() != expected as u64
        || decoder.total_in() != compressed.len() as u64
    {
        return Err(invalid("像素数据长度不正确或数据被截断"));
    }
    raw.truncate(expected);
    let mut image = SkinImage {
        height,
        pixels: vec![[0; 4]; WIDTH * height],
    };
    let mut cursor = 0;
    for &(x0, y0, dx, dy) in passes {
        let w = pass_len(WIDTH, x0, dx);
        let h = pass_len(height, y0, dy);
        if w == 0 || h == 0 {
            continue;
        }
        let stride = (w * bits).div_ceil(8);
        let mut previous = vec![0; stride];
        for y in 0..h {
            let mut row = raw[cursor + 1..cursor + 1 + stride].to_vec();
            unfilter(raw[cursor], &mut row, &previous, bits.div_ceil(8))?;
            for x in 0..w {
                image.pixels[(y0 + y * dy) * WIDTH + x0 + x * dx] =
                    decode_pixel(&row, x, color, depth, &palette, &transparency)?;
            }
            previous = row;
            cursor += stride + 1;
        }
    }
    Ok(image)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

pub fn encode_skin_png(image: &SkinImage) -> Result<Vec<u8>, String> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    for row in image.pixels.chunks_exact(WIDTH) {
        encoder
            .write_all(&[0])
            .map_err(|e| format!("压缩皮肤失败，请重新选择图片：{e}"))?;
        for pixel in row {
            encoder
                .write_all(pixel)
                .map_err(|e| format!("压缩皮肤失败，请重新选择图片：{e}"))?;
        }
    }
    let compressed = encoder
        .finish()
        .map_err(|e| format!("压缩皮肤失败，请重新选择图片：{e}"))?;
    let mut out = PNG_SIGNATURE.to_vec();
    let mut header = Vec::from((WIDTH as u32).to_be_bytes());
    header.extend_from_slice(&(image.height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn mirror_rect(
    source: &SkinImage,
    target: &mut SkinImage,
    src: (usize, usize),
    dst: (usize, usize),
    size: (usize, usize),
) {
    for y in 0..size.1 {
        for x in 0..size.0 {
            target.pixels[(dst.1 + y) * WIDTH + dst.0 + x] =
                source.pixels[(src.1 + y) * WIDTH + src.0 + size.0 - 1 - x];
        }
    }
}

fn modernize(image: &SkinImage) -> SkinImage {
    if image.height == MODERN_HEIGHT {
        return image.clone();
    }
    let mut output = SkinImage {
        height: MODERN_HEIGHT,
        pixels: vec![[0; 4]; WIDTH * MODERN_HEIGHT],
    };
    output.pixels[..image.pixels.len()].copy_from_slice(&image.pixels);
    // 旧布局的左肢使用右肢镜像；每个面单独镜像，不能直接翻转整块 UV。
    for (sx, sy, dx, dy, w, h) in [
        (4, 16, 20, 48, 4, 4),
        (8, 16, 24, 48, 4, 4),
        (8, 20, 16, 52, 4, 12),
        (4, 20, 20, 52, 4, 12),
        (0, 20, 24, 52, 4, 12),
        (12, 20, 28, 52, 4, 12),
        (44, 16, 36, 48, 4, 4),
        (48, 16, 40, 48, 4, 4),
        (48, 20, 32, 52, 4, 12),
        (44, 20, 36, 52, 4, 12),
        (40, 20, 40, 52, 4, 12),
        (52, 20, 44, 52, 4, 12),
    ] {
        mirror_rect(image, &mut output, (sx, sy), (dx, dy), (w, h));
    }
    output
}

fn arm_faces(x: usize, y: usize, width: usize) -> [(usize, usize, usize, usize); 6] {
    [
        (x + 4, y, width, 4),
        (x + 4 + width, y, width, 4),
        (x, y + 4, 4, 12),
        (x + 4, y + 4, width, 12),
        (x + 4 + width, y + 4, 4, 12),
        (x + 8 + width, y + 4, width, 12),
    ]
}

fn adapt_arms(image: &SkinImage, source_slim: bool, target_slim: bool) -> SkinImage {
    if source_slim == target_slim {
        return image.clone();
    }
    let mut output = image.clone();
    for (x, y) in [(40, 16), (40, 32), (32, 48), (48, 48)] {
        for row in y..y + 16 {
            output.pixels[row * WIDTH + x..row * WIDTH + x + 16].fill([0; 4]);
        }
        let source_faces = arm_faces(x, y, if source_slim { 3 } else { 4 });
        let target_faces = arm_faces(x, y, if target_slim { 3 } else { 4 });
        for ((sx, sy, sw, sh), (tx, ty, tw, _)) in source_faces.into_iter().zip(target_faces) {
            for row in 0..sh {
                for col in 0..tw {
                    let source_col = ((2 * col + 1) * sw / (2 * tw)).min(sw - 1);
                    output.pixels[(ty + row) * WIDTH + tx + col] =
                        image.pixels[(sy + row) * WIDTH + sx + source_col];
                }
            }
        }
    }
    output
}

/// 转换手臂各面的 UV；legacy 输出旧版本实际使用的 64×32 布局。
/// target_slim 仅决定纹理布局，不代表改变玩家模型或账号身份。
pub fn adapt_skin_png(
    bytes: &[u8],
    source_slim: bool,
    target_slim: bool,
    legacy: bool,
) -> Result<Vec<u8>, String> {
    let image = decode_skin_png(bytes)?;
    if image.height == LEGACY_HEIGHT && source_slim {
        return Err("64×32 皮肤使用经典手臂，请关闭纤细手臂或选择 64×64 皮肤".into());
    }
    let mut image = adapt_arms(&modernize(&image), source_slim, target_slim && !legacy);
    if legacy {
        // 旧版本没有躯干/肢体外层，合成右侧外层后裁切，保留头部外层原布局。
        for (sx, sy, dx, dy, w, h) in [
            (0, 32, 0, 16, 16, 16),
            (16, 32, 16, 16, 24, 16),
            (40, 32, 40, 16, 16, 16),
        ] {
            for y in 0..h {
                for x in 0..w {
                    let top = image.pixels[(sy + y) * WIDTH + sx + x];
                    let base = &mut image.pixels[(dy + y) * WIDTH + dx + x];
                    let a = u32::from(top[3]);
                    let b = u32::from(base[3]) * (255 - a);
                    let total = a * 255 + b;
                    if total != 0 {
                        for i in 0..3 {
                            base[i] = ((u32::from(top[i]) * a * 255 + u32::from(base[i]) * b)
                                / total) as u8;
                        }
                        base[3] = (total / 255) as u8;
                    }
                }
            }
        }
        image.height = LEGACY_HEIGHT;
        image.pixels.truncate(WIDTH * LEGACY_HEIGHT);
    }
    encode_skin_png(&image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(height: usize) -> SkinImage {
        SkinImage {
            height,
            pixels: (0..WIDTH * height)
                .map(|i| [(i % WIDTH) as u8, (i / WIDTH) as u8, 127, 255])
                .collect(),
        }
    }

    fn skin_jar(
        entries: &[(&str, &[u8])],
        method: zip::CompressionMethod,
    ) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(method);
        for &(name, bytes) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    #[test]
    fn client_skin_metadata_reads_all_three_pack_version_formats() {
        for (value, expected) in [
            (
                serde_json::json!(4),
                SkinPackFormat {
                    major: 4,
                    minor: None,
                },
            ),
            (
                serde_json::json!({"resource":12,"data":10}),
                SkinPackFormat {
                    major: 12,
                    minor: None,
                },
            ),
            (
                serde_json::json!({"resource_major":69,"resource_minor":1}),
                SkinPackFormat {
                    major: 69,
                    minor: Some(1),
                },
            ),
        ] {
            let metadata = serde_json::to_vec(&serde_json::json!({"pack_version":value})).unwrap();
            let file = skin_jar(
                &[
                    ("version.json", &metadata),
                    (MODERN_STEVE_TEXTURE, b"texture"),
                    (MODERN_ALEX_TEXTURE, b"texture"),
                    ("con.class", b"class"),
                ],
                zip::CompressionMethod::Deflated,
            );
            let assets = read_client_skin_assets(file.path(), &CancellationToken::new()).unwrap();
            assert_eq!(assets.pack_format, Some(expected));
            assert_eq!(assets.targets.len(), 2);
            assert!(assets
                .targets
                .iter()
                .any(|target| target.path == MODERN_ALEX_TEXTURE && target.slim));
            assert!(assets
                .targets
                .iter()
                .any(|target| target.path == MODERN_STEVE_TEXTURE && !target.slim));
        }
    }

    #[test]
    fn client_skin_discovery_filters_paths_and_rejects_invalid_metadata() {
        let file = skin_jar(
            &[
                (STEVE_TEXTURE, b"texture"),
                (
                    "assets/minecraft/textures/entity/player/wide/../../outside.png",
                    b"bad",
                ),
                (
                    "assets/minecraft/textures/entity/player/slim/extra_skin.png",
                    b"texture",
                ),
            ],
            zip::CompressionMethod::Stored,
        );
        let assets = read_client_skin_assets(file.path(), &CancellationToken::new()).unwrap();
        assert_eq!(assets.pack_format, None);
        assert_eq!(assets.targets.len(), 2);
        assert!(assets
            .targets
            .iter()
            .all(|target| !target.path.contains("..")));
        for metadata in [
            b"{}".as_slice(),
            b"{\"pack_version\":-1}",
            b"{\"pack_version\":0}",
            b"{\"pack_version\":{\"resource_major\":69}}",
            b"{",
        ] {
            let file = skin_jar(
                &[("version.json", metadata), (STEVE_TEXTURE, b"texture")],
                zip::CompressionMethod::Stored,
            );
            assert!(read_client_skin_assets(file.path(), &CancellationToken::new()).is_err());
        }
        let file = skin_jar(
            &[
                (
                    "version.json",
                    &vec![0; MAX_VERSION_JSON_BYTES as usize + 1],
                ),
                (STEVE_TEXTURE, b"texture"),
            ],
            zip::CompressionMethod::Deflated,
        );
        assert!(
            read_client_skin_assets(file.path(), &CancellationToken::new())
                .unwrap_err()
                .contains("大小限制")
        );
    }

    #[test]
    fn cancelled_client_skin_reads_stop_before_accessing_the_jar() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let missing = Path::new("missing-client.jar");
        assert!(read_skin_png_from_jar(missing, STEVE_TEXTURE, &cancel).is_err());
        assert!(read_client_skin_assets(missing, &cancel).is_err());
    }

    #[test]
    fn jar_skin_reader_accepts_obfuscated_classes_without_extracting_them() {
        let png = encode_skin_png(&fixture(MODERN_HEIGHT)).unwrap();
        let entry = "assets/minecraft/textures/entity/steve.png";
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let file = skin_jar(
                &[
                    ("con.class", b"class"),
                    ("aux.class", b"class"),
                    ("nul.class", b"class"),
                    (entry, &png),
                ],
                method,
            );
            let actual =
                read_skin_png_from_jar(file.path(), entry, &CancellationToken::new()).unwrap();
            assert_eq!(actual, png);
            assert_eq!(decode_skin_png(&actual).unwrap(), fixture(MODERN_HEIGHT));
        }
    }

    #[test]
    fn jar_skin_reader_rejects_crc_mismatch_oversize_and_missing_entries() {
        let png = encode_skin_png(&fixture(MODERN_HEIGHT)).unwrap();
        let entry = "assets/minecraft/textures/entity/steve.png";
        let file = skin_jar(&[(entry, &png)], zip::CompressionMethod::Stored);
        let cancel = CancellationToken::new();
        assert!(read_skin_png_from_jar(file.path(), "missing.png", &cancel).is_err());
        let original = std::fs::read(file.path()).unwrap();
        let mut corrupt = original.clone();
        let png_start = original
            .windows(PNG_SIGNATURE.len())
            .position(|bytes| bytes == PNG_SIGNATURE)
            .unwrap();
        corrupt[png_start] ^= 1;
        std::fs::write(file.path(), corrupt).unwrap();
        assert!(read_skin_png_from_jar(file.path(), entry, &cancel)
            .unwrap_err()
            .contains("读取失败"));
        let mut oversize = original.clone();
        let central = original
            .windows(4)
            .position(|bytes| bytes == b"PK\x01\x02")
            .unwrap();
        oversize[central + 24..central + 28]
            .copy_from_slice(&((MAX_SKIN_PNG_BYTES + 1) as u32).to_le_bytes());
        std::fs::write(file.path(), oversize).unwrap();
        assert!(read_skin_png_from_jar(file.path(), entry, &cancel)
            .unwrap_err()
            .contains("大小限制"));
    }

    #[test]
    fn jar_skin_reader_bounds_actual_inflate_output_when_directory_lies() {
        let entry = "assets/minecraft/textures/entity/steve.png";
        let file = skin_jar(
            &[(entry, &vec![0; MAX_SKIN_PNG_BYTES + 1])],
            zip::CompressionMethod::Deflated,
        );
        let mut bytes = std::fs::read(file.path()).unwrap();
        let central = bytes
            .windows(4)
            .position(|bytes| bytes == b"PK\x01\x02")
            .unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(file.path(), bytes).unwrap();
        // 目录声明 1 字节时，zip 9 在解压超出声明大小时即报错，早于这里的上限；
        // 两种拒绝都可以，关键是不会按虚假的目录大小读出超限数据。
        let error = read_skin_png_from_jar(file.path(), entry, &CancellationToken::new()).unwrap_err();
        assert!(
            error.contains("大小限制") || error.contains("declared uncompressed size"),
            "{error}"
        );
    }

    #[test]
    fn complete_png_roundtrip_and_corruption() {
        for height in [LEGACY_HEIGHT, MODERN_HEIGHT] {
            let image = fixture(height);
            let png = encode_skin_png(&image).unwrap();
            assert_eq!(decode_skin_png(&png).unwrap(), image);
            for length in [8, 12, png.len() - 1] {
                assert!(decode_skin_png(&png[..length]).is_err());
            }
            let mut damaged = png.clone();
            damaged[40] ^= 1;
            assert!(decode_skin_png(&damaged).is_err());
        }
        assert!(decode_skin_png(b"not PNG").is_err());
        assert!(decode_skin_png(&vec![0; MAX_SKIN_PNG_BYTES + 1]).is_err());
    }

    #[test]
    fn invalid_compressed_pixels_are_rejected_even_with_valid_chunk_crc() {
        let mut png = PNG_SIGNATURE.to_vec();
        let mut header = Vec::from((WIDTH as u32).to_be_bytes());
        header.extend_from_slice(&(MODERN_HEIGHT as u32).to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &header);
        chunk(&mut png, b"IDAT", b"not zlib");
        chunk(&mut png, b"IEND", &[]);
        assert!(decode_skin_png(&png).is_err());
    }

    fn png_with_raw(
        depth: u8,
        color: u8,
        interlace: u8,
        extra: &[(&[u8; 4], &[u8])],
        raw: &[u8],
    ) -> Vec<u8> {
        let mut header = Vec::from((WIDTH as u32).to_be_bytes());
        header.extend_from_slice(&(MODERN_HEIGHT as u32).to_be_bytes());
        header.extend_from_slice(&[depth, color, 0, 0, interlace]);
        let mut png = PNG_SIGNATURE.to_vec();
        chunk(&mut png, b"IHDR", &header);
        for &(kind, data) in extra {
            chunk(&mut png, kind, data);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(raw).unwrap();
        chunk(&mut png, b"IDAT", &encoder.finish().unwrap());
        chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn adam7_reconstructs_every_pixel() {
        let image = fixture(MODERN_HEIGHT);
        let mut raw = Vec::new();
        for (x0, y0, dx, dy) in ADAM7 {
            for y in (y0..MODERN_HEIGHT).step_by(dy) {
                raw.push(0);
                for x in (x0..WIDTH).step_by(dx) {
                    raw.extend_from_slice(&image.pixels[y * WIDTH + x]);
                }
            }
        }
        assert_eq!(
            decode_skin_png(&png_with_raw(8, 6, 1, &[], &raw)).unwrap(),
            image
        );
    }

    #[test]
    fn packed_palette_and_grayscale_transparency_are_decoded() {
        let mut raw = Vec::new();
        for _ in 0..MODERN_HEIGHT {
            raw.push(0);
            raw.extend_from_slice(&[0b0101_0101; WIDTH / 8]);
        }
        let png = png_with_raw(
            1,
            3,
            0,
            &[(b"PLTE", &[10, 20, 30, 40, 50, 60]), (b"tRNS", &[0, 255])],
            &raw,
        );
        let image = decode_skin_png(&png).unwrap();
        assert_eq!(image.pixels[0], [10, 20, 30, 0]);
        assert_eq!(image.pixels[1], [40, 50, 60, 255]);
        let png = png_with_raw(1, 0, 0, &[(b"tRNS", &[0, 1])], &raw);
        let image = decode_skin_png(&png).unwrap();
        assert_eq!(image.pixels[0], [0, 0, 0, 255]);
        assert_eq!(image.pixels[1], [255, 255, 255, 0]);
        assert!(decode_skin_png(&png_with_raw(1, 3, 0, &[], &raw)).is_err());
    }

    #[test]
    fn rgba_sixteen_bit_samples_are_scaled() {
        let mut raw = Vec::new();
        for _ in 0..MODERN_HEIGHT {
            raw.push(0);
            for _ in 0..WIDTH {
                raw.extend_from_slice(&[0xff, 0xff, 0x80, 0x80, 0, 0, 0xff, 0xff]);
            }
        }
        let image = decode_skin_png(&png_with_raw(16, 6, 0, &[], &raw)).unwrap();
        assert_eq!(image.pixels[0], [255, 128, 0, 255]);
    }

    #[test]
    fn truncated_zlib_extra_pixels_and_bad_dimensions_fail_before_use() {
        let valid = encode_skin_png(&fixture(MODERN_HEIGHT)).unwrap();
        let mut truncated = valid[..33].to_vec();
        let idat_len = u32_be(&valid[33..37]) as usize;
        chunk(&mut truncated, b"IDAT", &valid[41..41 + idat_len - 4]);
        chunk(&mut truncated, b"IEND", &[]);
        assert!(decode_skin_png(&truncated).is_err());
        let expected = (WIDTH * 4 + 1) * MODERN_HEIGHT;
        assert!(decode_skin_png(&png_with_raw(8, 6, 0, &[], &vec![0; expected + 1])).is_err());
        let mut wrong_size = PNG_SIGNATURE.to_vec();
        let mut header = valid[16..29].to_vec();
        header[..4].copy_from_slice(&128_u32.to_be_bytes());
        chunk(&mut wrong_size, b"IHDR", &header);
        wrong_size.extend_from_slice(&valid[33..]);
        assert!(decode_skin_png(&wrong_size).is_err());
    }

    #[test]
    fn all_filters_reconstruct_rows() {
        let previous = [7, 30, 80, 160, 255, 13, 87, 17];
        let original = [9, 38, 90, 200, 250, 27, 99, 3];
        for filter in 0..=4 {
            let mut encoded = original;
            for i in 0..encoded.len() {
                let left = if i >= 4 { original[i - 4] } else { 0 };
                let corner = if i >= 4 { previous[i - 4] } else { 0 };
                let predictor = match filter {
                    0 => 0,
                    1 => left,
                    2 => previous[i],
                    3 => ((u16::from(left) + u16::from(previous[i])) / 2) as u8,
                    _ => paeth(left, previous[i], corner),
                };
                encoded[i] = original[i].wrapping_sub(predictor);
            }
            unfilter(filter, &mut encoded, &previous, 4).unwrap();
            assert_eq!(encoded, original);
        }
        assert!(unfilter(5, &mut [0], &[0], 1).is_err());
    }

    #[test]
    fn slim_conversion_repositions_all_six_faces_and_keeps_body() {
        let source = fixture(MODERN_HEIGHT);
        let converted = adapt_arms(&source, true, false);
        assert_eq!(
            converted.pixels[20 * WIDTH + 48],
            source.pixels[20 * WIDTH + 47]
        );
        assert_eq!(
            converted.pixels[20 * WIDTH + 52],
            source.pixels[20 * WIDTH + 51]
        );
        assert_eq!(
            converted.pixels[48 * WIDTH + 40],
            source.pixels[48 * WIDTH + 39]
        );
        assert_eq!(
            converted.pixels[10 * WIDTH + 10],
            source.pixels[10 * WIDTH + 10]
        );
        assert_ne!(converted, source);
        let slim = adapt_arms(&source, false, true);
        assert_eq!(slim.pixels[20 * WIDTH + 47], source.pixels[20 * WIDTH + 48]);
        assert_eq!(slim.pixels[20 * WIDTH + 55], [0; 4]);
    }

    #[test]
    fn legacy_skin_mirrors_each_limb_face_and_exports_real_legacy_height() {
        let image = fixture(LEGACY_HEIGHT);
        let png = encode_skin_png(&image).unwrap();
        let modern = decode_skin_png(&adapt_skin_png(&png, false, false, false).unwrap()).unwrap();
        assert_eq!(modern.height, MODERN_HEIGHT);
        assert_eq!(modern.pixels[48 * WIDTH + 20], image.pixels[16 * WIDTH + 7]);
        assert_eq!(
            modern.pixels[52 * WIDTH + 32],
            image.pixels[20 * WIDTH + 51]
        );
        let old = decode_skin_png(&adapt_skin_png(&png, false, false, true).unwrap()).unwrap();
        assert_eq!(old, image);
        assert!(adapt_skin_png(&png, true, false, false).is_err());
    }
}
