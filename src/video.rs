use crate::i18n::t;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::{AtomicU64, Ordering}, Arc, Condvar, Mutex, MutexGuard, TryLockError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const THUMB_WIDTH: usize = 320;
pub const THUMB_HEIGHT: usize = 180;
const STEP: Duration = Duration::from_millis(66);
const MAX_JPEG: usize = 32 * 1024 * 1024;

// Serialize cache creation in this process. Preparation is infrequent, so a
// global lock avoids path-key normalization and per-path lock-lifetime issues.
static CACHE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn lock_cache(cancelled: &impl Fn() -> bool) -> io::Result<MutexGuard<'static, ()>> {
    loop {
        if cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Video preparation cancelled."));
        }
        match CACHE_LOCK.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(25)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    #[default]
    Detail,
    Low,
}

impl Quality {
    fn cache(self) -> &'static str {
        match self {
            Self::Detail => "openengine-detail-v1.mjpeg",
            Self::Low => "openengine-960x540-15.mjpeg",
        }
    }

    fn filter(self) -> &'static str {
        match self {
            Self::Detail => "scale=w='min(iw,3840)':h='min(ih,2160)':force_original_aspect_ratio=decrease:force_divisible_by=4:flags=lanczos,setsar=1,fps=15",
            Self::Low => "scale=960:540:force_original_aspect_ratio=increase:flags=lanczos,crop=960:540,setsar=1,fps=15",
        }
    }
}

pub struct Frame {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}
const BOUNDARY: &[u8] = b"--openengine";
type Shared = Arc<(Mutex<State>, Condvar)>;

struct State {
    playing: bool,
    cancelled: bool,
    failed: bool,
    latest: Option<Frame>,
}

pub struct Video {
    worker: Option<JoinHandle<()>>,
    shared: Shared,
    pub frame: Option<Frame>,
    playing: bool,
}

pub(crate) fn find_ffmpeg() -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("ffmpeg.exe"))
            .find(|file| file.is_file())
    })
}

pub fn thumbnail(path: &Path) -> Option<Vec<u8>> {
    let output = ffmpeg(path)
        .ok()?
        .args([
            "-vf",
            "scale=320:180:force_original_aspect_ratio=increase,crop=320:180",
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-pix_fmt",
            "bgr24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .ok()?;
    (output.status.success() && output.stdout.len() == THUMB_WIDTH * THUMB_HEIGHT * 3)
        .then_some(output.stdout)
}

fn ffmpeg(path: &Path) -> io::Result<Command> {
    let executable = find_ffmpeg().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            t(
                "FFmpeg отсутствует в PATH.",
                "FFmpeg was not found on PATH.",
                "PATH 中未找到 FFmpeg。",
            ),
        )
    })?;
    let format = match path.extension().and_then(|ext| ext.to_str()) {
        Some("wmv") => "asf",
        Some("webm" | "mkv") => "matroska",
        _ => "mov",
    };
    let mut command = Command::new(executable);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-threads",
            "1",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            format,
            "-i",
        ])
        .arg(path)
        .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-filter_threads", "1"])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

pub fn prepare_cache(
    path: &Path,
    quality: Quality,
    cancelled: impl Fn() -> bool,
) -> io::Result<PathBuf> {
    let _cache_guard = lock_cache(&cancelled)?;
    let target = path.with_file_name(quality.cache());
    if target.is_file() {
        return Ok(target);
    }

    // Unique create_new names avoid collisions with stale files and other processes.
    let (temp, file) = loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = path.with_file_name(format!(
            "{}.{}.{}.tmp",
            quality.cache(),
            std::process::id(),
            sequence
        ));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };

    let result = (|| {
        let mut command = ffmpeg(path)?;
        command
            .args([
                "-vf",
                quality.filter(),
                "-threads",
                "1",
                "-c:v",
                "mjpeg",
                "-q:v",
                if quality == Quality::Detail { "2" } else { "8" },
                "-pix_fmt",
                if quality == Quality::Detail { "yuvj444p" } else { "yuvj420p" },
                "-f",
                "mpjpeg",
                "-boundary_tag",
                "openengine",
                "pipe:1",
            ])
            .stdout(Stdio::from(file));

        let mut child = command.spawn()?;
        // Do not retain the parent's copy of the stdout handle during rename.
        drop(command);

        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    t(
                        "Подготовка видео отменена.",
                        "Video preparation cancelled.",
                        "视频准备已取消。",
                    ),
                ));
            }
            thread::sleep(Duration::from_millis(70));
        };

        if !status.success() || fs::metadata(&temp)?.len() < 100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                t(
                    "Не удалось подготовить видео. Попробуй MP4 с H.264.",
                    "Could not prepare video. Try an H.264 MP4.",
                    "无法准备视频，请尝试 H.264 编码的 MP4。",
                ),
            ));
        }

        // A failed replacement leaves the existing target untouched.
        fs::rename(&temp, &target)?;
        Ok(target.clone())
    })();

    let _ = fs::remove_file(&temp);
    result
}

impl Video {
    pub fn available(path: &Path, quality: Quality) -> bool {
        path.with_file_name(quality.cache()).is_file() || find_ffmpeg().is_some()
    }

    pub fn start(path: &Path, quality: Quality) -> io::Result<Self> {
        let shared = Arc::new((
            Mutex::new(State {
                playing: false,
                cancelled: false,
                failed: false,
                latest: None,
            }),
            Condvar::new(),
        ));
        let state = Arc::clone(&shared);
        let path = path.to_owned();
        let worker = thread::Builder::new()
            .name("video-frames".into())
            .spawn(move || {
                if render_loop(&path, quality, &state).is_err() {
                    state.0.lock().unwrap().failed = true;
                }
            })?;
        Ok(Self {
            worker: Some(worker),
            shared,
            frame: None,
            playing: false,
        })
    }

    pub fn play(&mut self, playing: bool) {
        if self.playing == playing {
            return;
        }
        self.playing = playing;
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap().playing = playing;
        wake.notify_one();
    }

    pub fn update(&mut self) -> bool {
        if let Some(frame) = self.shared.0.lock().unwrap().latest.take() {
            self.frame = Some(frame);
            return true;
        }
        false
    }

    pub fn failed(&self) -> bool {
        self.shared.0.lock().unwrap().failed
    }
}

fn render_loop(source: &Path, quality: Quality, shared: &Shared) -> io::Result<()> {
    let (lock, wake) = &**shared;
    let cache = prepare_cache(source, quality, || lock.lock().unwrap().cancelled)?;
    let mut input = BufReader::new(File::open(cache)?);
    let mut jpeg = Vec::new();
    loop {
        let state = wake
            .wait_while(lock.lock().unwrap(), |s| !s.playing && !s.cancelled)
            .unwrap();
        if state.cancelled {
            return Ok(());
        }
        drop(state);
        let now = Instant::now();
        if !read_frame(&mut input, &mut jpeg)? {
            input.seek(SeekFrom::Start(0))?;
            if !read_frame(&mut input, &mut jpeg)? {
                return Err(invalid());
            }
        }
        let mut decoder = jpeg_decoder::Decoder::new(jpeg.as_slice());
        decoder.read_info().map_err(|_| invalid())?;
        let info = decoder.info().ok_or_else(invalid)?;
        let (width, height) = (usize::from(info.width), usize::from(info.height));
        if width == 0
            || width > 3840
            || width % 4 != 0
            || height == 0
            || height > 2160
            || info.pixel_format != jpeg_decoder::PixelFormat::RGB24
        {
            return Err(invalid());
        }
        let mut pixels = decoder.decode().map_err(|_| invalid())?;
        if pixels.len() != width * height * 3 {
            return Err(invalid());
        }
        for pixel in pixels.chunks_exact_mut(3) {
            pixel.swap(0, 2);
        }
        let mut state = lock.lock().unwrap();
        state.latest = Some(Frame {
            pixels,
            width,
            height,
        });
        let _ = wake
            .wait_timeout_while(state, STEP.saturating_sub(now.elapsed()), |s| {
                s.playing && !s.cancelled
            })
            .unwrap();
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        t(
            "Повреждён кеш видео OpenEngine.",
            "The OpenEngine video cache is damaged.",
            "OpenEngine 视频缓存已损坏。",
        ),
    )
}

fn read_frame(input: &mut impl BufRead, frame: &mut Vec<u8>) -> io::Result<bool> {
    let mut line = Vec::with_capacity(64);
    fn get_line(input: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<usize> {
        line.clear();
        input.take(512).read_until(b'\n', line)
    }
    loop {
        if get_line(input, &mut line)? == 0 {
            return Ok(false);
        }
        if line.strip_suffix(b"\r\n") == Some(BOUNDARY) {
            break;
        }
        if line != b"\r\n" {
            return Err(invalid());
        }
    }
    if get_line(input, &mut line)? == 0 {
        return Ok(false);
    }
    if line != b"Content-type: image/jpeg\r\n" {
        return Err(invalid());
    }
    get_line(input, &mut line)?;
    let length = std::str::from_utf8(&line)
        .ok()
        .and_then(|line| line.trim_end().strip_prefix("Content-length: "))
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|&n| n > 0 && n <= MAX_JPEG)
        .ok_or_else(invalid)?;
    get_line(input, &mut line)?;
    if line != b"\r\n" {
        return Err(invalid());
    }
    frame.resize(length, 0);
    input.read_exact(frame)?;
    Ok(true)
}

impl Drop for Video {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap().cancelled = true;
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
