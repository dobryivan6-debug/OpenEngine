use crate::i18n::t;
use crate::{data, video::Video, win::*};
use std::{
    cell::RefCell,
    env,
    mem::{size_of, zeroed},
    process::Command,
    ptr::{null, null_mut},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Registry::*, Threading::CreateMutexW},
    UI::{Shell::*, WindowsAndMessaging::*},
};

const CONTROL: &str = "OpenEngineControl";
const SURFACE: &str = "OpenEngineWallpaperSurface";
const TRAY: u32 = WM_APP + 1;
const SHOW_LIBRARY: u32 = WM_APP + 2;
const FRAME: usize = 1;
const WATCH: usize = 2;

thread_local! { static ENGINE: RefCell<Engine> = RefCell::new(Engine::default()); }

#[derive(Default)]
struct Engine {
    surface: HWND,
    host: HWND,
    paused: bool,
    animating: bool,
    active: String,
    quality: crate::video::Quality,
    video: Option<Video>,
    still: Option<crate::video::Frame>,
    scene: Option<crate::scene_gl::Renderer>,
    audio: crate::audio::Audio,
    tick: u32,
    taskbar_created: u32,
    size: (i32, i32),
    frames: u32,
    fps_since: Option<Instant>,
}

pub unsafe fn run(open: bool) {
    let mutex = CreateMutexW(null(), 0, wide("Local\\OpenEngine.Native").as_ptr());
    if mutex.is_null() {
        return;
    }
    if GetLastError() == ERROR_ALREADY_EXISTS {
        if open {
            let existing = FindWindowW(wide(CONTROL).as_ptr(), null());
            if !existing.is_null() {
                PostMessageW(existing, SHOW_LIBRARY, 0, 0);
            }
        }
        CloseHandle(mutex);
        return;
    }
    register(CONTROL, Some(controller));
    register(SURFACE, Some(wallpaper));
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        wide(CONTROL).as_ptr(),
        wide("OpenEngine engine").as_ptr(),
        WS_POPUP,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if !hwnd.is_null() {
        ENGINE.with(|engine| {
            engine.borrow_mut().taskbar_created =
                RegisterWindowMessageW(wide("TaskbarCreated").as_ptr())
        });
        tray(hwnd, NIM_ADD);
        refresh(hwnd);
        SetTimer(hwnd, WATCH, 650, None);
        if open {
            open_library();
        }
        loop_messages();
        ENGINE.with(|engine| {
            let mut state = engine.borrow_mut();
            state.scene = None;
            state.audio = Default::default();
            state.video = None;
        });
    }
    CloseHandle(mutex);
}

unsafe fn tray(hwnd: HWND, command: NOTIFY_ICON_MESSAGE) {
    let mut info: NOTIFYICONDATAW = zeroed();
    info.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    info.hWnd = hwnd;
    info.uID = 1;
    info.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    info.uCallbackMessage = TRAY;
    if command == NIM_ADD {
        info.hIcon = icon();
    }
    let tip = wide("OpenEngine");
    info.szTip[..tip.len()].copy_from_slice(&tip);
    Shell_NotifyIconW(command, &info);
    if !info.hIcon.is_null() {
        DestroyIcon(info.hIcon);
    }
}

unsafe fn open_library() {
    let hwnd = FindWindowW(wide("OpenEngineLibrary").as_ptr(), null());
    if !hwnd.is_null() {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
        return;
    }
    if let Ok(exe) = env::current_exe() {
        let _ = Command::new(exe).arg("--library-ui").spawn();
    }
}

unsafe fn tray_menu(hwnd: HWND) {
    crate::i18n::set(data::language());
    let paused = ENGINE.with(|engine| engine.borrow().paused);
    let menu = CreatePopupMenu();
    AppendMenuW(
        menu,
        MF_STRING,
        1,
        wide(t("Библиотека обоев", "Wallpaper Library", "壁纸库")).as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING | if paused { MF_CHECKED } else { 0 },
        2,
        wide(t("Пауза", "Pause", "暂停")).as_ptr(),
    );
    let autostart = startup_enabled();
    AppendMenuW(
        menu,
        MF_STRING | if autostart { MF_CHECKED } else { 0 },
        4,
        wide(t("Запускать с Windows", "Start with Windows", "开机启动")).as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(
        menu,
        MF_STRING,
        3,
        wide(t("Выход", "Quit", "退出")).as_ptr(),
    );
    let mut point = zeroed();
    GetCursorPos(&mut point);
    SetForegroundWindow(hwnd);
    let result = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        0,
        hwnd,
        null(),
    );
    DestroyMenu(menu);
    PostMessageW(hwnd, WM_NULL, 0, 0);
    match result {
        1 => open_library(),
        2 => {
            ENGINE.with(|engine| engine.borrow_mut().paused = !paused);
            refresh(hwnd);
        }
        3 => {
            DestroyWindow(hwnd);
        }
        4 => {
            if !set_startup(!autostart) {
                message(
                    hwnd,
                    t(
                        "Не удалось изменить автозапуск.",
                        "Could not change startup settings.",
                        "无法更改开机启动设置。",
                    ),
                );
            }
        }
        _ => {}
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "OpenEngineWallpaper";

unsafe fn startup_enabled() -> bool {
    RegGetValueW(
        HKEY_CURRENT_USER,
        wide(RUN_KEY).as_ptr(),
        wide(RUN_NAME).as_ptr(),
        RRF_RT_REG_SZ,
        null_mut(),
        null_mut(),
        &mut 0,
    ) == ERROR_SUCCESS
}

unsafe fn set_startup(enabled: bool) -> bool {
    let mut key = null_mut();
    if RegOpenKeyExW(
        HKEY_CURRENT_USER,
        wide(RUN_KEY).as_ptr(),
        0,
        KEY_SET_VALUE,
        &mut key,
    ) != ERROR_SUCCESS
    {
        return false;
    }
    let name = wide(RUN_NAME);
    let result = if enabled {
        env::current_exe().ok().is_some_and(|exe| {
            use std::os::windows::ffi::OsStrExt;
            let line: Vec<u16> = std::iter::once(34)
                .chain(exe.as_os_str().encode_wide())
                .chain(Some(34))
                .chain(" --background".encode_utf16())
                .chain(Some(0))
                .collect();
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                line.as_ptr().cast(),
                (line.len() * 2) as u32,
            ) == ERROR_SUCCESS
        })
    } else {
        let code = RegDeleteValueW(key, name.as_ptr());
        code == ERROR_SUCCESS || code == ERROR_FILE_NOT_FOUND
    };
    RegCloseKey(key);
    result
}

unsafe extern "system" fn controller(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match message {
        TRAY => {
            match lp as u32 {
                WM_RBUTTONUP => tray_menu(hwnd),
                WM_LBUTTONDBLCLK => open_library(),
                _ => {}
            }
            0
        }
        SHOW_LIBRARY => {
            open_library();
            0
        }
        crate::perf::WALLPAPER_FPS => ENGINE.with(|engine| {
            let mut state = engine.borrow_mut();
            let now = Instant::now();
            let frames = std::mem::take(&mut state.frames);
            state.fps_since.replace(now).map_or(0, |from| {
                let elapsed = now.duration_since(from).as_secs_f64();
                if elapsed < 0.05 {
                    0
                } else {
                    (f64::from(frames) * 10.0 / elapsed).round() as LRESULT + 1
                }
            })
        }),
        WM_TIMER => {
            if wp == WATCH {
                refresh(hwnd);
            } else if wp == FRAME {
                let interactive = desktop_focused();
                let surface = ENGINE.with(|engine| {
                    let mut state = engine.borrow_mut();
                    let updated = if let Some(scene) = state.scene.as_mut() {
                        scene.draw(interactive)
                    } else if let Some(video) = state.video.as_mut() {
                        video.update()
                    } else {
                        state.tick = state.tick.wrapping_add(1);
                        true
                    };
                    if updated {
                        if !state.surface.is_null() {
                            state.frames = state.frames.saturating_add(1);
                        }
                        state.surface
                    } else {
                        null_mut()
                    }
                });
                if !surface.is_null() {
                    InvalidateRect(surface, null(), 0);
                }
            }
            0
        }
        WM_DESTROY => {
            tray(hwnd, NIM_DELETE);
            ENGINE.with(|engine| {
                let mut state = engine.borrow_mut();
                state.video.take();
                state.scene.take();
                state.audio = Default::default();
                if IsWindow(state.surface) != 0 {
                    DestroyWindow(state.surface);
                }
            });
            PostQuitMessage(0);
            0
        }
        _ => {
            let created = ENGINE.with(|engine| {
                engine
                    .try_borrow()
                    .map(|state| state.taskbar_created)
                    .unwrap_or(0)
            });
            if created != 0 && message == created {
                tray(hwnd, NIM_ADD);
                return 0;
            }
            DefWindowProcW(hwnd, message, wp, lp)
        }
    }
}

unsafe fn refresh(owner: HWND) {
    let size = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    let id = data::active();
    let quality = data::quality();
    let volume = data::volume();
    let covered = desktop_covered(size);
    let setup = ENGINE.with(|engine| {
        let state = engine.borrow();
        state.surface.is_null() || IsWindow(state.surface) == 0 || IsWindow(state.host) == 0
    });
    if setup {
        if let Some((surface, host)) = attach(size) {
            ENGINE.with(|engine| {
                let mut state = engine.borrow_mut();
                state.active.clear();
                state.surface = surface;
                state.host = host;
                state.size = size;
            });
        }
    }
    let mut resize = null_mut();
    let mut invalidate = null_mut();
    let mut fallback = false;
    let mut scene_error = None;
    ENGINE.with(|engine| {
        let mut state = engine.borrow_mut();
        if state.size != size {
            resize = state.surface;
            state.size = size;
        }
        if (state.active != id || state.quality != quality)
            && !state.surface.is_null()
            && IsWindow(state.surface) != 0
        {
            state.quality = quality;
            state.video = None;
            state.still = None;
            state.scene = None;
            state.audio = Default::default();
            if state.animating {
                KillTimer(owner, FRAME);
                state.animating = false;
            }
            if let Some(path) = data::media_for(&id) {
                if data::is_still(&path) {
                    state.still = data::read_still(&path);
                } else if data::is_scene(&path) {
                    match crate::scene_gl::Renderer::start(
                        state.surface,
                        &path,
                        quality == crate::video::Quality::Low,
                    ) {
                        Ok(mut scene) => {
                            scene.draw(false);
                            state.scene = Some(scene);
                            state.audio = crate::audio::Audio::open(&path);
                        }
                        Err(error) => scene_error = Some(error),
                    }
                } else {
                    state.video = Video::start(&path, quality).ok();
                }
            }
            if id != "winter"
                && state.video.is_none()
                && state.still.is_none()
                && state.scene.is_none()
            {
                fallback = true;
            }
            state.active = id;
            invalidate = state.surface;
        }
        if state.video.as_ref().is_some_and(Video::failed) {
            state.video.take();
            invalidate = state.surface;
            fallback = true;
        }
        let play = !state.paused
            && !covered
            && state.still.is_none()
            && !state.surface.is_null()
            && IsWindow(state.surface) != 0;
        if let Some(video) = state.video.as_mut() {
            video.play(play);
        }
        state.audio.update(play, volume);
        let interval = if state.scene.is_some() && quality != crate::video::Quality::Low {
            33
        } else {
            66
        };
        if state.animating != play {
            if play {
                if let Some(scene) = state.scene.as_mut() {
                    scene.reset_clock();
                }
                SetTimer(owner, FRAME, interval, None);
            } else {
                KillTimer(owner, FRAME);
            }
            state.animating = play;
        }
    });
    if !resize.is_null() {
        SetWindowPos(
            resize,
            null_mut(),
            0,
            0,
            size.0,
            size.1,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    if !invalidate.is_null() {
        InvalidateRect(invalidate, null(), 0);
    }
    if fallback {
        let _ = data::select("winter");
    }
    if let Some(error) = scene_error {
        crate::i18n::set(data::language());
        message(
            owner,
            &format!(
                "{}\n{error}",
                t(
                    "Не удалось запустить сцену.",
                    "Could not start the scene.",
                    "无法启动场景。",
                )
            ),
        );
    }
}

unsafe fn attach(size: (i32, i32)) -> Option<(HWND, HWND)> {
    let progman = FindWindowW(wide("Progman").as_ptr(), null());
    if progman.is_null() {
        return None;
    }
    SendMessageTimeoutW(progman, 0x052c, 0x0d, 1, SMTO_ABORTIFHUNG, 850, null_mut());
    let icons = FindWindowExW(
        progman,
        null_mut(),
        wide("SHELLDLL_DefView").as_ptr(),
        null(),
    );
    let (host, icons) = if icons.is_null() {
        let mut pair: (HWND, HWND) = (null_mut(), null_mut());
        EnumWindows(Some(find_shell), &mut pair as *mut (HWND, HWND) as LPARAM);
        pair
    } else {
        (progman, icons)
    };
    if host.is_null() {
        return None;
    }
    let flags =
        WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | if icons.is_null() { 0 } else { WS_EX_LAYERED };
    let hwnd = CreateWindowExW(
        flags,
        wide(SURFACE).as_ptr(),
        wide("OpenEngine wallpaper").as_ptr(),
        WS_POPUP,
        0,
        0,
        size.0,
        size.1,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if hwnd.is_null() {
        return None;
    }
    SetWindowLongW(
        hwnd,
        GWL_STYLE,
        ((GetWindowLongW(hwnd, GWL_STYLE) as u32 & !WS_POPUP) | WS_CHILD) as i32,
    );
    SetParent(hwnd, host);
    if GetParent(hwnd) != host {
        DestroyWindow(hwnd);
        return None;
    }
    if !icons.is_null() {
        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
    }
    let position = if icons.is_null() { HWND_BOTTOM } else { icons };
    SetWindowPos(
        hwnd,
        position,
        0,
        0,
        size.0,
        size.1,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    Some((hwnd, host))
}

unsafe extern "system" fn find_shell(window: HWND, args: LPARAM) -> i32 {
    let icons = FindWindowExW(
        window,
        null_mut(),
        wide("SHELLDLL_DefView").as_ptr(),
        null(),
    );
    if icons.is_null() {
        return 1;
    }
    let host = FindWindowExW(window, null_mut(), wide("WorkerW").as_ptr(), null());
    let result = &mut *(args as *mut (HWND, HWND));
    *result = if host.is_null() {
        (
            FindWindowExW(null_mut(), window, wide("WorkerW").as_ptr(), null()),
            null_mut(),
        )
    } else {
        (window, icons)
    };
    0
}

unsafe fn desktop_focused() -> bool {
    let window = GetForegroundWindow();
    let mut name = [0u16; 64];
    let n = GetClassNameW(window, name.as_mut_ptr(), name.len() as i32);
    n > 0
        && [wide("Progman"), wide("WorkerW")]
            .iter()
            .any(|class| class.as_slice() == &name[..n as usize + 1])
}

unsafe fn desktop_covered(size: (i32, i32)) -> bool {
    let window = GetForegroundWindow();
    if window.is_null() || desktop_focused() {
        return false;
    }
    let mut area: RECT = zeroed();
    GetWindowRect(window, &mut area) != 0
        && area.left <= 8
        && area.top <= 8
        && area.right >= size.0 - 8
        && area.bottom >= size.1 - 55
}

unsafe extern "system" fn wallpaper(hwnd: HWND, event: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match event {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint = zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            paint_wallpaper(dc, client(hwnd));
            EndPaint(hwnd, &paint);
            0
        }
        WM_PRINTCLIENT => {
            paint_wallpaper(wp as HDC, client(hwnd));
            0
        }
        _ => DefWindowProcW(hwnd, event, wp, lp),
    }
}

unsafe fn paint_wallpaper(dc: HDC, area: RECT) {
    ENGINE.with(|engine| {
        let Ok(state) = engine.try_borrow() else {
            return;
        };
        if let Some(scene) = &state.scene {
            scene.paint(dc, area);
        } else if let Some(frame) = state
            .still
            .as_ref()
            .or_else(|| state.video.as_ref().and_then(|video| video.frame.as_ref()))
        {
            bitmap_size(dc, area, &frame.pixels, frame.width, frame.height);
        } else {
            bitmap(dc, area, WINTER);
            if state.video.is_none() {
                snow(dc, area, state.tick);
            }
        }
    });
}

unsafe fn snow(dc: HDC, area: RECT, tick: u32) {
    let brush = CreateSolidBrush(rgb(213, 231, 244));
    let previous = SelectObject(dc, brush);
    let previous_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    let (width, height) = (area.right, area.bottom);
    if width > 0 && height > 0 {
        for index in 1u32..61 {
            let seed = index.wrapping_mul(2_654_435_761);
            let x = (seed.wrapping_add((tick / 3).wrapping_mul(821_211)) % 100_003) as i32 * width
                / 100_003;
            let y = (seed
                .rotate_left(9)
                .wrapping_add(tick.wrapping_mul(101 + seed % 410))
                % 100_003) as i32
                * height
                / 100_003;
            let radius = 2 + (seed % 3) as i32;
            Ellipse(dc, x, y, x + radius, y + radius);
        }
    }
    SelectObject(dc, previous_pen);
    SelectObject(dc, previous);
    DeleteObject(brush);
}
