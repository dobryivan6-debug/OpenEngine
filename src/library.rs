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
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{
        Controls::{Dialogs::*, DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED},
        Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "OpenEngineLibrary";
const LIST: usize = 101;
const IMPORT: usize = 102;
const APPLY: usize = 103;
const THEME: usize = 104;
const IMPORT_DONE: u32 = WM_APP + 3;
const CARD_W: i32 = 180;
const CARD_H: i32 = 156;

struct Palette {
    canvas: COLORREF,
    panel: COLORREF,
    card: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    accent: COLORREF,
}
const THEMES: [Palette; 3] = [
    Palette {
        canvas: rgb(235, 246, 255),
        panel: rgb(255, 255, 255),
        card: rgb(218, 234, 249),
        text: rgb(35, 55, 81),
        muted: rgb(77, 104, 132),
        accent: rgb(245, 199, 224),
    },
    Palette {
        canvas: rgb(255, 241, 233),
        panel: rgb(255, 252, 249),
        card: rgb(251, 228, 216),
        text: rgb(80, 47, 57),
        muted: rgb(125, 84, 88),
        accent: rgb(255, 189, 176),
    },
    Palette {
        canvas: rgb(232, 246, 240),
        panel: rgb(251, 255, 253),
        card: rgb(219, 239, 229),
        text: rgb(37, 63, 58),
        muted: rgb(75, 112, 99),
        accent: rgb(238, 205, 217),
    },
];
const TITLES: [&str; 3] = ["Морозный зефир", "Персиковый плед", "Мятный чай"];

thread_local! { static UI: RefCell<Option<Library>> = const { RefCell::new(None) }; }
static RESULT: Mutex<Option<Result<String, String>>> = Mutex::new(None);

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
    body: HFONT,
    title: HFONT,
    display: HFONT,
    brush: HBRUSH,
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
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
    let width = 1030;
    let height = 690;
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
        loop_messages();
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
        "+  Импорт видео",
        BS_OWNERDRAW as u32,
    );
    let apply = control(
        hwnd,
        APPLY,
        "BUTTON",
        "На рабочий стол",
        BS_OWNERDRAW as u32,
    );
    let combo = control(hwnd, THEME, "COMBOBOX", "", CBS_DROPDOWNLIST as u32);
    let body = make_font(16, 400);
    for control in [list, import, apply, combo] {
        SendMessageW(control, WM_SETFONT, body as usize, 0);
    }
    SendMessageW(list, LB_SETITEMHEIGHT, 0, CARD_H as LPARAM);
    SendMessageW(list, LB_SETCOLUMNWIDTH, CARD_W as usize, 0);
    for name in TITLES {
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
            body,
            title: make_font(21, 600),
            display: make_font(32, 650),
            brush: CreateSolidBrush(THEMES[theme].panel),
        })
    });
    reload(hwnd, None);
    layout(hwnd);
    SetTimer(hwnd, 1, 1000, None);
}

unsafe fn reload(hwnd: HWND, select: Option<&str>) {
    let entries = data::entries();
    let active = data::active();
    let (list, names, index) = UI.with(|window| {
        let mut window = window.borrow_mut();
        let Some(ui) = window.as_mut() else {
            return (null_mut(), vec![], 0);
        };
        ui.entries = entries;
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

unsafe fn layout(hwnd: HWND) {
    let area = client(hwnd);
    let Some((list, import, apply, combo, count)) = UI.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|ui| (ui.list, ui.import, ui.apply, ui.combo, ui.entries.len()))
    }) else {
        return;
    };
    let height = area.bottom;
    let width = area.right;
    let left_width = ((width - 64) * 59 / 100).max(380);
    let right_x = 40 + left_width;
    MoveWindow(
        list,
        34,
        136,
        left_width - 20,
        if count <= 3 {
            CARD_H + 20
        } else {
            (height - 258).max(166)
        },
        1,
    );
    MoveWindow(import, 24 + left_width - 161, 87, 147, 33, 1);
    MoveWindow(
        apply,
        right_x + 16,
        height - 174,
        width - right_x - 54,
        38,
        1,
    );
    MoveWindow(combo, width - 258, height - 80, 207, 175, 1);
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
            (ui.apply, !active && !ui.busy, active)
        })
    }) else {
        return;
    };
    SetWindowTextW(
        apply,
        wide(if active {
            "Установлено"
        } else {
            "На рабочий стол"
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
        .is_some_and(|path| !Video::available(path))
    {
        message(
            hwnd,
            "Для видео нужен FFmpeg в PATH. Установи его и перезапусти OpenEngine.",
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
        ui.status = "Обои установлены.".into();
        InvalidateRect(ui.list, null(), 0);
    });
    update_controls(hwnd);
}

unsafe fn pick_video(hwnd: HWND) {
    if UI.with(|state| state.borrow().as_ref().is_some_and(|ui| ui.busy)) {
        return;
    }
    let filter = wide("Видео MP4, M4V, WMV\0*.mp4;*.m4v;*.wmv\0");
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
    let import = UI.with(|state| {
        let mut state = state.borrow_mut();
        let ui = state.as_mut().unwrap();
        ui.busy = true;
        ui.status = "Копирую видео…".into();
        ui.import
    });
    EnableWindow(import, 0);
    update_controls(hwnd);
    let window = hwnd as usize;
    thread::spawn(move || {
        *RESULT.lock().unwrap() = Some(data::import(&source).map_err(|error| error.to_string()));
        PostMessageW(window as HWND, IMPORT_DONE, 0, 0);
    });
}

unsafe fn import_done(hwnd: HWND) {
    let result = RESULT.lock().unwrap().take();
    let Some(result) = result else { return };
    let import = UI.with(|state| {
        let mut state = state.borrow_mut();
        let ui = state.as_mut().unwrap();
        ui.busy = false;
        ui.status = match &result {
            Ok(_) => "Видео добавлено в библиотеку.".into(),
            Err(error) => error.clone(),
        };
        ui.import
    });
    EnableWindow(import, 1);
    match result {
        Ok(id) => reload(hwnd, Some(&id)),
        Err(error) => message(hwnd, &error),
    }
    update_controls(hwnd);
}

unsafe extern "system" fn procedure(hwnd: HWND, event: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match event {
        WM_GETMINMAXINFO => {
            let info = &mut *(lp as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: 835, y: 570 };
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
        WM_CTLCOLORLISTBOX => UI.with(|state| {
            state.borrow().as_ref().map_or(0, |ui| {
                SetBkColor(wp as HDC, THEMES[ui.theme].panel);
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
                THEME if code == CBN_SELCHANGE as u32 => {
                    let index = SendMessageW(lp as HWND, CB_GETCURSEL, 0, 0);
                    if index >= 0 && (index as usize) < THEMES.len() {
                        let index = index as usize;
                        if let Err(error) = data::save_theme(index) {
                            message(hwnd, &error.to_string());
                        }
                        UI.with(|state| {
                            if let Some(ui) = state.borrow_mut().as_mut() {
                                ui.theme = index;
                                DeleteObject(ui.brush);
                                ui.brush = CreateSolidBrush(THEMES[index].panel);
                                for control in [ui.list, ui.apply, ui.import] {
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

unsafe fn paint_ui(dc: HDC, area: RECT, ui: &Library) {
    let p = &THEMES[ui.theme];
    let width = area.right;
    let height = area.bottom;
    fill(dc, area, p.canvas);
    text(
        dc,
        "OpenEngine",
        rect(24, 10, 290, 52),
        p.text,
        ui.display,
        0,
    );
    let active = ui
        .entries
        .iter()
        .find(|item| item.id == ui.active)
        .map(|item| item.title.as_str())
        .unwrap_or("Зимняя ночь");
    text(
        dc,
        &format!("На рабочем столе  /  {active}"),
        rect(width / 2, 17, width / 2 - 24, 40),
        p.muted,
        ui.body,
        DT_RIGHT,
    );
    let left_width = ((width - 64) * 59 / 100).max(380);
    let x = 40 + left_width;
    let detail_w = width - x - 24;
    fill(dc, rect(24, 74, left_width, height - 188), p.panel);
    text(dc, "Библиотека", rect(37, 82, 190, 43), p.text, ui.title, 0);
    fill(dc, rect(x, 74, detail_w, height - 188), p.panel);
    let hero_width = detail_w - 32;
    let hero_height = (hero_width * 9 / 16).min((height - 320).max(125));
    let hero = rect(x + 16, 90, hero_width, hero_height);
    if let Some(item) = ui.entries.get(ui.selected) {
        preview(dc, hero, item, ui.display);
        text(
            dc,
            &item.title,
            rect(hero.left, hero.bottom + 12, hero_width, 38),
            p.text,
            ui.title,
            0,
        );
        text(
            dc,
            if item.path.is_some() {
                "Локальное видео · зациклено · без звука"
            } else {
                "Встроенная сцена · падающий снег"
            },
            rect(hero.left, hero.bottom + 48, hero_width, 26),
            p.muted,
            ui.body,
            0,
        );
    }
    fill(dc, rect(24, height - 103, width - 48, 67), p.panel);
    text(
        dc,
        "Тема интерфейса",
        rect(40, height - 99, width - 320, 35),
        p.text,
        ui.title,
        0,
    );
    text(
        dc,
        "Цвета окна и библиотеки",
        rect(40, height - 69, width - 320, 26),
        p.muted,
        ui.body,
        0,
    );
    text(
        dc,
        &ui.status,
        rect(24, height - 30, width - 48, 24),
        p.muted,
        ui.body,
        0,
    );
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
        fill(dc, row, p.panel);
        let Some(item) = ui.entries.get(args.itemID as usize) else {
            return;
        };
        let tile = rect(row.left + 4, row.top + 3, CARD_W - 10, CARD_H - 9);
        fill(dc, tile, if selected { p.card } else { p.canvas });
        if selected {
            fill(
                dc,
                rect(tile.left, tile.top, tile.right - tile.left, 3),
                p.accent,
            );
        }
        let thumb = rect(tile.left + 7, tile.top + 9, tile.right - tile.left - 14, 84);
        preview(dc, thumb, item, ui.body);
        text(
            dc,
            &item.title,
            rect(
                tile.left + 8,
                tile.top + 96,
                tile.right - tile.left - 15,
                23,
            ),
            p.text,
            ui.body,
            0,
        );
        let detail = if item.id == ui.active {
            "На рабочем столе"
        } else if item.path.is_some() {
            "Видео"
        } else {
            "Сцена"
        };
        text(
            dc,
            detail,
            rect(
                tile.left + 8,
                tile.top + 118,
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
        let color = if args.CtlID as usize == APPLY && !disabled && !pressed {
            p.accent
        } else {
            p.card
        };
        fill(dc, row, color);
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
