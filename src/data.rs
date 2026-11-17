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
    match fs::read_to_string(root().join("theme.txt"))
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("peach") => 1,
        Some("mint") => 2,
        _ => 0,
    }
}

pub fn select(id: &str) -> io::Result<()> {
    write_setting("active.txt", id)
}

pub fn save_theme(theme: usize) -> io::Result<()> {
    write_setting("theme.txt", ["frost", "peach", "mint"][theme])
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

pub fn video_for(id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let dir = root().join(id);
    ["video.mp4", "video.m4v", "video.wmv"]
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
        title: "Зимняя ночь".into(),
        path: None,
    }];
    if let Ok(dirs) = fs::read_dir(root()) {
        for dir in dirs.flatten() {
            let id = dir.file_name().to_string_lossy().into_owned();
            let Some(path) = video_for(&id) else { continue };
            let title = fs::read_to_string(dir.path().join("entry.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|json| json["Title"].as_str().map(str::to_owned))
                .unwrap_or_else(|| "Локальное видео".into());
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
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["mp4", "m4v", "wmv"].contains(&ext.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Выбери MP4, M4V или WMV.",
        ));
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = format!("{:032x}", (stamp << 32) | u128::from(std::process::id()));
    let dir = root().join(&id);
    let staging = dir.with_extension("tmp");
    fs::create_dir_all(root())?;
    fs::create_dir(&staging)?;
    let result = (|| {
        let filename = format!("video.{ext}");
        let title = source.file_stem().unwrap_or_default().to_string_lossy();
        let video = staging.join(&filename);
        fs::copy(source, &video)?;
        if crate::video::Video::available(&video) {
            crate::video::prepare_cache(&video, || false)?;
        }
        if let Some(pixels) = crate::video::thumbnail(&video) {
            let _ = fs::write(staging.join("thumbnail.bgr"), pixels);
        }
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
