use crate::{i18n::t, particles::Emitter};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    path::Path,
};

pub const MAX_PACKAGE: u64 = 128 * 1024 * 1024;
const MAX_TEXTURE_PIXELS: usize = 40 * 1024 * 1024;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

pub struct Package<'a>(HashMap<String, &'a [u8]>);

fn error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        t(
            "Не удалось прочитать структуру сцены Wallpaper Engine.",
            "Could not read the Wallpaper Engine scene structure.",
            "无法读取 Wallpaper Engine 场景结构。",
        ),
    )
}

pub fn read_package(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_PACKAGE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PACKAGE {
        return Err(error());
    }
    Ok(bytes)
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> Option<&'a [u8]> {
    let (head, tail) = input.split_at_checked(length)?;
    *input = tail;
    Some(head)
}

fn u32le(input: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(input, 4)?.try_into().ok()?))
}

fn key(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

impl<'a> Package<'a> {
    pub fn parse(mut input: &'a [u8]) -> io::Result<Self> {
        let parsed = (|| {
            let length = u32le(&mut input)? as usize;
            let magic = take(&mut input, length)?;
            if magic.len() != 8
                || &magic[..4] != b"PKGV"
                || !magic[4..].iter().all(u8::is_ascii_digit)
            {
                return None;
            }
            let count = u32le(&mut input)? as usize;
            if count > 10_000 {
                return None;
            }
            let mut index = Vec::with_capacity(count);
            for _ in 0..count {
                let length = u32le(&mut input)? as usize;
                if length == 0 || length > 2048 {
                    return None;
                }
                let name = std::str::from_utf8(take(&mut input, length)?).ok()?;
                let offset = u32le(&mut input)? as usize;
                let size = u32le(&mut input)? as usize;
                index.push((key(name), offset, size));
            }
            let mut files = HashMap::with_capacity(count);
            for (name, offset, size) in index {
                files.insert(name, input.get(offset..offset.checked_add(size)?)?);
            }
            Some(Self(files))
        })();
        parsed.ok_or_else(error)
    }

    pub fn get(&self, name: &str) -> Option<&'a [u8]> {
        self.0.get(&key(name)).copied()
    }

    pub fn json(&self, name: &str) -> Option<Value> {
        let bytes = self.get(name)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return None;
        }
        serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)).ok()
    }

    pub fn material(&self, name: &str) -> Option<Value> {
        self.json(name)
            .or_else(|| self.json(&format!("materials/{name}.json")))
    }

    fn texture(&self, name: &str, budget: usize) -> Option<image::RgbaImage> {
        let base = name.strip_prefix("materials/").unwrap_or(name);
        let base = base.strip_suffix(".tex").unwrap_or(base);
        let bytes = self
            .get(&format!("materials/{base}.tex"))
            .or_else(|| self.get(name))
            .or_else(|| self.get(&format!("materials/{base}.png")))?;
        decode_texture(bytes, budget)
    }
}

fn png(bytes: &[u8], budget: usize) -> Option<image::RgbaImage> {
    let mut reader =
        image::ImageReader::with_format(io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some((budget * 8) as u64);
    reader.limits(limits);
    let decoder = reader.into_decoder().ok()?;
    let (w, h) = image::ImageDecoder::dimensions(&decoder);
    if w as usize * h as usize > budget {
        return None;
    }
    Some(
        image::DynamicImage::from_decoder(decoder)
            .ok()?
            .into_rgba8(),
    )
}

fn decode_texture(bytes: &[u8], budget: usize) -> Option<image::RgbaImage> {
    if let Some(at) = bytes.windows(PNG.len()).take(512).position(|p| p == PNG) {
        return png(&bytes[at..], budget);
    }
    if !bytes.starts_with(b"TEXV0005\0TEXI0001\0") {
        return None;
    }
    let format = u32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?);
    let flags = u32::from_le_bytes(bytes.get(22..26)?.try_into().ok()?);
    let mut tail = bytes.get(46..)?;
    let version = take(&mut tail, 9)?;
    if ![b"TEXB0001\0", b"TEXB0002\0", b"TEXB0003\0"].contains(&version.try_into().ok()?) {
        return None;
    }
    if u32le(&mut tail)? == 0 {
        return None;
    }
    if version == b"TEXB0003\0" && u32le(&mut tail)? != u32::MAX {
        return None;
    }
    if u32le(&mut tail)? == 0 {
        return None;
    }
    let w = u32le(&mut tail)?;
    let h = u32le(&mut tail)?;
    if !(1..=8192).contains(&w) || !(1..=8192).contains(&h) {
        return None;
    }
    let pixels = w as usize * h as usize;
    if pixels > budget {
        return None;
    }
    let blocks = match format {
        7 => 8,
        4 | 6 | 12 => 16,
        _ => 0,
    };
    let expected = if blocks > 0 {
        w.div_ceil(4) as usize * h.div_ceil(4) as usize * blocks
    } else {
        pixels
            * match format {
                0 => 4,
                1 => 3,
                8 => 2,
                9 => 1,
                _ => return None,
            }
    };
    let (compression, unpacked) = if version == b"TEXB0001\0" {
        (0, 0)
    } else {
        (u32le(&mut tail)?, u32le(&mut tail)? as usize)
    };
    let len = u32le(&mut tail)? as usize;
    let encoded = take(&mut tail, len)?;
    let data = match compression {
        0 if len == expected => std::borrow::Cow::Borrowed(encoded),
        1 if unpacked == expected => {
            std::borrow::Cow::Owned(lz4_flex::block::decompress(encoded, expected).ok()?)
        }
        _ => return None,
    };
    let alpha = flags & 524_288 != 0;
    let rgba = match format {
        0 => data.into_owned(),
        1 => data
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        8 if alpha => data
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        8 => data
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[1], 0, 255])
            .collect(),
        9 if alpha => data.iter().flat_map(|&a| [255, 255, 255, a]).collect(),
        9 => data.iter().flat_map(|&r| [r, r, r, 255]).collect(),
        _ => decode_blocks(&data, w as usize, h as usize, blocks, format),
    };
    image::RgbaImage::from_raw(w, h, rgba)
}

fn decode_blocks(data: &[u8], width: usize, height: usize, stride: usize, format: u32) -> Vec<u8> {
    let mut rgba = vec![0; width * height * 4];
    let mut block = [0; 64];
    for (index, bytes) in data.chunks_exact(stride).enumerate() {
        match format {
            7 => bcdec_rs::bc1(bytes, &mut block, 16),
            6 => bcdec_rs::bc2(bytes, &mut block, 16),
            4 => bcdec_rs::bc3(bytes, &mut block, 16),
            _ => bcdec_rs::bc7(bytes, &mut block, 16),
        }
        let x = (index % width.div_ceil(4)) * 4;
        let y = (index / width.div_ceil(4)) * 4;
        let span = (width - x).min(4) * 4;
        for row in 0..(height - y).min(4) {
            let dst = ((y + row) * width + x) * 4;
            rgba[dst..dst + span].copy_from_slice(&block[row * 16..row * 16 + span]);
        }
    }
    rgba
}

pub fn number(v: &Value, default: f32, min: f32, max: f32) -> f32 {
    let v = v.get("value").unwrap_or(v);
    v.as_f64()
        .map(|n| n as f32)
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
        .unwrap_or(default)
        .clamp(min, max)
}

pub fn vector<const N: usize>(v: &Value, mut default: [f32; N]) -> [f32; N] {
    let v = v.get("value").unwrap_or(v);
    if let Some(s) = v.as_str() {
        for (out, n) in default.iter_mut().zip(s.split_whitespace()) {
            if let Ok(n) = n.parse::<f32>() {
                if n.is_finite() {
                    *out = n.clamp(-100_000.0, 100_000.0);
                }
            }
        }
    } else if let Some(array) = v.as_array() {
        for (out, n) in default.iter_mut().zip(array) {
            *out = number(n, *out, -100_000.0, 100_000.0);
        }
    }
    default
}

pub fn list(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn visible(v: &Value) -> bool {
    v.get("value").unwrap_or(v).as_bool().unwrap_or(true)
}

#[derive(Default)]
pub struct Effect {
    pub water: [f32; 4],
    pub scroll: [f32; 2],
    pub water_mask: Option<usize>,
    pub normals: Option<usize>,
    pub shake: [f32; 4],
    pub flow: Option<usize>,
    pub iris: [f32; 2],
    pub iris_mask: Option<usize>,
}

pub struct Layer {
    pub texture: usize,
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub angle: f32,
    pub tint: [f32; 4],
    pub depth: [f32; 2],
    pub additive: bool,
    pub effect: Effect,
}

pub struct Track<'a> {
    pub file: &'a [u8],
    pub extension: String,
    pub volume: u32,
    pub looping: bool,
}

pub enum Object {
    Image(Layer),
    Particles(Box<Emitter>),
}

pub struct Scene {
    pub size: [f32; 2],
    pub clear: [f32; 3],
    pub camera: [f32; 3],
    pub parallax: f32,
    pub objects: Vec<Object>,
    pub images: Vec<image::RgbaImage>,
    textures: HashMap<String, Option<usize>>,
    pixels: usize,
}

impl Scene {
    pub fn load<'a>(package: &Package<'a>, main: &str) -> io::Result<(Self, Vec<Track<'a>>)> {
        let scene = package.json(main).ok_or_else(error)?;
        let general = &scene["general"];
        let projection = &general["orthogonalprojection"];
        let size = [
            number(&projection["width"], 1920.0, 32.0, 16384.0),
            number(&projection["height"], 1080.0, 32.0, 16384.0),
        ];
        let mut data = Self {
            size,
            clear: vector(&general["clearcolor"], [0.0; 3]),
            camera: [0.0; 3],
            parallax: 0.0,
            objects: Vec::new(),
            images: Vec::new(),
            textures: HashMap::new(),
            pixels: 0,
        };
        if general["camerashake"].as_bool() == Some(true) {
            data.camera = [
                number(&general["camerashakeamplitude"], 0.3, 0.0, 4.0),
                number(&general["camerashakespeed"], 1.0, 0.0, 10.0),
                number(&general["camerashakeroughness"], 1.0, 0.0, 10.0),
            ];
        }
        if general["cameraparallax"].as_bool() == Some(true) {
            data.parallax = number(&general["cameraparallaxamount"], 0.5, 0.0, 2.0)
                * number(&general["cameraparallaxmouseinfluence"], 1.0, 0.0, 2.0);
        }
        let mut tracks = Vec::new();
        for object in list(&scene["objects"]).iter().take(512) {
            if !visible(&object["visible"]) {
                continue;
            }
            if let Some(model) = object["image"].as_str() {
                if let Some(layer) = data.layer(package, object, model) {
                    data.objects.push(Object::Image(layer));
                }
            } else if let Some(particle) = object["particle"].as_str() {
                Emitter::load(&mut data, package, object, particle);
            } else {
                for name in list(&object["sound"]).iter().filter_map(Value::as_str) {
                    if tracks.len() >= 8 {
                        break;
                    }
                    let Some((_, extension)) = name.rsplit_once('.') else {
                        continue;
                    };
                    let extension = extension.to_ascii_lowercase();
                    if !matches!(extension.as_str(), "wav" | "mp3" | "ogg" | "flac") {
                        continue;
                    }
                    let Some(file) = package
                        .get(name)
                        .or_else(|| package.get(&format!("sound/{name}")))
                    else {
                        continue;
                    };
                    if file.len() > 32 * 1024 * 1024 {
                        continue;
                    }
                    tracks.push(Track {
                        file,
                        extension,
                        volume: (100.0 * number(&object["volume"], 1.0, 0.0, 1.0)) as u32,
                        looping: object["playbackmode"].as_str() == Some("loop"),
                    });
                }
            }
        }
        if data.objects.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                t(
                    "В сцене нет поддерживаемых слоёв изображения или эмиттеров.",
                    "The scene has no supported image layers or emitters.",
                    "场景不包含受支持的图层或发射器。",
                ),
            ));
        }
        Ok((data, tracks))
    }

    pub fn texture(&mut self, package: &Package<'_>, name: &str) -> Option<usize> {
        let key = key(name);
        if let Some(index) = self.textures.get(&key) {
            return *index;
        }
        let index = package
            .texture(name, MAX_TEXTURE_PIXELS - self.pixels)
            .map(|image| {
                self.pixels += image.width() as usize * image.height() as usize;
                let index = self.images.len();
                self.images.push(image);
                index
            });
        self.textures.insert(key, index);
        index
    }

    fn layer(&mut self, package: &Package<'_>, object: &Value, model: &str) -> Option<Layer> {
        let model = package.json(model)?;
        let material = package.material(model["material"].as_str()?)?;
        let pass = list(&material["passes"]).first()?;
        let texture = self.texture(package, pass["textures"][0].as_str()?)?;
        let image = &self.images[texture];
        let [w, h] = vector(
            &object["size"],
            [image.width() as f32, image.height() as f32],
        );
        let [sx, sy] = vector(&object["scale"], [1.0, 1.0]);
        let [_, _, angle] = vector(&object["angles"], [0.0; 3]);
        let [r, g, b] = vector(&object["color"], [1.0; 3]);
        let brightness = number(&object["brightness"], 1.0, 0.0, 4.0);
        let mut origin = vector(&object["origin"], [self.size[0] / 2.0, self.size[1] / 2.0]);
        let size = [w * sx, h * sy];
        let alignment = object["alignment"].as_str().unwrap_or("center");
        if alignment.contains("left") {
            origin[0] += size[0] * 0.5;
        }
        if alignment.contains("right") {
            origin[0] -= size[0] * 0.5;
        }
        if alignment.contains("top") {
            origin[1] -= size[1] * 0.5;
        }
        if alignment.contains("bottom") {
            origin[1] += size[1] * 0.5;
        }
        let mut effect = Effect::default();
        for item in list(&object["effects"])
            .iter()
            .filter(|item| visible(&item["visible"]))
        {
            let Some(file) = item["file"].as_str() else {
                continue;
            };
            let definition = package.json(file);
            let name = definition
                .as_ref()
                .and_then(|d| d["replacementkey"].as_str())
                .or_else(|| file.rsplit('/').nth(1))
                .unwrap_or("");
            let pass = &item["passes"][0];
            let v = &pass["constantshadervalues"];
            let mask = pass["textures"][1].as_str();
            match name {
                "waterripple" => {
                    effect.water = [
                        number(&v["ripplestrength"], 0.1, 0.0, 1.0),
                        number(&v["animationspeed"], 0.15, 0.0, 3.0),
                        number(&v["scale"], 1.0, 0.1, 32.0),
                        number(&v["ratio"], 1.0, 0.0, 10.0),
                    ];
                    let rate = number(&v["scrollspeed"], 0.0, 0.0, 2.0).powi(2);
                    let angle = number(&v["scrolldirection"], 0.0, -7.0, 7.0);
                    effect.scroll = [-angle.sin() * rate, angle.cos() * rate];
                    effect.water_mask = mask.and_then(|name| self.texture(package, name));
                    effect.normals = self.texture(
                        package,
                        pass["textures"][2]
                            .as_str()
                            .unwrap_or("effects/waterripplenormal"),
                    );
                }
                "shake" if effect.flow.is_none() => {
                    let flow = mask.and_then(|name| self.texture(package, name));
                    if flow.is_some() {
                        let [lo, hi] = vector(&v["bounds"], [0.0, 1.0]);
                        effect.shake = [
                            number(&v["strength"], 0.1, 0.0, 1.0),
                            number(&v["speed"], 1.0, 0.0, 20.0),
                            lo,
                            (hi - lo).max(0.01),
                        ];
                        effect.flow = flow;
                    }
                }
                "iris" => {
                    effect.iris = [
                        number(&v["speed"], 1.0, 0.0, 10.0),
                        number(&v["noiseamount"], 0.0, 0.0, 20.0) * 0.00035,
                    ];
                    effect.iris_mask = mask.and_then(|name| self.texture(package, name));
                }
                _ => {}
            }
        }
        Some(Layer {
            texture,
            origin,
            size,
            angle,
            tint: [
                r * brightness,
                g * brightness,
                b * brightness,
                number(&object["alpha"], 1.0, 0.0, 1.0),
            ],
            depth: vector(&object["parallaxDepth"], [1.0; 2]),
            additive: pass["blending"].as_str() == Some("additive"),
            effect,
        })
    }
}
