use crate::{
    data, i18n,
    win::{bitmap_size, client, fill, rect, rgb, wide},
    workshop::{self, Item, Page},
};
use std::{
    cell::RefCell,
    io,
    mem::size_of,
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::EM_SETLIMITTEXT, Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*,
    },
};

const CLASS: &str = "OpenEngine.Workshop";
pub const LIBRARY_CHANGED: u32 = WM_APP + 8;
const QUERY: i32 = 121;
const SEARCH: i32 = 122;
const ITEMS: i32 = 123;
const PREV: i32 = 124;
const NEXT: i32 = 125;
const DOWNLOAD: i32 = 126;
const STATUS: i32 = 127;
const PAGE: i32 = 128;
const OPEN: i32 = 129;

type Preview = (Vec<u8>, usize, usize);
type PreviewJob = (u64, Receiver<Option<Preview>>);

thread_local! { static UI: RefCell<Option<State>> = const { RefCell::new(None) }; }

struct State {
    owner: HWND,
    font: HFONT,
    dark: bool,
    background: HBRUSH,
    items: Vec<Item>,
    page: u32,
    last: u32,
    query: String,
    search: Option<Receiver<io::Result<Page>>>,
    preview_job: Option<PreviewJob>,
    preview: Option<(u64, Preview)>,
    download: Option<(u64, Receiver<io::Result<Option<String>>>)>,
    pending: Vec<u64>,
    next_poll: Instant,
}

impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.background);
        }
    }
}

fn t(ru: &'static str, en: &'static str, zh: &'static str) -> &'static str {
    i18n::t(ru, en, zh)
}

pub unsafe fn open(owner: HWND) {
    let class = wide(CLASS);
    let existing = FindWindowW(class.as_ptr(), null());
    if !existing.is_null() {
        ShowWindow(existing, SW_RESTORE);
        SetForegroundWindow(existing);
        return;
    }
    let instance = GetModuleHandleW(null());
    let registration = WNDCLASSW {
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        lpfnWndProc: Some(window_proc),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: crate::win::icon(),
        ..Default::default()
    };
    RegisterClassW(&registration);
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        wide("Steam Workshop · OpenEngine").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        1010,
        635,
        owner,
        null_mut(),
        instance,
        null(),
    );
    if hwnd.is_null() {
        return;
    }
    let font = CreateFontW(
        -17,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        1,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide("Segoe UI").as_ptr(),
    );
    let dark = data::theme() == 1;
    let mut d = i32::from(dark);
    DwmSetWindowAttribute(
        hwnd,
        20,
        (&mut d as *mut i32).cast(),
        size_of::<i32>() as u32,
    );
    UI.with(|ui| {
        *ui.borrow_mut() = Some(State {
            owner,
            font,
            dark,
            background: CreateSolidBrush(background(dark)),
            items: Vec::new(),
            page: 1,
            last: 1,
            query: String::new(),
            search: None,
            preview_job: None,
            preview: None,
            download: None,
            pending: data::workshop_queue(),
            next_poll: Instant::now(),
        })
    });
    for (id, kind, label, flags) in [
        (
            QUERY,
            "EDIT",
            "",
            WS_BORDER | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
        ),
        (SEARCH, "BUTTON", t("Найти", "Search", "搜索"), WS_TABSTOP),
        (
            ITEMS,
            "LISTBOX",
            "",
            WS_BORDER | WS_VSCROLL | LBS_NOTIFY as u32 | WS_TABSTOP,
        ),
        (PREV, "BUTTON", t("Назад", "Previous", "上一页"), WS_TABSTOP),
        (NEXT, "BUTTON", t("Далее", "Next", "下一页"), WS_TABSTOP),
        (
            DOWNLOAD,
            "BUTTON",
            t("Скачать", "Download", "下载"),
            WS_TABSTOP,
        ),
        (STATUS, "STATIC", "", 0),
        (PAGE, "STATIC", "", 0),
        (
            OPEN,
            "BUTTON",
            t("Открыть в Steam", "Open in Steam", "在 Steam 打开"),
            WS_TABSTOP,
        ),
    ] {
        let child = CreateWindowExW(
            0,
            wide(kind).as_ptr(),
            wide(label).as_ptr(),
            WS_CHILD | WS_VISIBLE | flags,
            0,
            0,
            1,
            1,
            hwnd,
            id as HMENU,
            instance,
            null(),
        );
        SendMessageW(child, WM_SETFONT, font as usize, 0);
    }
    SendDlgItemMessageW(
        hwnd,
        QUERY,
        0x1501,
        0,
        wide(t(
            "Поиск обоев Steam",
            "Search Steam wallpapers",
            "搜索 Steam 壁纸",
        ))
        .as_ptr() as LPARAM,
    );
    SendDlgItemMessageW(hwnd, QUERY, EM_SETLIMITTEXT, 180, 0);
    layout(hwnd);
    SetTimer(hwnd, 1, 220, None);
    ShowWindow(hwnd, SW_SHOW);
    begin_search(hwnd, 1, String::new());
}

fn background(dark: bool) -> COLORREF {
    if dark {
        rgb(28, 33, 43)
    } else {
        rgb(241, 246, 252)
    }
}

unsafe fn layout(hwnd: HWND) {
    let area = client(hwnd);
    let w = area.right;
    let h = area.bottom;
    let pane = if w >= 850 { 360 } else { 292 };
    let list = w - pane - 62;
    let right = list + 44;
    for (id, x, y, width, height) in [
        (QUERY, 20, 20, w - 153, 34),
        (SEARCH, w - 121, 20, 101, 34),
        (ITEMS, 20, 72, list, h - 185),
        (PREV, 20, h - 99, 92, 32),
        (PAGE, 117, h - 94, 122, 28),
        (NEXT, 242, h - 99, 92, 32),
        (DOWNLOAD, right, 110 + pane * 9 / 16, pane, 40),
        (OPEN, right, 160 + pane * 9 / 16, pane, 36),
        (STATUS, 20, h - 56, w - 40, 48),
    ] {
        MoveWindow(GetDlgItem(hwnd, id), x, y, width.max(1), height.max(1), 1);
    }
    InvalidateRect(hwnd, null(), 1);
}

unsafe fn status(hwnd: HWND, message: &str) {
    SetWindowTextW(GetDlgItem(hwnd, STATUS), wide(message).as_ptr());
}

fn work<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = send.send(job());
    });
    receive
}

unsafe fn begin_search(hwnd: HWND, page: u32, query: String) {
    let language = data::language();
    UI.with(|ui| {
        if let Some(ui) = ui.borrow_mut().as_mut() {
            if ui.search.is_some() {
                return;
            }
            ui.search = Some(work({
                let query = query.clone();
                move || workshop::catalog(&query, page, language)
            }));
            ui.query = query;
            ui.page = page;
            ui.items.clear();
            ui.preview = None;
            ui.preview_job = None;
            SendDlgItemMessageW(hwnd, ITEMS, LB_RESETCONTENT, 0, 0);
            status(
                hwnd,
                t(
                    "Загружаем каталог Steam…",
                    "Loading the Steam catalog…",
                    "正在加载 Steam 目录…",
                ),
            );
            buttons(hwnd, ui);
        }
    });
}

fn selected(hwnd: HWND, ui: &State) -> Option<&Item> {
    let index = unsafe { SendDlgItemMessageW(hwnd, ITEMS, LB_GETCURSEL, 0, 0) } as usize;
    ui.items.get(index)
}

unsafe fn buttons(hwnd: HWND, ui: &State) {
    let item = selected(hwnd, ui);
    let search = ui.search.is_none();
    for (id, enabled) in [
        (SEARCH, search),
        (PREV, search && ui.page > 1),
        (NEXT, search && ui.page < ui.last),
        (OPEN, item.is_some()),
        (
            DOWNLOAD,
            item.is_some_and(|item| {
                !matches!(item.kind.as_str(), "Web" | "Application")
                    && ui.download.is_none()
                    && !ui.pending.contains(&item.id)
            }),
        ),
    ] {
        EnableWindow(GetDlgItem(hwnd, id), i32::from(enabled));
    }
    SetWindowTextW(
        GetDlgItem(hwnd, PAGE),
        wide(&format!("{} / {}", ui.page, ui.last)).as_ptr(),
    );
}

unsafe fn update_selection(hwnd: HWND, ui: &mut State) {
    if let Some(item) = selected(hwnd, ui).cloned() {
        let id = item.id;
        if ui.preview_job.is_none() && ui.preview.as_ref().is_none_or(|(shown, _)| *shown != id) {
            ui.preview_job = Some((id, work(move || workshop::preview(&item))));
        }
    }
    buttons(hwnd, ui);
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn tick(hwnd: HWND, ui: &mut State) {
    if let Some(result) = ui.search.as_ref().and_then(|r| r.try_recv().ok()) {
        ui.search = None;
        match result {
            Ok(page) => {
                ui.items = page.items;
                ui.last = page.last;
                for item in &ui.items {
                    let label = if item.kind.is_empty() {
                        item.title.clone()
                    } else {
                        format!("[{}]  {}", item.kind, item.title)
                    };
                    SendDlgItemMessageW(
                        hwnd,
                        ITEMS,
                        LB_ADDSTRING,
                        0,
                        wide(&label).as_ptr() as LPARAM,
                    );
                }
                if !ui.items.is_empty() {
                    SendDlgItemMessageW(hwnd, ITEMS, LB_SETCURSEL, 0, 0);
                    update_selection(hwnd, ui);
                    if ui.pending.is_empty() {
                        status(
                            hwnd,
                            t(
                                "Выбери сцену или видео и нажми «Скачать».",
                                "Choose a scene or video, then select Download.",
                                "选择场景或视频，然后点击下载。",
                            ),
                        );
                    }
                } else {
                    status(hwnd, t("Ничего не найдено.", "No results.", "未找到结果。"));
                }
            }
            Err(error) => {
                ui.last = ui.page;
                status(hwnd, &error.to_string());
            }
        }
        buttons(hwnd, ui);
    }
    if let Some((id, result)) = ui
        .preview_job
        .as_ref()
        .and_then(|(id, r)| r.try_recv().ok().map(|out| (*id, out)))
    {
        ui.preview_job = None;
        ui.preview = result.map(|image| (id, image));
        if selected(hwnd, ui).is_some_and(|item| item.id != id) {
            update_selection(hwnd, ui);
        }
        InvalidateRect(hwnd, null(), 0);
    }
    if let Some((id, result)) = ui
        .download
        .as_ref()
        .and_then(|(id, r)| r.try_recv().ok().map(|output| (*id, output)))
    {
        ui.download = None;
        match result {
            Ok(Some(_)) => {
                ui.pending.retain(|&queued| queued != id);
                let _ = data::save_workshop_queue(&ui.pending);
                PostMessageW(ui.owner, LIBRARY_CHANGED, 0, 0);
                status(
                    hwnd,
                    t(
                        "Обои добавлены в библиотеку OpenEngine.",
                        "Added to the OpenEngine library.",
                        "已添加到 OpenEngine 壁纸库。",
                    ),
                );
            }
            Ok(None) => {
                if !ui.pending.contains(&id) && ui.pending.len() < 64 {
                    ui.pending.push(id);
                }
                let _ = data::save_workshop_queue(&ui.pending);
                status(hwnd, t("Нажми «Подписаться» в Steam. После загрузки обои появятся в OpenEngine.",
                    "Select Subscribe in Steam. OpenEngine imports the wallpaper when Steam finishes.", "请在 Steam 中点击订阅。下载完成后，壁纸将自动导入。"));
                workshop::open_item(hwnd, id);
            }
            Err(error) => {
                ui.pending.retain(|&queued| queued != id);
                let _ = data::save_workshop_queue(&ui.pending);
                status(hwnd, &error.to_string());
            }
        }
        buttons(hwnd, ui);
    }
    if ui.download.is_none() && !ui.pending.is_empty() && Instant::now() >= ui.next_poll {
        ui.next_poll = Instant::now() + Duration::from_secs(3);
        if let Some((id, folder)) = ui
            .pending
            .iter()
            .find_map(|&id| workshop::local_item(id).map(|folder| (id, folder)))
        {
            ui.download = Some((
                id,
                work(move || data::import_workshop(&folder, id).map(Some)),
            ));
            buttons(hwnd, ui);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, event: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match event {
        WM_GETMINMAXINFO => {
            (*(lp as *mut MINMAXINFO)).ptMinTrackSize = POINT { x: 730, y: 475 };
            0
        }
        WM_SIZE => {
            layout(hwnd);
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            UI.with(|ui| {
                ui.borrow_mut().take();
            });
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLORSTATIC => UI.with(|ui| {
            ui.try_borrow()
                .ok()
                .as_deref()
                .and_then(Option::as_ref)
                .map_or(0, |ui| {
                    SetTextColor(
                        wp as HDC,
                        if ui.dark {
                            rgb(234, 239, 248)
                        } else {
                            rgb(32, 48, 72)
                        },
                    );
                    SetBkColor(wp as HDC, background(ui.dark));
                    ui.background as LRESULT
                })
        }),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            UI.with(|ui| {
                if let Some(ui) = ui.try_borrow().ok().as_deref().and_then(Option::as_ref) {
                    let area = client(hwnd);
                    fill(dc, area, background(ui.dark));
                    let pane = if area.right >= 850 { 360 } else { 292 };
                    let tile = rect(area.right - pane - 18, 72, pane, pane * 9 / 16);
                    fill(
                        dc,
                        tile,
                        if ui.dark {
                            rgb(40, 49, 64)
                        } else {
                            rgb(222, 234, 249)
                        },
                    );
                    if let Some((id, (pixels, w, h))) = &ui.preview {
                        if selected(hwnd, ui).is_some_and(|item| item.id == *id) {
                            bitmap_size(dc, tile, pixels, *w, *h);
                        }
                    }
                }
            });
            EndPaint(hwnd, &paint);
            0
        }
        WM_TIMER => {
            UI.with(|ui| {
                if let Some(ui) = ui.borrow_mut().as_mut() {
                    tick(hwnd, ui);
                }
            });
            0
        }
        WM_COMMAND => {
            let id = (wp & 0xffff) as i32;
            let notification = ((wp >> 16) & 0xffff) as u32;
            if id == SEARCH {
                let mut query = [0u16; 181];
                let n =
                    GetDlgItemTextW(hwnd, QUERY, query.as_mut_ptr(), query.len() as i32) as usize;
                begin_search(hwnd, 1, String::from_utf16_lossy(&query[..n]));
            } else if matches!(id, PREV | NEXT) {
                let nav = UI.with(|ui| {
                    ui.borrow().as_ref().map(|ui| {
                        (
                            if id == PREV {
                                ui.page.saturating_sub(1).max(1)
                            } else {
                                (ui.page + 1).min(ui.last)
                            },
                            ui.query.clone(),
                        )
                    })
                });
                if let Some((page, query)) = nav {
                    begin_search(hwnd, page, query);
                }
            } else {
                UI.with(|ui| {
                    let mut ui = ui.borrow_mut();
                    let Some(ui) = ui.as_mut() else {
                        return;
                    };
                    match id {
                        ITEMS if notification == LBN_SELCHANGE as u32 => update_selection(hwnd, ui),
                        DOWNLOAD => {
                            if let Some(item) = selected(hwnd, ui) {
                                let item = item.id;
                                if ui.download.is_none() && !ui.pending.contains(&item) {
                                    ui.download =
                                        Some((item, work(move || workshop::import_item(item))));
                                    status(
                                        hwnd,
                                        t(
                                            "Проверяем файлы в Steam…",
                                            "Checking Steam files…",
                                            "正在检查 Steam 文件…",
                                        ),
                                    );
                                    buttons(hwnd, ui);
                                }
                            }
                        }
                        OPEN => {
                            if let Some(item) = selected(hwnd, ui) {
                                workshop::open_item(hwnd, item.id);
                            }
                        }
                        _ => {}
                    }
                });
            }
            0
        }
        _ => DefWindowProcW(hwnd, event, wp, lp),
    }
}
