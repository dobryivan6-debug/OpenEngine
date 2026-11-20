use crate::i18n::t;
use crate::{
    data::{self, Entry},
    video::Video,
    win::*,
};
use std::{
    cell::RefCell,
    ffi::OsString,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::Mutex,
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{
        Controls::{Dialogs::*, DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED},
        Input::KeyboardAndMouse::EnableWindow,
        Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP},
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "OpenEngineLibrary";
const LIST: usize = 101;
const IMPORT: usize = 102;
const APPLY: usize = 103;
const THEME: usize = 104;
const IMPORT_DONE: u32 = WM_APP + 3;
const SEARCH: usize = 105;
const GALLERY: usize = 106;
const SETTINGS: usize = 107;
const QUALITY: usize = 108;
const FILTER: usize = 109;
const LANGUAGE: usize = 112;
const VOLUME: usize = 113;
const WORKSHOP: usize = 114;
const CARD_W: i32 = 208;
const CARD_H: i32 = 174;

struct Palette {
    canvas: COLORREF,
    panel: COLORREF,
    card: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    accent: COLORREF,
}
const THEMES: [Palette; 2] = [
    Palette {
        canvas: rgb(243, 247, 253),
        panel: rgb(255, 255, 255),
        card: rgb(218, 234, 252),
        text: rgb(29, 48, 72),
        muted: rgb(73, 96, 123),
        accent: rgb(245, 199, 224),
    },
    Palette {
        canvas: rgb(16, 22, 32),
        panel: rgb(24, 33, 47),
        card: rgb(41, 57, 80),
        text: rgb(236, 242, 250),
        muted: rgb(170, 190, 215),
        accent: rgb(87, 66, 100),
    },
];
const LANGUAGES: [&str; 3] = ["Русский", "English", "简体中文"];

thread_local! { static UI: RefCell<Option<Library>> = const { RefCell::new(None) }; }
struct ImportResult {
    last_id: Option<String>,
    added: usize,
    problems: Vec<String>,
}
static RESULT: Mutex<Option<ImportResult>> = Mutex::new(None);

struct Library {
    entries: Vec<Entry>,
    selected: usize,
    theme: usize,
    active: String,
    status: String,
    busy: bool,
    list: HWND,
    import: HWND,
    apply: HWND,
    combo: HWND,
    language: HWND,
    body: HFONT,
    title: HFONT,
    display: HFONT,
    brush: HBRUSH,
    search: HWND,
    gallery: HWND,
    settings: HWND,
    quality: HWND,
    preferences: bool,
    query: String,
    filter: usize,
    filters: [HWND; 3],
    metrics: std::sync::mpsc::Receiver<crate::perf::Snapshot>,
    snapshot: crate::perf::Snapshot,
    mark: HICON,
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.mark);
            DeleteObject(self.brush);
            for font in [self.body, self.title, self.display] {
                DeleteObject(font);
            }
        }
    }
}

pub unsafe fn run() {
    let mutex = CreateMutexW(null(), 0, wide("Local\\OpenEngine.Library.Native").as_ptr());
    if mutex.is_null() {
        return;
    }
    if GetLastError() == ERROR_ALREADY_EXISTS {
        let hwnd = FindWindowW(wide(CLASS).as_ptr(), null());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
        CloseHandle(mutex);
        return;
    }
    register(CLASS, Some(procedure));
    let width = 1296;
    let height = 824;
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    let hwnd = CreateWindowExW(
        0,
        wide(CLASS).as_ptr(),
        wide("OpenEngine").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        (screen_w - width) / 2,
        (screen_h - height) / 2,
        width,
        height,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if !hwnd.is_null() {
        init(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        UI.with(|state| state.borrow_mut().take());
    }
    CloseHandle(mutex);
}

unsafe fn make_font(size: i32, weight: i32) -> HFONT {
    CreateFontW(
        -size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        1,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide("Segoe UI").as_ptr(),
    )
}

unsafe fn control(parent: HWND, id: usize, class: &str, caption: &str, style: u32) -> HWND {
    CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(caption).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
        0,
        0,
        0,
        0,
        parent,
        id as HMENU,
        GetModuleHandleW(null()),
        null(),
    )
}

unsafe fn init(hwnd: HWND) {
    let theme = data::theme();
    let list = control(
        hwnd,
        LIST,
        "LISTBOX",
        "",
        LBS_NOTIFY as u32
            | LBS_NOINTEGRALHEIGHT as u32
            | LBS_HASSTRINGS as u32
            | LBS_OWNERDRAWFIXED as u32
            | LBS_MULTICOLUMN as u32
            | WS_HSCROLL,
    );
    let import = control(
        hwnd,
        IMPORT,
        "BUTTON",
        t("+  Добавить", "+  Add", "+  添加"),
        BS_OWNERDRAW as u32,
    );
    let apply = control(
        hwnd,
        APPLY,
        "BUTTON",
        t("На рабочий стол", "Set wallpaper", "设为壁纸"),
        BS_OWNERDRAW as u32,
    );
    let combo = control(hwnd, THEME, "COMBOBOX", "", CBS_DROPDOWNLIST as u32);
    let language = control(hwnd, LANGUAGE, "COMBOBOX", "", CBS_DROPDOWNLIST as u32);
    for label in LANGUAGES {
        SendMessageW(language, CB_ADDSTRING, 0, wide(label).as_ptr() as LPARAM);
    }
    SendMessageW(language, CB_SETCURSEL, data::language(), 0);
    let gallery = control(
        hwnd,
        GALLERY,
        "BUTTON",
        t("Библиотека", "Library", "壁纸库"),
        BS_OWNERDRAW as u32,
    );
    let settings = control(
        hwnd,
        SETTINGS,
        "BUTTON",
        t("Настройки", "Settings", "设置"),
        BS_OWNERDRAW as u32,
    );
    let search = control(hwnd, SEARCH, "EDIT", "", WS_BORDER | ES_AUTOHSCROLL as u32);
    let volume = control(hwnd, VOLUME, "COMBOBOX", "", CBS_DROPDOWNLIST as u32);
    let workshop = control(
        hwnd,
        WORKSHOP,
        "BUTTON",
        "Steam Workshop",
        BS_OWNERDRAW as u32,
    );
    let quality = control(hwnd, QUALITY, "COMBOBOX", "", CBS_DROPDOWNLIST as u32);
    SendMessageW(
        search,
        0x1501,
        0,
        wide(t("Поиск по названию", "Search by name", "按名称搜索")).as_ptr() as LPARAM,
    );
    for label in [
        t("Детальное · до 4K", "Detail · Up to 4K", "高清 · 最高 4K"),
        t("Экономичное · 540p", "Low memory · 540p", "节省内存 · 540p"),
    ] {
        SendMessageW(quality, CB_ADDSTRING, 0, wide(label).as_ptr() as LPARAM);
    }
    SendMessageW(
        quality,
        CB_SETCURSEL,
        usize::from(data::quality() == crate::video::Quality::Low),
        0,
    );
    let filters = std::array::from_fn(|index| {
        control(hwnd, FILTER + index, "BUTTON", "", BS_OWNERDRAW as u32)
    });
    let body = make_font(16, 400);
    for control in [
        list, import, apply, combo, search, gallery, settings, quality, volume, workshop,
    ] {
        SendMessageW(control, WM_SETFONT, body as usize, 0);
    }
    for filter in filters {
        SendMessageW(filter, WM_SETFONT, body as usize, 0);
    }
    SendMessageW(list, LB_SETITEMHEIGHT, 0, CARD_H as LPARAM);
    SendMessageW(list, LB_SETCOLUMNWIDTH, CARD_W as usize, 0);
    for name in [t("Белая", "White", "白色"), t("Тёмная", "Dark", "深色")] {
        SendMessageW(combo, CB_ADDSTRING, 0, wide(name).as_ptr() as LPARAM);
    }
    SendMessageW(combo, CB_SETCURSEL, theme, 0);
    UI.with(|window| {
        *window.borrow_mut() = Some(Library {
            entries: vec![],
            selected: 0,
            theme,
            active: data::active(),
            status: String::new(),
            busy: false,
            list,
            import,
            apply,
            combo,
            language,
            body,
            title: make_font(21, 600),
            display: make_font(32, 650),
            brush: CreateSolidBrush(THEMES[theme].canvas),
            search,
            gallery,
            settings,
            quality,
            preferences: false,
            query: String::new(),
            filter: 0,
            filters,
            metrics: crate::perf::monitor(),
            snapshot: Default::default(),
            mark: crate::win::icon(),
        })
    });
    set_title_theme(hwnd, theme);
    update_language(hwnd);
    layout(hwnd);
    DragAcceptFiles(hwnd, 1);
    SetTimer(hwnd, 1, 1000, None);
}

unsafe fn set_title_theme(hwnd: HWND, theme: usize) {
    let dark = i32::from(theme == 1);
    DwmSetWindowAttribute(
        hwnd,
        20,
        (&dark as *const i32).cast(),
        size_of::<i32>() as u32,
    );
}

unsafe fn update_language(hwnd: HWND) {
    let captions = [
        (IMPORT, t("+  Добавить", "+  Add", "+  添加")),
        (GALLERY, t("Библиотека", "Library", "壁纸库")),
        (SETTINGS, t("Настройки", "Settings", "设置")),
        (FILTER, t("Все обои", "All", "全部")),
        (FILTER + 1, t("Сцены", "Scenes", "场景")),
        (FILTER + 2, t("Видео", "Videos", "视频")),
    ];
    for (id, label) in captions {
        let control = GetDlgItem(hwnd, id as i32);
        SetWindowTextW(control, wide(label).as_ptr());
        InvalidateRect(control, null(), 0);
    }
    let theme = GetDlgItem(hwnd, THEME as i32);
    let quality = GetDlgItem(hwnd, QUALITY as i32);
    for (control, labels, index) in [
        (
            theme,
            [t("Белая", "White", "白色"), t("Тёмная", "Dark", "深色")],
            data::theme(),
        ),
        (
            quality,
            [
                t("Детальное · до 4K", "Detail · Up to 4K", "高清 · 最高 4K"),
                t("Экономичное · 540p", "Low memory · 540p", "节省内存 · 540p"),
            ],
            usize::from(data::quality() == crate::video::Quality::Low),
        ),
    ] {
        SendMessageW(control, CB_RESETCONTENT, 0, 0);
        for label in labels {
            SendMessageW(control, CB_ADDSTRING, 0, wide(label).as_ptr() as LPARAM);
        }
        SendMessageW(control, CB_SETCURSEL, index, 0);
    }
    let volume = GetDlgItem(hwnd, VOLUME as i32);
    SendMessageW(volume, CB_RESETCONTENT, 0, 0);
    for label in [
        t("Выключен", "Muted", "静音").to_owned(),
        "25%".into(),
        "50%".into(),
        "75%".into(),
        "100%".into(),
    ] {
        SendMessageW(volume, CB_ADDSTRING, 0, wide(&label).as_ptr() as LPARAM);
    }
    SendMessageW(
        volume,
        CB_SETCURSEL,
        ((data::volume() + 12) / 25) as usize,
        0,
    );
    UI.with(|state| {
        if let Some(ui) = state.borrow_mut().as_mut() {
            ui.status.clear();
        }
    });
    reload(hwnd, None);
}

unsafe fn reload(hwnd: HWND, select: Option<&str>) {
    let entries = data::entries();
    let active = data::active();
    let (list, names, index) = UI.with(|window| {
        let mut window = window.borrow_mut();
        let Some(ui) = window.as_mut() else {
            return (null_mut(), vec![], 0);
        };
        ui.entries = entries
            .into_iter()
            .filter(|item| {
                item.title.to_lowercase().contains(&ui.query)
                    && match ui.filter {
                        1 => item
                            .path
                            .as_deref()
                            .is_none_or(|path| !data::is_video(path)),
                        2 => item.path.as_deref().is_some_and(data::is_video),
                        _ => true,
                    }
            })
            .collect();
        ui.active = active;
        ui.selected = ui
            .entries
            .iter()
            .position(|item| item.id == select.unwrap_or(&ui.active))
            .unwrap_or(0);
        (
            ui.list,
            ui.entries
                .iter()
                .map(|item| wide(&item.title))
                .collect::<Vec<_>>(),
            ui.selected,
        )
    });
    if list.is_null() {
        return;
    }
    SendMessageW(list, LB_RESETCONTENT, 0, 0);
    for name in names {
        SendMessageW(list, LB_ADDSTRING, 0, name.as_ptr() as LPARAM);
    }
    SendMessageW(list, LB_SETCURSEL, index, 0);
    layout(hwnd);
    update_controls(hwnd);
}

struct Layout {
    x: i32,
    inspector: i32,
    width: i32,
    height: i32,
}

impl Layout {
    fn new(area: RECT) -> Self {
        let width = area.right;
        Self {
            x: 226,
            inspector: width - (width * 31 / 100).max(301),
            width,
            height: area.bottom,
        }
    }
}

unsafe fn layout(hwnd: HWND) {
    let l = Layout::new(client(hwnd));
    let Some((
        list,
        import,
        apply,
        combo,
        search,
        gallery,
        settings,
        quality,
        preferences,
        filters,
        count,
        language,
    )) = UI.with(|state| {
        state.borrow().as_ref().map(|ui| {
            (
                ui.list,
                ui.import,
                ui.apply,
                ui.combo,
                ui.search,
                ui.gallery,
                ui.settings,
                ui.quality,
                ui.preferences,
                ui.filters,
                ui.entries.len(),
                ui.language,
            )
        })
    })
    else {
        return;
    };
    MoveWindow(gallery, 22, 108, 160, 44, 1);
    MoveWindow(settings, 22, 158, 160, 44, 1);
    MoveWindow(GetDlgItem(hwnd, WORKSHOP as i32), 22, 208, 160, 44, 1);
    MoveWindow(import, l.width - 172, 32, 146, 42, 1);
    MoveWindow(
        search,
        l.x + 65,
        116,
        (l.inspector - l.x - 95).min(330),
        30,
        1,
    );
    MoveWindow(
        list,
        l.x,
        209,
        l.inspector - l.x - 20,
        if count <= ((l.inspector - l.x - 20) / CARD_W).max(1) as usize {
            CARD_H + 18
        } else {
            (l.height - 270).max(190)
        },
        1,
    );
    MoveWindow(
        apply,
        l.inspector + 18,
        l.height - 104,
        l.width - l.inspector - 63,
        44,
        1,
    );
    let half = (l.width - l.x - 45) / 2;
    MoveWindow(combo, l.x + 24, 198, half.min(255), 180, 1);
    MoveWindow(language, l.x + half + 23, 198, half.min(255), 180, 1);
    MoveWindow(quality, l.x + 24, 371, 255, 180, 1);
    for (index, filter) in filters.into_iter().enumerate() {
        MoveWindow(filter, l.x + 107 * index as i32, 160, 99, 34, 1);
        ShowWindow(filter, if preferences { SW_HIDE } else { SW_SHOW });
        InvalidateRect(filter, null(), 0);
    }
    for control in [list, import, apply, search] {
        ShowWindow(control, if preferences { SW_HIDE } else { SW_SHOW });
    }
    let volume = GetDlgItem(hwnd, VOLUME as i32);
    MoveWindow(volume, l.x + 24, 522, 255, 150, 1);
    for control in [combo, quality, language, volume] {
        ShowWindow(control, if preferences { SW_SHOW } else { SW_HIDE });
    }
    for control in [gallery, settings] {
        InvalidateRect(control, null(), 1);
    }
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn update_controls(hwnd: HWND) {
    let Some((apply, enabled, active)) = UI.with(|state| {
        let state = state.borrow();
        state.as_ref().map(|ui| {
            let active = ui
                .entries
                .get(ui.selected)
                .is_some_and(|item| item.id == ui.active);
            (
                ui.apply,
                !active && !ui.busy && !ui.entries.is_empty(),
                active,
            )
        })
    }) else {
        return;
    };
    SetWindowTextW(
        apply,
        wide(if active {
            t("Установлено", "Applied", "已应用")
        } else {
            t("На рабочий стол", "Set wallpaper", "设为壁纸")
        })
        .as_ptr(),
    );
    EnableWindow(apply, i32::from(enabled));
    InvalidateRect(apply, null(), 0);
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn apply_selected(hwnd: HWND) {
    let item = UI.with(|state| {
        state.borrow().as_ref().and_then(|ui| {
            if ui.busy {
                None
            } else {
                ui.entries.get(ui.selected).cloned()
            }
        })
    });
    let Some(item) = item else { return };
    if item
        .path
        .as_deref()
        .is_some_and(|path| data::is_video(path) && !Video::available(path, data::quality()))
    {
        message(
            hwnd,
            t(
                "Для видео нужен FFmpeg в PATH. Установи его и перезапусти OpenEngine.",
                "Install FFmpeg, add it to PATH, and restart OpenEngine.",
                "请安装 FFmpeg，将其加入 PATH，然后重新启动 OpenEngine。",
            ),
        );
        return;
    }
    if let Err(error) = data::select(&item.id) {
        message(hwnd, &error.to_string());
        return;
    }
    UI.with(|state| {
        let mut state = state.borrow_mut();
        let ui = state.as_mut().unwrap();
        ui.active = item.id;
        ui.status = t("Обои установлены.", "Wallpaper applied.", "已应用壁纸。").into();
        InvalidateRect(ui.list, null(), 0);
    });
    update_controls(hwnd);
}

unsafe fn pick_video(hwnd: HWND) {
    if UI.with(|state| state.borrow().as_ref().is_some_and(|ui| ui.busy)) {
        return;
    }
    let filter = wide(&format!(
        "{}\0*.mp4;*.m4v;*.wmv;*.webm;*.mkv;*.pkg;*.zip;project.json\0",
        t(
            "Видео, сцены, ZIP и проекты",
            "Videos, scenes, ZIP and projects",
            "视频、场景、ZIP 与项目"
        )
    ));
    let mut file = vec![0u16; 32768];
    let mut dialog: OPENFILENAMEW = zeroed();
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = hwnd;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER;
    if GetOpenFileNameW(&mut dialog) == 0 {
        return;
    }
    let length = file.iter().position(|&x| x == 0).unwrap_or(file.len());
    let source = PathBuf::from(OsString::from_wide(&file[..length]));
    start_import(hwnd, vec![source]);
}

unsafe fn drop_files(hwnd: HWND, drop: HDROP) {
    let count = DragQueryFileW(drop, u32::MAX, null_mut(), 0);
    let mut paths = Vec::new();
    for index in 0..count {
        let length = DragQueryFileW(drop, index, null_mut(), 0);
        if length == 0 {
            continue;
        }
        let mut path = vec![0; length as usize + 1];
        let n = DragQueryFileW(drop, index, path.as_mut_ptr(), path.len() as u32);
        if n > 0 {
            paths.push(PathBuf::from(OsString::from_wide(&path[..n as usize])));
        }
    }
    DragFinish(drop);
    start_import(hwnd, paths);
}

unsafe fn start_import(hwnd: HWND, sources: Vec<PathBuf>) {
    if sources.is_empty() {
        return;
    }
    let import = UI.with(|state| {
        let mut state = state.borrow_mut();
        let Some(ui) = state.as_mut() else {
            return null_mut();
        };
        if ui.busy {
            ui.status = t(
                "Дождись окончания текущего импорта.",
                "Wait for the current import to finish.",
                "请等待当前导入完成。",
            )
            .into();
            return null_mut();
        }
        ui.busy = true;
        ui.status = t(
            "Подготавливаю обои…",
            "Preparing wallpapers…",
            "正在准备壁纸…",
        )
        .into();
        ui.import
    });
    if import.is_null() {
        InvalidateRect(hwnd, null(), 0);
        return;
    }
    EnableWindow(import, 0);
    update_controls(hwnd);
    let window = hwnd as usize;
    thread::spawn(move || {
        let mut result = ImportResult {
            last_id: None,
            added: 0,
            problems: Vec::new(),
        };
        for path in sources {
            match data::import(&path) {
                Ok(id) => {
                    result.last_id = Some(id);
                    result.added += 1;
                }
                Err(error) => result.problems.push(format!(
                    "{}: {error}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )),
            }
        }
        *RESULT.lock().unwrap() = Some(result);
        PostMessageW(window as HWND, IMPORT_DONE, 0, 0);
    });
}

unsafe fn import_done(hwnd: HWND) {
    let Some(result) = RESULT.lock().unwrap().take() else {
        return;
    };
    let (import, search) = UI.with(|state| {
        let mut state = state.borrow_mut();
        let ui = state.as_mut().unwrap();
        ui.busy = false;
        ui.status = if result.added > 0 {
            format!(
                "{}: {}",
                t(
                    "Добавлено в библиотеку",
                    "Added to library",
                    "已添加到壁纸库"
                ),
                result.added
            )
        } else {
            t(
                "Не удалось добавить обои.",
                "Could not import wallpapers.",
                "无法导入壁纸。",
            )
            .into()
        };
        if result.last_id.is_some() {
            ui.preferences = false;
            ui.filter = 0;
            ui.query.clear();
        }
        (ui.import, ui.search)
    });
    EnableWindow(import, 1);
    if let Some(id) = result.last_id {
        SetWindowTextW(search, wide("").as_ptr());
        reload(hwnd, Some(&id));
    }
    if !result.problems.is_empty() {
        message(hwnd, &result.problems.join("\n"));
    }
    update_controls(hwnd);
}

unsafe extern "system" fn procedure(hwnd: HWND, event: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match event {
        crate::workshop_ui::LIBRARY_CHANGED => {
            reload(hwnd, None);
            0
        }
        WM_DROPFILES => {
            drop_files(hwnd, wp as HDROP);
            0
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lp as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: 960, y: 660 };
            0
        }
        WM_SIZE => {
            layout(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint = zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            UI.with(|state| {
                if let Some(ui) = state.borrow().as_ref() {
                    paint_ui(dc, client(hwnd), ui);
                }
            });
            EndPaint(hwnd, &paint);
            0
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => UI.with(|state| {
            state.borrow().as_ref().map_or(0, |ui| {
                SetBkColor(wp as HDC, THEMES[ui.theme].canvas);
                SetTextColor(wp as HDC, THEMES[ui.theme].text);
                ui.brush as LRESULT
            })
        }),
        WM_DRAWITEM => {
            let args = &*(lp as *const DRAWITEMSTRUCT);
            UI.with(|state| {
                if let Ok(state) = state.try_borrow() {
                    if let Some(ui) = state.as_ref() {
                        draw_item(args, ui);
                    }
                }
            });
            1
        }
        WM_COMMAND => {
            let id = wp & 0xffff;
            let code = (wp >> 16) as u32;
            match id {
                LIST if code == LBN_SELCHANGE as u32 || code == LBN_DBLCLK as u32 => {
                    let index = SendMessageW(lp as HWND, LB_GETCURSEL, 0, 0);
                    if index >= 0 {
                        UI.with(|state| {
                            if let Some(ui) = state.borrow_mut().as_mut() {
                                ui.selected = index as usize;
                            }
                        });
                        update_controls(hwnd);
                        if code == LBN_DBLCLK as u32 {
                            apply_selected(hwnd);
                        }
                    }
                }
                APPLY if code == BN_CLICKED => apply_selected(hwnd),
                IMPORT if code == BN_CLICKED => pick_video(hwnd),
                GALLERY | SETTINGS if code == BN_CLICKED => {
                    UI.with(|state| {
                        if let Some(ui) = state.borrow_mut().as_mut() {
                            ui.preferences = id == SETTINGS;
                        }
                    });
                    layout(hwnd);
                }
                SEARCH if code == EN_CHANGE as u32 => {
                    let mut buffer = vec![0; GetWindowTextLengthW(lp as HWND).max(0) as usize + 1];
                    let n = GetWindowTextW(lp as HWND, buffer.as_mut_ptr(), buffer.len() as i32)
                        .max(0) as usize;
                    let query = String::from_utf16_lossy(&buffer[..n]).trim().to_lowercase();
                    UI.with(|state| {
                        if let Some(ui) = state.borrow_mut().as_mut() {
                            ui.query = query;
                        }
                    });
                    reload(hwnd, None);
                }
                id if (FILTER..FILTER + 3).contains(&id) && code == BN_CLICKED => {
                    UI.with(|state| {
                        if let Some(ui) = state.borrow_mut().as_mut() {
                            ui.filter = id - FILTER;
                        }
                    });
                    reload(hwnd, None);
                }
                WORKSHOP if code == BN_CLICKED => crate::workshop_ui::open(hwnd),
                VOLUME if code == CBN_SELCHANGE as u32 => {
                    let index = SendMessageW(lp as HWND, CB_GETCURSEL, 0, 0);
                    if (0..=4).contains(&index) {
                        if let Err(error) = data::save_volume(index as u32 * 25) {
                            message(hwnd, &error.to_string());
                        }
                    }
                }
                QUALITY if code == CBN_SELCHANGE as u32 => {
                    let index = SendMessageW(lp as HWND, CB_GETCURSEL, 0, 0);
                    if index >= 0 {
                        if let Err(error) = data::save_quality(index == 1) {
                            message(hwnd, &error.to_string());
                        }
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
                LANGUAGE if code == CBN_SELCHANGE as u32 => {
                    let index = SendMessageW(lp as HWND, CB_GETCURSEL, 0, 0);
                    if (0..3).contains(&index) {
                        if let Err(error) = data::save_language(index as usize) {
                            message(hwnd, &error.to_string());
                        } else {
                            crate::i18n::set(index as usize);
                            update_language(hwnd);
                        }
                    }
                }
                THEME if code == CBN_SELCHANGE as u32 => {
                    let index = SendMessageW(lp as HWND, CB_GETCURSEL, 0, 0);
                    if index >= 0 && (index as usize) < THEMES.len() {
                        let index = index as usize;
                        if let Err(error) = data::save_theme(index) {
                            message(hwnd, &error.to_string());
                        }
                        set_title_theme(hwnd, index);
                        UI.with(|state| {
                            if let Some(ui) = state.borrow_mut().as_mut() {
                                ui.theme = index;
                                DeleteObject(ui.brush);
                                ui.brush = CreateSolidBrush(THEMES[index].canvas);
                                for control in [
                                    ui.list,
                                    ui.apply,
                                    ui.import,
                                    ui.search,
                                    ui.gallery,
                                    ui.settings,
                                ] {
                                    InvalidateRect(control, null(), 1);
                                }
                            }
                        });
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
                _ => {}
            }
            0
        }
        IMPORT_DONE => {
            import_done(hwnd);
            0
        }
        WM_TIMER => {
            let id = data::active();
            let changed = UI.with(|state| {
                let mut state = state.borrow_mut();
                let Some(ui) = state.as_mut() else {
                    return false;
                };
                if let Ok(snapshot) = ui.metrics.try_recv() {
                    ui.snapshot = snapshot;
                    let sidebar = rect(0, monitor_top(client(hwnd).bottom) - 14, 204, 209);
                    InvalidateRect(hwnd, &sidebar, 0);
                }
                if ui.active == id {
                    return false;
                }
                ui.active = id;
                InvalidateRect(ui.list, null(), 1);
                true
            });
            if changed {
                update_controls(hwnd);
            }
            0
        }
        WM_CLOSE => {
            if !UI.with(|state| state.borrow().as_ref().is_some_and(|ui| ui.busy)) {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, event, wp, lp),
    }
}

unsafe fn text(
    dc: HDC,
    value: &str,
    mut area: RECT,
    color: COLORREF,
    font: HFONT,
    flags: DRAW_TEXT_FORMAT,
) {
    let old = SelectObject(dc, font);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(dc, color);
    let value = wide(value);
    DrawTextW(
        dc,
        value.as_ptr(),
        -1,
        &mut area,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS | flags,
    );
    SelectObject(dc, old);
}

unsafe fn rounded(dc: HDC, a: RECT, color: COLORREF, radius: i32) {
    let pen = SelectObject(dc, GetStockObject(NULL_PEN));
    let brush = CreateSolidBrush(color);
    let old = SelectObject(dc, brush);
    RoundRect(dc, a.left, a.top, a.right, a.bottom, radius, radius);
    SelectObject(dc, old);
    SelectObject(dc, pen);
    DeleteObject(brush);
}

unsafe fn paint_ui(dc: HDC, area: RECT, ui: &Library) {
    let p = &THEMES[ui.theme];
    let l = Layout::new(area);
    fill(dc, area, p.canvas);
    fill(dc, rect(0, 0, 204, l.height), p.panel);
    DrawIconEx(dc, 21, 27, ui.mark, 34, 34, 0, null_mut(), DI_NORMAL);
    text(dc, "OpenEngine", rect(64, 22, 130, 44), p.text, ui.title, 0);
    paint_monitor(dc, area.bottom, ui, p);
    if ui.preferences {
        text(
            dc,
            t("Настройки", "Settings", "设置"),
            rect(l.x, 22, 360, 61),
            p.text,
            ui.display,
            0,
        );
        rounded(dc, rect(l.x, 109, l.width - l.x - 28, 149), p.panel, 22);
        text(
            dc,
            t("Цветовая тема", "Theme", "主题"),
            rect(l.x + 24, 120, 280, 36),
            p.text,
            ui.title,
            0,
        );
        text(
            dc,
            t(
                "Цвета окна, карточек и кнопок",
                "Window, tile, and button colors",
                "窗口、卡片与按钮的配色",
            ),
            rect(l.x + 24, 157, 390, 29),
            p.muted,
            ui.body,
            0,
        );
        let half = (l.width - l.x - 45) / 2;
        text(
            dc,
            t("Язык", "Language", "语言"),
            rect(l.x + half + 23, 120, half - 10, 36),
            p.text,
            ui.title,
            0,
        );
        rounded(dc, rect(l.x, 278, l.width - l.x - 28, 183), p.panel, 22);
        text(
            dc,
            t("Качество рендера", "Render quality", "渲染质量"),
            rect(l.x + 24, 288, 320, 36),
            p.text,
            ui.title,
            0,
        );
        text(
            dc,
            t(
                "Детальный режим сохраняет разрешение исходника до 3840 × 2160.",
                "Detail mode preserves source resolution up to 3840 × 2160.",
                "高清模式保留最高 3840 × 2160 的源分辨率。",
            ),
            rect(l.x + 24, 324, l.width - l.x - 67, 32),
            p.muted,
            ui.body,
            0,
        );
        text(
            dc,
            t(
                "Экономичный режим уменьшает нагрузку и размер кеша.",
                "Low-memory mode reduces memory and cache size.",
                "节省内存模式可减少内存与缓存占用。",
            ),
            rect(l.x + 24, 417, l.width - l.x - 67, 26),
            p.muted,
            ui.body,
            0,
        );
        rounded(dc, rect(l.x, 475, l.width - l.x - 28, 104), p.panel, 22);
        text(
            dc,
            t("Звук сцены", "Scene audio", "场景声音"),
            rect(l.x + 24, 482, 320, 34),
            p.text,
            ui.title,
            0,
        );
        return;
    }
    text(
        dc,
        t("Библиотека", "Library", "壁纸库"),
        rect(l.x, 22, 360, 61),
        p.text,
        ui.display,
        0,
    );
    let count = ui.entries.len();
    text(
        dc,
        &format!("{} · {count}", t("Обои в библиотеке", "Wallpapers", "壁纸")),
        rect(l.x, 81, 310, 26),
        p.muted,
        ui.body,
        0,
    );
    text(
        dc,
        t("Поиск", "Search", "搜索"),
        rect(l.x, 114, 62, 32),
        p.muted,
        ui.body,
        0,
    );
    if count == 0 {
        text(
            dc,
            t(
                "По этому запросу ничего не найдено",
                "No matching wallpapers",
                "未找到匹配的壁纸",
            ),
            rect(l.x + 8, 211, l.inspector - l.x - 22, 46),
            p.muted,
            ui.title,
            0,
        );
    }
    rounded(
        dc,
        rect(l.inspector, 102, l.width - l.inspector - 26, l.height - 138),
        p.panel,
        24,
    );
    let preview_width = l.width - l.inspector - 62;
    let hero = rect(l.inspector + 18, 121, preview_width, preview_width * 9 / 16);
    if let Some(item) = ui.entries.get(ui.selected) {
        preview(dc, hero, item, ui.display);
        let still = item.path.as_deref().is_some_and(data::is_still);
        let live = item.path.as_deref().is_some_and(data::is_scene);
        let meta = if live {
            t("ИНТЕРАКТИВНАЯ СЦЕНА", "INTERACTIVE SCENE", "交互场景")
        } else if still {
            t(
                "СТАТИЧНАЯ ТЕКСТУРА ИЗ СЦЕНЫ",
                "STILL IMAGE FROM SCENE",
                "场景静态纹理",
            )
        } else if item.path.is_none() {
            t(
                "ВСТРОЕННАЯ СЦЕНА · 4K",
                "BUILT-IN SCENE · 4K",
                "内置场景 · 4K",
            )
        } else {
            t("ВИДЕО ИЗ БИБЛИОТЕКИ", "LIBRARY VIDEO", "壁纸库视频")
        };
        text(
            dc,
            meta,
            rect(hero.left, hero.bottom + 17, preview_width, 24),
            p.muted,
            ui.body,
            0,
        );
        text(
            dc,
            &item.title,
            rect(hero.left, hero.bottom + 46, preview_width, 48),
            p.text,
            ui.display,
            0,
        );
        let sub = if item.id == ui.active {
            t("Сейчас на рабочем столе", "On your desktop", "当前桌面壁纸")
        } else if live {
            t(
                "Реагирует на клики по рабочему столу",
                "Reacts to clicks on your desktop",
                "响应桌面点击",
            )
        } else if still {
            t(
                "Фон без шейдеров и эффектов Wallpaper Engine",
                "Still image without Wallpaper Engine effects",
                "静态壁纸，不含 Wallpaper Engine 特效",
            )
        } else if item.path.is_some() {
            t(
                "Циклическое воспроизведение без звука",
                "Loops without audio",
                "静音循环播放",
            )
        } else {
            t(
                "Зимний пейзаж с анимацией снега",
                "Winter landscape with falling snow",
                "飘雪冬景",
            )
        };
        text(
            dc,
            sub,
            rect(hero.left, hero.bottom + 97, preview_width, 32),
            p.muted,
            ui.body,
            0,
        );
    }
    text(
        dc,
        &ui.status,
        rect(l.x, l.height - 40, l.inspector - l.x - 22, 28),
        p.muted,
        ui.body,
        0,
    );
}

fn monitor_top(height: i32) -> i32 {
    (height - 223).max(269)
}

unsafe fn paint_monitor(dc: HDC, height: i32, ui: &Library, palette: &Palette) {
    let top = monitor_top(height);
    fill(dc, rect(24, top - 12, 152, 1), palette.card);
    text(
        dc,
        t("Нагрузка OpenEngine", "OpenEngine load", "OpenEngine 负载"),
        rect(24, top, 154, 24),
        palette.muted,
        ui.body,
        0,
    );
    let units = t("МиБ", "MiB", "MiB");
    for (index, (label, value, unit)) in [
        ("RAM", ui.snapshot.ram, units),
        ("VRAM", ui.snapshot.vram, units),
        ("CPU", ui.snapshot.cpu, "%"),
        ("GPU", ui.snapshot.gpu, "%"),
        ("FPS", ui.snapshot.fps, ""),
    ]
    .into_iter()
    .enumerate()
    {
        let y = top + 34 + 26 * index as i32;
        let value = value.map_or_else(
            || "—".into(),
            |v| {
                if unit == units {
                    format!("{v:.1} {unit}")
                } else {
                    format!("{v:.1}{unit}")
                }
            },
        );
        text(dc, label, rect(24, y, 60, 24), palette.muted, ui.body, 0);
        text(
            dc,
            &value,
            rect(83, y, 93, 24),
            palette.text,
            ui.body,
            DT_RIGHT,
        );
    }
}

unsafe fn preview(dc: HDC, area: RECT, item: &Entry, font: HFONT) {
    if item.path.is_none() {
        bitmap(dc, area, WINTER);
    } else if let Some(thumbnail) = data::thumbnail(item) {
        bitmap_size(
            dc,
            area,
            &thumbnail,
            crate::video::THUMB_WIDTH,
            crate::video::THUMB_HEIGHT,
        );
    } else {
        fill(dc, area, rgb(43, 72, 105));
        text(dc, "▶", area, rgb(217, 240, 251), font, DT_CENTER);
    }
}

unsafe fn draw_item(args: &DRAWITEMSTRUCT, ui: &Library) {
    let p = &THEMES[ui.theme];
    let dc = args.hDC;
    let row = args.rcItem;
    if args.CtlID as usize == LIST {
        let selected = args.itemState & ODS_SELECTED as u32 != 0;
        fill(dc, row, p.canvas);
        let Some(item) = ui.entries.get(args.itemID as usize) else {
            return;
        };
        let tile = rect(row.left + 4, row.top + 3, CARD_W - 10, CARD_H - 9);
        rounded(dc, tile, if selected { p.card } else { p.panel }, 16);
        if selected {
            fill(
                dc,
                rect(tile.left, tile.top, tile.right - tile.left, 3),
                p.accent,
            );
        }
        let thumb = rect(tile.left + 7, tile.top + 9, tile.right - tile.left - 14, 99);
        preview(dc, thumb, item, ui.body);
        text(
            dc,
            &item.title,
            rect(
                tile.left + 8,
                tile.top + 108,
                tile.right - tile.left - 15,
                23,
            ),
            p.text,
            ui.body,
            0,
        );
        let detail = if item.id == ui.active {
            t("На рабочем столе", "On your desktop", "当前壁纸")
        } else if item.path.as_deref().is_some_and(data::is_scene) {
            t("Живая сцена", "Live scene", "动态场景")
        } else if item.path.as_deref().is_some_and(data::is_still) {
            t("Статичные обои", "Still wallpaper", "静态壁纸")
        } else if item.path.is_some() {
            t("Видео", "Video", "视频")
        } else {
            t("Сцена", "Scene", "场景")
        };
        text(
            dc,
            detail,
            rect(
                tile.left + 8,
                tile.top + 132,
                tile.right - tile.left - 15,
                21,
            ),
            p.muted,
            ui.body,
            0,
        );
    } else {
        let disabled = args.itemState & ODS_DISABLED as u32 != 0;
        let pressed = args.itemState & ODS_SELECTED as u32 != 0;
        let id = args.CtlID as usize;
        let navigation = id == GALLERY || id == SETTINGS || id == WORKSHOP;
        let filter = (FILTER..FILTER + 3).contains(&id);
        let color = if filter {
            if id - FILTER == ui.filter {
                p.card
            } else {
                p.panel
            }
        } else if navigation {
            if pressed || (id != WORKSHOP && (id == SETTINGS) == ui.preferences) {
                p.card
            } else {
                p.panel
            }
        } else if (id == APPLY || id == IMPORT) && !disabled && !pressed {
            p.accent
        } else {
            p.card
        };
        let inset = rect(
            row.left + 1,
            row.top + 1,
            row.right - row.left - 2,
            row.bottom - row.top - 2,
        );
        fill(
            dc,
            row,
            if navigation || id == APPLY {
                p.panel
            } else {
                p.canvas
            },
        );
        rounded(dc, inset, color, 14);
        let mut label = [0u16; 80];
        let n = GetWindowTextW(args.hwndItem, label.as_mut_ptr(), label.len() as i32);
        text(
            dc,
            &String::from_utf16_lossy(&label[..n.max(0) as usize]),
            row,
            if disabled { p.muted } else { p.text },
            ui.body,
            DT_CENTER,
        );
    }
    if args.itemState & ODS_FOCUS as u32 != 0 {
        let focus = rect(
            row.left + 3,
            row.top + 3,
            row.right - row.left - 6,
            row.bottom - row.top - 6,
        );
        DrawFocusRect(dc, &focus);
    }
}
