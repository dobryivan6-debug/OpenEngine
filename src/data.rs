use crate::i18n::t;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

#[derive(Clone)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub path: Option<PathBuf>,
}

pub fn root() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("OpenEngine")
        .join("Library")
}

pub fn active() -> String {
    fs::read_to_string(root().join("active.txt"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "winter".into())
}

pub fn theme() -> usize {
    usize::from(fs::read_to_string(root().join("theme.txt")).is_ok_and(|s| s.trim() == "dark"))
}

pub fn language() -> usize {
    match fs::read_to_string(root().join("language.txt"))
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("en") => 1,
        Some("zh-CN") => 2,
        _ => 0,
    }
}

pub fn save_language(index: usize) -> io::Result<()> {
    write_setting("language.txt", ["ru", "en", "zh-CN"][index])
}

pub fn quality() -> crate::video::Quality {
    if fs::read_to_string(root().join("quality.txt")).is_ok_and(|s| s.trim() == "low") {
        crate::video::Quality::Low
    } else {
        crate::video::Quality::Detail
    }
}

pub fn save_quality(low: bool) -> io::Result<()> {
    write_setting("quality.txt", if low { "low" } else { "detail" })
}

pub fn select(id: &str) -> io::Result<()> {
    write_setting("active.txt", id)
}

pub fn save_theme(theme: usize) -> io::Result<()> {
    write_setting("theme.txt", ["white", "dark"][theme])
}

fn write_setting(name: &str, value: &str) -> io::Result<()> {
    let root = root();
    fs::create_dir_all(&root)?;
    let target = root.join(name);
    let temp = root.join(format!("{name}.{}.tmp", std::process::id()));
    fs::write(&temp, value)?;
    let result = unsafe {
        MoveFileExW(
            crate::win::wide_os(temp.as_os_str()).as_ptr(),
            crate::win::wide_os(target.as_os_str()).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(temp);
        return Err(error);
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn media_for(id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let dir = root().join(id);
    [
        "video.mp4",
        "video.m4v",
        "video.wmv",
        "video.webm",
        "video.mkv",
        "scene.pkg",
        "image.bgr",
    ]
    .into_iter()
    .map(|name| dir.join(name))
    .find(|path| path.is_file())
}

pub fn thumbnail(entry: &Entry) -> Option<Vec<u8>> {
    let path = entry.path.as_ref()?.with_file_name("thumbnail.bgr");
    let size = crate::video::THUMB_WIDTH * crate::video::THUMB_HEIGHT * 3;
    if fs::metadata(&path).ok()?.len() != size as u64 {
        return None;
    }
    let pixels = fs::read(path).ok()?;
    (pixels.len() == size).then_some(pixels)
}

pub fn entries() -> Vec<Entry> {
    let mut items = vec![Entry {
        id: "winter".into(),
        title: t("Зимняя ночь", "Winter Night", "冬夜").into(),
        path: None,
    }];
    if let Ok(dirs) = fs::read_dir(root()) {
        for dir in dirs.flatten() {
            let id = dir.file_name().to_string_lossy().into_owned();
            let Some(path) = media_for(&id) else { continue };
            let title = fs::read_to_string(dir.path().join("entry.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|json| json["Title"].as_str().map(str::to_owned))
                .unwrap_or_else(|| t("Локальное видео", "Local video", "本地视频").into());
            items.push(Entry {
                id,
                title,
                path: Some(path),
            });
        }
    }
    items[1..].sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
    items
}

pub fn import(source: &Path) -> io::Result<String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    import_as(
        source,
        format!("{:032x}", (stamp << 32) | u128::from(std::process::id())),
    )
}

pub fn import_workshop(source: &Path, item: u64) -> io::Result<String> {
    let id = workshop_id(item);
    if media_for(&id).is_some() {
        return Ok(id);
    }
    import_as(source, id)
}

pub fn workshop_id(item: u64) -> String {
    format!("{item:032x}")
}

pub fn workshop_queue() -> Vec<u64> {
    fs::read_to_string(root().join("workshop-queue.txt"))
        .unwrap_or_default()
        .lines()
        .take(64)
        .filter_map(|line| line.parse().ok())
        .collect()
}

pub fn save_workshop_queue(items: &[u64]) -> io::Result<()> {
    let list = items
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    write_setting("workshop-queue.txt", &list)
}

fn import_as(source: &Path, id: String) -> io::Result<String> {
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let project = source.is_dir()
        || source
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("project.json"));
    if !project && !["mp4", "m4v", "wmv", "webm", "mkv", "zip", "pkg"].contains(&ext.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            t(
                "Выбери видео, ZIP или папку проекта Wallpaper Engine.",
                "Choose a video, ZIP, or Wallpaper Engine project folder.",
                "请选择视频、ZIP 或 Wallpaper Engine 项目文件夹。",
            ),
        ));
    }
    let dir = root().join(&id);
    let staging = dir.with_extension("tmp");
    fs::create_dir_all(root())?;
    fs::create_dir(&staging)?;
    let result = (|| {
        let (title, media) = if project {
            crate::archive::unpack_project(source, &staging)?
        } else if ext == "zip" {
            crate::archive::unpack(source, &staging)?
        } else if ext == "pkg" {
            let raw = crate::scene::read_package(source)?;
            crate::archive::stage_scene(&raw, &staging, None)?;
            (None, crate::archive::Source::Scene)
        } else {
            let filename = format!("video.{ext}");
            fs::copy(source, staging.join(&filename))?;
            (None, crate::archive::Source::Video(filename))
        };
        let title = title.unwrap_or_else(|| {
            source
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let filename = match media {
            crate::archive::Source::Scene => "scene.pkg".into(),
            crate::archive::Source::Video(filename) => {
                let video = staging.join(&filename);
                let quality = quality();
                if crate::video::Video::available(&video, quality) {
                    crate::video::prepare_cache(&video, quality, || false)?;
                }
                if let Some(pixels) = crate::video::thumbnail(&video) {
                    fs::write(staging.join("thumbnail.bgr"), pixels)?;
                }
                filename
            }
        };
        fs::write(
            staging.join("entry.json"),
            serde_json::json!({ "Id": id, "Title": title, "VideoName": filename }).to_string(),
        )?;
        fs::rename(&staging, dir)?;
        Ok(id)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

pub fn volume() -> u32 {
    fs::read_to_string(root().join("volume.txt"))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
        .min(100)
}

pub fn save_volume(volume: u32) -> io::Result<()> {
    write_setting("volume.txt", &volume.min(100).to_string())
}

pub fn is_scene(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "pkg")
}

pub fn is_video(path: &Path) -> bool {
    !is_still(path) && !is_scene(path)
}

pub fn is_still(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "bgr")
}

pub fn read_still(path: &Path) -> Option<crate::video::Frame> {
    let length = fs::metadata(path).ok()?.len();
    if !(20..=(3840 * 2160 * 3 + 8)).contains(&length) {
        return None;
    }
    let mut bytes = fs::read(path).ok()?;
    let width = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let height = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    if !(4..=3840).contains(&width)
        || width % 4 != 0
        || !(1..=2160).contains(&height)
        || bytes.len() != 8 + width * height * 3
    {
        return None;
    }
    bytes.drain(..8);
    Some(crate::video::Frame {
        pixels: bytes,
        width,
        height,
    })
}
