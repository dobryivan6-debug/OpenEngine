use std::{ffi::OsStr, mem::zeroed, os::windows::ffi::OsStrExt, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, RECT},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};

pub const WIDTH: usize = 1600;
pub const HEIGHT: usize = 900;
pub const FRAME_LEN: usize = WIDTH * HEIGHT * 3;
pub const WINTER: &[u8; FRAME_LEN] = include_bytes!("../assets/winter.bgr");

pub fn wide(text: &str) -> Vec<u16> {
    wide_os(OsStr::new(text))
}
pub fn wide_os(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}
pub const fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    r | g << 8 | b << 16
}

pub unsafe fn register(name: &str, proc: WNDPROC) {
    let name = wide(name);
    let class = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: GetModuleHandleW(std::ptr::null()),
        lpszClassName: name.as_ptr(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: icon(),
        ..zeroed()
    };
    RegisterClassW(&class);
}

pub unsafe fn icon() -> HICON {
    let ico = include_bytes!("../assets/OpenEngine.ico");
    let offset = u32::from_le_bytes(ico[18..22].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(ico[14..18].try_into().unwrap()) as usize;
    let icon = CreateIconFromResourceEx(
        ico[offset..offset + size].as_ptr(),
        size as u32,
        1,
        0x30000,
        24,
        24,
        LR_DEFAULTCOLOR,
    );
    if icon.is_null() {
        CopyIcon(LoadIconW(null_mut(), IDI_APPLICATION))
    } else {
        icon
    }
}

pub unsafe fn loop_messages() {
    let mut message: MSG = zeroed();
    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

pub unsafe fn client(hwnd: HWND) -> RECT {
    let mut rect = zeroed();
    GetClientRect(hwnd, &mut rect);
    rect
}

pub fn rect(x: i32, y: i32, width: i32, height: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

pub unsafe fn fill(dc: HDC, area: RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(dc, &area, brush);
    DeleteObject(brush);
}

pub unsafe fn bitmap(dc: HDC, area: RECT, pixels: &[u8]) {
    bitmap_size(dc, area, pixels, WIDTH, HEIGHT);
}

pub unsafe fn bitmap_size(
    dc: HDC,
    area: RECT,
    pixels: &[u8],
    pixel_width: usize,
    pixel_height: usize,
) {
    let mut info: BITMAPINFO = zeroed();
    let w = area.right - area.left;
    let h = area.bottom - area.top;
    if w <= 0 || h <= 0 {
        return;
    }
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = pixel_width as i32;
    info.bmiHeader.biHeight = -(pixel_height as i32);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 24;
    info.bmiHeader.biCompression = BI_RGB;
    let aspect = w as f32 / h as f32;
    let src_h = (pixel_width as f32 / aspect).min(pixel_height as f32) as i32;
    let src_w = (pixel_height as f32 * aspect).min(pixel_width as f32) as i32;
    SetStretchBltMode(dc, HALFTONE);
    SetBrushOrgEx(dc, 0, 0, null_mut());
    StretchDIBits(
        dc,
        area.left,
        area.top,
        w,
        h,
        (pixel_width as i32 - src_w) / 2,
        (pixel_height as i32 - src_h) / 2,
        src_w,
        src_h,
        pixels.as_ptr().cast(),
        &info,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}

pub unsafe fn message(hwnd: HWND, text: &str) {
    MessageBoxW(
        hwnd,
        wide(text).as_ptr(),
        wide("OpenEngine").as_ptr(),
        MB_OK | MB_ICONWARNING,
    );
}
