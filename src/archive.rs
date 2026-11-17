use crate::i18n::t;
use std::{
    fs::{self, File},
    io::{self, Cursor, Read, Write},
    path::Path,
};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
const MAX_TEXTURE: u64 = 128 * 1024 * 1024;
const MAX_VIDEO: u64 = 8 * 1024 * 1024 * 1024;

pub enum Source {
    Video(String),
    Still,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn unpack(path: &Path, folder: &Path) -> io::Result<(Option<String>, Source)> {
    let mut zip = zip::ZipArchive::new(File::open(path)?)?;
    if zip.len() > 10000 {
        return Err(invalid(t(
            "Слишком много файлов в ZIP.",
            "The ZIP contains too many files.",
            "ZIP 中的文件过多。",
        )));
    }
    let names: Vec<_> = (0..zip.len())
        .filter_map(|i| {
            zip.by_index(i)
                .ok()
                .filter(|file| file.is_file())
                .map(|file| (i, file.name().replace('\\', "/")))
        })
        .collect();
    let project = names.iter().find(|(_, name)| {
        name.rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("project.json"))
    });
    let mut title = None;
    let mut preferred = None;
    let mut scene = false;
    if let Some((i, name)) = project {
        let json = read(&mut zip.by_index(*i)?, 1024 * 1024)?;
        if let Ok(meta) = serde_json::from_slice::<serde_json::Value>(
            json.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&json),
        ) {
            title = meta["title"]
                .as_str()
                .map(|s| s.trim().chars().take(160).collect::<String>())
                .filter(|s| !s.is_empty());
            scene = meta["type"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("scene"));
            preferred = meta["file"].as_str().map(|file| {
                let base = name.rsplit_once('/').map_or("", |(base, _)| base);
                let file = file.replace('\\', "/");
                if base.is_empty() {
                    file
                } else {
                    format!("{base}/{file}")
                }
            });
        }
    }
    let found = preferred
        .as_ref()
        .and_then(|path| {
            names
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(path) && video_ext(name).is_some())
        })
        .or_else(|| {
            if scene {
                None
            } else {
                names.iter().find(|(_, name)| video_ext(name).is_some())
            }
        });
    if let Some((i, name)) = found {
        let filename = format!("video.{}", video_ext(name).unwrap());
        let mut member = zip.by_index(*i)?;
        if member.size() > MAX_VIDEO {
            return Err(invalid(t(
                "Видео в ZIP больше 8 ГиБ.",
                "The video in the ZIP exceeds 8 GiB.",
                "ZIP 中的视频超过 8 GiB。",
            )));
        }
        let mut file = File::create(folder.join(&filename))?;
        if io::copy(&mut member.by_ref().take(MAX_VIDEO + 1), &mut file)? > MAX_VIDEO {
            return Err(invalid(t(
                "Видео в ZIP больше 8 ГиБ.",
                "The video in the ZIP exceeds 8 GiB.",
                "ZIP 中的视频超过 8 GiB。",
            )));
        }
        return Ok((title, Source::Video(filename)));
    }
    if let Some((i, _)) = names.iter().find(|(_, name)| {
        name.rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("scene.pkg"))
    }) {
        let pkg = read(&mut zip.by_index(*i)?, MAX_TEXTURE)?;
        let png = background(&pkg)?;
        save_image(png, folder)?;
        return Ok((title, Source::Still));
    }
    Err(invalid(t(
        "В ZIP нет поддерживаемого видео или сцены Wallpaper Engine с PNG-текстурой.",
        "The ZIP contains no supported video or Wallpaper Engine scene with a PNG texture.",
        "ZIP 中没有支持的视频或包含 PNG 纹理的 Wallpaper Engine 场景。",
    )))
}

fn video_ext(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1;
    ["mp4", "m4v", "wmv", "webm", "mkv"]
        .into_iter()
        .find(|known| ext.eq_ignore_ascii_case(known))
}

fn read(file: &mut zip::read::ZipFile<'_, File>, limit: u64) -> io::Result<Vec<u8>> {
    if file.size() > limit {
        return Err(invalid(t(
            "Файл в архиве слишком большой.",
            "A file in the archive is too large.",
            "压缩包中的文件过大。",
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid(t(
            "Файл в архиве слишком большой.",
            "A file in the archive is too large.",
            "压缩包中的文件过大。",
        )));
    }
    Ok(bytes)
}

fn background(pkg: &[u8]) -> io::Result<&[u8]> {
    fn bytes<'a>(pkg: &'a [u8], position: &mut usize, size: usize) -> Option<&'a [u8]> {
        let end = position.checked_add(size)?;
        let out = pkg.get(*position..end)?;
        *position = end;
        Some(out)
    }
    fn number(pkg: &[u8], position: &mut usize) -> Option<usize> {
        Some(u32::from_le_bytes(bytes(pkg, position, 4)?.try_into().ok()?) as usize)
    }
    fn parse(pkg: &[u8]) -> Option<&[u8]> {
        let mut pos = 0;
        let magic_len = number(pkg, &mut pos)?;
        let magic = bytes(pkg, &mut pos, magic_len)?;
        if magic != b"PKGV0018" {
            return None;
        }
        let count = number(pkg, &mut pos)?;
        if count > 10000 {
            return None;
        }
        let mut textures = Vec::new();
        for _ in 0..count {
            let len = number(pkg, &mut pos)?;
            let name = bytes(pkg, &mut pos, len)?;
            let start = number(pkg, &mut pos)?;
            let length = number(pkg, &mut pos)?;
            if name.ends_with(b".tex") {
                textures.push((start, length));
            }
        }
        textures
            .into_iter()
            .filter_map(|(offset, length)| {
                let start = pos.checked_add(offset)?;
                let texture = pkg.get(start..start.checked_add(length)?)?;
                let png = &texture[texture
                    .windows(PNG.len())
                    .take(512)
                    .position(|prefix| prefix == PNG)?..];
                let w = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
                let h = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);
                if !(4..=3840).contains(&w) || !(1..=2160).contains(&h) {
                    return None;
                }
                Some((w * h, png))
            })
            .max_by_key(|(size, _)| *size)
            .map(|(_, png)| png)
    }
    parse(pkg).ok_or_else(|| {
        invalid(t(
            "В сцене нет PNG-текстуры до 3840×2160. Этот тип сцены пока не поддерживается.",
            "This scene has no supported PNG texture up to 3840×2160.",
            "此场景没有受支持的 PNG 纹理（最高 3840×2160）。",
        ))
    })
}

fn save_image(png: &[u8], folder: &Path) -> io::Result<()> {
    let reader = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png);
    let image = reader.decode().map_err(|_| {
        invalid(t(
            "Не удалось открыть текстуру PNG.",
            "Could not open the PNG texture.",
            "无法打开 PNG 纹理。",
        ))
    })?;
    let width = image.width() & !3;
    let height = image.height();
    let thumb = image.resize_to_fill(
        crate::video::THUMB_WIDTH as u32,
        crate::video::THUMB_HEIGHT as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let frame = image.crop_imm(0, 0, width, height).into_rgb8();
    let mut pixels = frame.into_raw();
    for pixel in pixels.chunks_exact_mut(3) {
        pixel.swap(0, 2);
    }
    let mut output = File::create(folder.join("image.bgr"))?;
    output.write_all(&width.to_le_bytes())?;
    output.write_all(&height.to_le_bytes())?;
    output.write_all(&pixels)?;
    let mut preview = thumb.into_rgb8().into_raw();
    for pixel in preview.chunks_exact_mut(3) {
        pixel.swap(0, 2);
    }
    fs::write(folder.join("thumbnail.bgr"), preview)
}
pub fn unpack_project(source: &Path, staging: &Path) -> io::Result<(Option<String>, Source)> {
    use std::path::Component;
    let project = if source.is_dir() {
        source.join("project.json")
    } else {
        source.to_owned()
    };
    let root = project
        .parent()
        .ok_or_else(|| {
            invalid(t(
                "Не найдена папка проекта.",
                "The project folder was not found.",
                "未找到项目文件夹。",
            ))
        })?
        .canonicalize()?;
    let mut meta = Vec::new();
    File::open(&project)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut meta)?;
    if meta.len() > 1024 * 1024 {
        return Err(invalid(t(
            "project.json слишком большой.",
            "The project.json file is too large.",
            "project.json 文件过大。",
        )));
    }
    let json: serde_json::Value = serde_json::from_slice(
        meta.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&meta),
    )
    .map_err(|_| {
        invalid(t(
            "Не удалось прочитать project.json.",
            "Could not read project.json.",
            "无法读取 project.json。",
        ))
    })?;
    let title = json["title"]
        .as_str()
        .map(|s| s.trim().chars().take(160).collect::<String>())
        .filter(|s| !s.is_empty());
    let member = |name: &str| {
        let name = name.replace('\\', "/");
        let path = Path::new(&name);
        if !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(invalid(t(
                "Проект ссылается на файл вне своей папки.",
                "The project refers to a file outside its folder.",
                "项目引用了文件夹之外的文件。",
            )));
        }
        let full = root.join(path).canonicalize()?;
        if !full.starts_with(&root) || !full.is_file() {
            return Err(invalid(t(
                "Медиафайл проекта не найден в его папке.",
                "The project media file was not found in its folder.",
                "在项目文件夹中未找到媒体文件。",
            )));
        }
        Ok(full)
    };
    if let Some(name) = json["file"].as_str() {
        if let Some(ext) = video_ext(name) {
            let file = member(name)?;
            let filename = format!("video.{ext}");
            fs::copy(file, staging.join(&filename))?;
            return Ok((title, Source::Video(filename)));
        }
    }
    if json["type"]
        .as_str()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("scene"))
    {
        let file = member("scene.pkg")?;
        let mut data = Vec::new();
        File::open(file)?
            .take(MAX_TEXTURE + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 > MAX_TEXTURE {
            return Err(invalid(t(
                "scene.pkg слишком большой.",
                "The scene.pkg file is too large.",
                "scene.pkg 文件过大。",
            )));
        }
        save_image(background(&data)?, staging)?;
        return Ok((title, Source::Still));
    }
    Err(invalid(t(
        "Проект не содержит поддерживаемого видео или PNG-текстуры.",
        "The project has no supported video or PNG texture.",
        "项目不含受支持的视频或 PNG 纹理。",
    )))
}
