use crate::{archive, data, i18n::t, video, win::wide};
use serde_json::Value;
use std::{
    env,
    fs::{self, File},
    io::{self, Write},
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::HWND,
    Networking::WinHttp::*,
    System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ},
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};

const APP: &str = "431960";
const COMMUNITY: &str = "steamcommunity.com";

#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub title: String,
    pub kind: String,
    pub preview: Option<String>,
}

pub struct Page {
    pub items: Vec<Item>,
    pub last: u32,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        t(
            "Steam вернул ответ в неизвестном формате.",
            "Steam returned an unrecognized response.",
            "Steam 返回了无法识别的响应。",
        ),
    )
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    use std::fmt::Write;
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

pub fn catalog(query: &str, page: u32, language: usize) -> io::Result<Page> {
    let language = ["russian", "english", "schinese"][language.min(2)];
    let sort = if query.trim().is_empty() {
        "trend"
    } else {
        "textsearch"
    };
    let target = format!("/workshop/browse/?appid={APP}&browsesort={sort}&section=readytouseitems&searchtext={}&p={page}&l={language}", encode(query.trim()));
    let mut html = Vec::new();
    request(COMMUNITY, &target, None, 3 * 1024 * 1024, &mut html)?;
    let html = std::str::from_utf8(&html).map_err(|_| invalid())?;
    let (_, encoded) = html.split_once("\"queryData\":").ok_or_else(invalid)?;
    let query_data = serde_json::Deserializer::from_str(encoded)
        .into_iter::<String>()
        .next()
        .ok_or_else(invalid)?
        .map_err(|_| invalid())?;
    let response: Value = serde_json::from_str(&query_data).map_err(|_| invalid())?;
    let result = crate::scene::list(&response["queries"])
        .iter()
        .find(|row| row["queryKey"][0].as_str() == Some("workshop_browse"))
        .ok_or_else(invalid)?;
    let result = &result["state"]["data"];
    if result["eresult"].as_u64() != Some(1) {
        return Err(invalid());
    }
    let items = crate::scene::list(&result["results"])
        .iter()
        .filter_map(|row| {
            if row["consumer_appid"].as_u64() != Some(431960) {
                return None;
            }
            let kind = crate::scene::list(&row["tags"])
                .iter()
                .filter_map(|tag| tag["tag"].as_str())
                .find(|tag| ["Scene", "Video", "Web", "Application"].contains(tag))
                .unwrap_or("");
            Some(Item {
                id: row["publishedfileid"].as_str()?.parse().ok()?,
                title: row["title"]
                    .as_str()?
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(160)
                    .collect(),
                kind: kind.to_owned(),
                preview: row["preview_url"].as_str().map(str::to_owned),
            })
        })
        .collect();
    let last = result["total_pages"].as_u64().unwrap_or(1).clamp(1, 1000) as u32;
    Ok(Page { items, last })
}

pub fn preview(item: &Item) -> Option<(Vec<u8>, usize, usize)> {
    let (host, path) = cdn_url(item.preview.as_deref()?)?;
    let mut bytes = Vec::new();
    request(host, path, None, 2 * 1024 * 1024, &mut bytes).ok()?;
    let image = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().ok()?.into_rgb8()
    } else {
        let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(&bytes));
        decoder.set_max_decoding_buffer_size(32 * 1024 * 1024);
        let pixels = decoder.decode().ok()?;
        let info = decoder.info()?;
        image::RgbImage::from_raw(info.width as u32, info.height as u32, pixels)?
    };
    let width = video::THUMB_WIDTH;
    let height = video::THUMB_HEIGHT;
    let frame = image::DynamicImage::ImageRgb8(image).resize_to_fill(
        width as u32,
        height as u32,
        image::imageops::FilterType::Triangle,
    );
    let mut pixels = frame.into_rgb8().into_raw();
    for pixel in pixels.chunks_exact_mut(3) {
        pixel.swap(0, 2);
    }
    Some((pixels, width, height))
}

fn cdn_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (host, _) = rest.split_once('/')?;
    let valid = matches!(
        host,
        "steamusercontent-a.akamaihd.net" | "steamuserimages-a.akamaihd.net"
    ) || host.ends_with(".steamusercontent.com")
        || host.ends_with(".steamcontent.com");
    (valid
        && host
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-'))
    .then_some((host, &rest[host.len()..]))
}

pub fn import_item(id: u64) -> io::Result<Option<String>> {
    let entry = data::workshop_id(id);
    if data::media_for(&entry).is_some() {
        return Ok(Some(entry));
    }
    if let Some(folder) = local_item(id) {
        return data::import_workshop(&folder, id).map(Some);
    }
    let body = format!("itemcount=1&publishedfileids%5B0%5D={id}");
    let mut output = Vec::new();
    request(
        "api.steampowered.com",
        "/ISteamRemoteStorage/GetPublishedFileDetails/v1/",
        Some(body.as_bytes()),
        256 * 1024,
        &mut output,
    )?;
    let details: Value = serde_json::from_slice(&output).map_err(|_| invalid())?;
    let info = &details["response"]["publishedfiledetails"][0];
    if info["consumer_app_id"].as_u64() != Some(431960) || info["result"].as_u64() != Some(1) {
        return Err(invalid());
    }
    if let Some((host, path)) = info["file_url"].as_str().and_then(cdn_url) {
        let suffix = info["filename"]
            .as_str()
            .and_then(|name| name.rsplit_once('.'))
            .map(|(_, ext)| ext.to_ascii_lowercase());
        if let Some(ext) = suffix.filter(|ext| {
            ["zip", "pkg", "mp4", "m4v", "wmv", "webm", "mkv"].contains(&ext.as_str())
        }) {
            let dir = data::root().join(format!("workshop-{id}.download"));
            fs::create_dir_all(data::root())?;
            fs::create_dir(&dir)?;
            let file = dir.join(format!("Workshop {id}.{ext}"));
            let result = (|| {
                let mut writer = File::create(&file)?;
                request(
                    host,
                    path,
                    None,
                    archive::MAX_WORKSHOP_DOWNLOAD,
                    &mut writer,
                )?;
                drop(writer);
                data::import_workshop(&file, id).map(Some)
            })();
            let _ = fs::remove_file(file);
            let _ = fs::remove_dir(dir);
            return result;
        }
    }
    Ok(None)
}

pub fn item_url(id: u64) -> String {
    format!("https://{COMMUNITY}/sharedfiles/filedetails/?id={id}")
}

pub fn open_item(owner: HWND, id: u64) {
    unsafe {
        let uri = format!("steam://url/CommunityFilePage/{id}");
        let verb = wide("open");
        if ShellExecuteW(
            owner,
            verb.as_ptr(),
            wide(&uri).as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        ) as usize
            <= 32
        {
            ShellExecuteW(
                owner,
                verb.as_ptr(),
                wide(&item_url(id)).as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            );
        }
    }
}

pub fn local_item(id: u64) -> Option<PathBuf> {
    let id = id.to_string();
    steam_libraries()
        .into_iter()
        .map(|root| {
            root.join("steamapps")
                .join("workshop")
                .join("content")
                .join(APP)
                .join(&id)
        })
        .find(|p| p.join("project.json").is_file())
}

fn steam_libraries() -> Vec<PathBuf> {
    let mut buf = [0u16; 2048];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide("Software\\Valve\\Steam").as_ptr(),
            wide("SteamPath").as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        ) == 0
    };
    let mut roots = Vec::new();
    if let Some(path) = env::var_os("OPENENGINE_STEAM_ROOT") {
        roots.push(PathBuf::from(path));
    }
    if ok {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        roots.push(PathBuf::from(String::from_utf16_lossy(&buf[..end])));
    }
    if let Some(path) = env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(path).join("Steam"));
    }
    let installed = roots.clone();
    for root in installed {
        let file = root.join("steamapps").join("libraryfolders.vdf");
        if fs::metadata(&file).is_ok_and(|m| m.len() <= 2 * 1024 * 1024) {
            if let Ok(contents) = fs::read_to_string(file) {
                roots.extend(contents.lines().filter_map(|line| {
                    let value = line
                        .trim()
                        .strip_prefix("\"path\"")?
                        .trim()
                        .strip_prefix('"')?
                        .strip_suffix('"')?;
                    Some(PathBuf::from(value.replace("\\\\", "\\")))
                }));
            }
        }
    }
    roots.retain(|root| root.is_absolute());
    roots.sort();
    roots.dedup();
    roots
}

struct Handle(*mut std::ffi::c_void);

impl Handle {
    fn new(handle: *mut std::ffi::c_void) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}

fn request(
    host: &str,
    path: &str,
    post: Option<&[u8]>,
    limit: u64,
    out: &mut impl Write,
) -> io::Result<()> {
    unsafe {
        let agent = wide("OpenEngine/0.5");
        let session = Handle::new(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 5000, 5000, 10000, 10000);
        let host = wide(host);
        let server = Handle::new(WinHttpConnect(session.0, host.as_ptr(), 443, 0))?;
        let method = wide(if post.is_some() { "POST" } else { "GET" });
        let path = wide(path);
        let request = Handle::new(WinHttpOpenRequest(
            server.0,
            method.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let redirects = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        if WinHttpSetOption(
            request.0,
            WINHTTP_OPTION_REDIRECT_POLICY,
            (&redirects as *const u32).cast(),
            4,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let header = wide("Content-Type: application/x-www-form-urlencoded");
        let (header, length, body, bytes) = if let Some(body) = post {
            (
                header.as_ptr(),
                u32::MAX,
                body.as_ptr().cast(),
                body.len() as u32,
            )
        } else {
            (null(), 0, null(), 0)
        };
        if WinHttpSendRequest(request.0, header, length, body, bytes, bytes, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut status = 0u32;
        let mut size = 4u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if status != 200 {
            return Err(io::Error::other(format!("Steam HTTP {status}")));
        }
        let mut total = 0;
        let mut buffer = [0u8; 32 * 1024];
        loop {
            let mut n = 0u32;
            if WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut n,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            if n == 0 {
                return Ok(());
            }
            total += u64::from(n);
            if total > limit {
                return Err(io::Error::other(t(
                    "Файл Steam слишком большой.",
                    "The Steam file is too large.",
                    "Steam 文件过大。",
                )));
            }
            out.write_all(&buffer[..n as usize])?;
        }
    }
}
