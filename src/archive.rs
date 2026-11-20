use crate::i18n::t;
use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
};

const MAX_VIDEO: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_WORKSHOP_DOWNLOAD: u64 = 512 * 1024 * 1024;

pub enum Source {
    Video(String),
    Scene,
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
    let mut scene_main = None;
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
            scene_main = meta["file"].as_str().and_then(scene_json);
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
        let pkg = read(&mut zip.by_index(*i)?, crate::scene::MAX_PACKAGE)?;
        stage_scene(&pkg, folder, scene_main.as_deref())?;
        return Ok((title, Source::Scene));
    }
    Err(invalid(t(
        "В ZIP нет поддерживаемого видео или сцены Wallpaper Engine .",
        "The ZIP contains no supported video or Wallpaper Engine scene.",
        "ZIP 中没有支持的视频或Wallpaper Engine 场景。",
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

fn scene_json(name: &str) -> Option<String> {
    (name.len() <= 2048 && name.to_ascii_lowercase().ends_with(".json"))
        .then(|| name.replace('\\', "/"))
}

pub fn scene_entry(path: &Path) -> String {
    fs::read_to_string(path.with_file_name("scene-main.txt"))
        .ok()
        .and_then(|name| scene_json(&name))
        .unwrap_or_else(|| "scene.json".into())
}

pub fn stage_scene(pkg: &[u8], folder: &Path, main: Option<&str>) -> io::Result<()> {
    use crate::scene::{Object, Package, Scene};
    let package = Package::parse(pkg)?;
    let main = main
        .filter(|main| package.get(main).is_some())
        .unwrap_or("scene.json");
    let (scene, tracks) = Scene::load(&package, main)?;
    if let Some(image) = scene
        .objects
        .iter()
        .filter_map(|object| match object {
            Object::Image(layer) => Some(&scene.images[layer.texture]),
            _ => None,
        })
        .max_by_key(|image| u64::from(image.width()) * u64::from(image.height()))
    {
        let thumb = image::imageops::resize(
            image,
            crate::video::THUMB_WIDTH as u32,
            crate::video::THUMB_HEIGHT as u32,
            image::imageops::FilterType::Triangle,
        );
        let pixels: Vec<u8> = thumb
            .as_raw()
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        fs::write(folder.join("thumbnail.bgr"), pixels)?;
    }
    drop(scene);
    crate::audio::stage(&tracks, folder)?;
    fs::write(folder.join("scene-main.txt"), main)?;
    fs::write(folder.join("scene.pkg"), pkg)
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
        let data = crate::scene::read_package(&file)?;
        let main = json["file"].as_str().and_then(scene_json);
        stage_scene(&data, staging, main.as_deref())?;
        return Ok((title, Source::Scene));
    }
    Err(invalid(t(
        "Проект не содержит поддерживаемого видео или сцены.",
        "The project has no supported video or scene.",
        "项目不含受支持的视频或场景。",
    )))
}
