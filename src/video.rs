use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const WIDTH: usize = 960;
pub const HEIGHT: usize = 540;
pub const THUMB_WIDTH: usize = 320;
pub const THUMB_HEIGHT: usize = 180;
const STEP: Duration = Duration::from_millis(66);
const MAX_JPEG: usize = 1_500_000;
const FRAME_SIZE: usize = WIDTH * HEIGHT * 3;
const CACHE_NAME: &str = "openengine-960x540-15.mjpeg";
const BOUNDARY: &[u8] = b"--openengine";
type Shared = Arc<(Mutex<State>, Condvar)>;

struct State {
    playing: bool,
    cancelled: bool,
    failed: bool,
    latest: Option<Vec<u8>>,
}

pub struct Video {
    worker: Option<JoinHandle<()>>,
    shared: Shared,
    pub frame: Option<Vec<u8>>,
    playing: bool,
}

fn find_ffmpeg() -> Option<PathBuf> {
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
    let executable = find_ffmpeg()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "FFmpeg отсутствует в PATH."))?;
    let format = if path.extension().is_some_and(|ext| ext == "wmv") {
        "asf"
    } else {
        "mov"
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

pub fn prepare_cache(path: &Path, cancelled: impl Fn() -> bool) -> io::Result<PathBuf> {
    let target = path.with_file_name(CACHE_NAME);
    if target.is_file() {
        return Ok(target);
    }
    let temp = path.with_file_name(format!("{CACHE_NAME}.{}.tmp", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        let mut child = ffmpeg(path)?.args([
            "-vf", "scale=960:540:force_original_aspect_ratio=increase:flags=lanczos,crop=960:540,setsar=1,fps=15",
            "-threads", "1", "-c:v", "mjpeg", "-q:v", "8", "-pix_fmt", "yuvj420p",
            "-f", "mpjpeg", "-boundary_tag", "openengine", "pipe:1",
        ]).stdout(Stdio::from(file)).spawn()?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Подготовка видео отменена.",
                ));
            }
            thread::sleep(Duration::from_millis(70));
        };
        if !status.success() || fs::metadata(&temp)?.len() < 100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Не удалось подготовить видео. Попробуй MP4 с H.264.",
            ));
        }
        match fs::rename(&temp, &target) {
            Ok(()) => Ok(target),
            Err(_) if target.is_file() => Ok(target),
            Err(error) => Err(error),
        }
    })();
    let _ = fs::remove_file(temp);
    result
}

impl Video {
    pub fn available(path: &Path) -> bool {
        path.with_file_name(CACHE_NAME).is_file() || find_ffmpeg().is_some()
    }

    pub fn start(path: &Path) -> io::Result<Self> {
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
                if render_loop(&path, &state).is_err() {
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

fn render_loop(source: &Path, shared: &Shared) -> io::Result<()> {
    let (lock, wake) = &**shared;
    let cache = prepare_cache(source, || lock.lock().unwrap().cancelled)?;
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
        if usize::from(info.width) != WIDTH
            || usize::from(info.height) != HEIGHT
            || info.pixel_format != jpeg_decoder::PixelFormat::RGB24
        {
            return Err(invalid());
        }
        let mut pixels = decoder.decode().map_err(|_| invalid())?;
        if pixels.len() != FRAME_SIZE {
            return Err(invalid());
        }
        for pixel in pixels.chunks_exact_mut(3) {
            pixel.swap(0, 2);
        }
        let mut state = lock.lock().unwrap();
        state.latest = Some(pixels);
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
        "Повреждён кеш видео OpenEngine.",
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
