#![windows_subsystem = "windows"]

mod data;
mod desktop;
mod library;
mod video;
mod win;

fn main() {
    let argument = std::env::args().nth(1);
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
        if argument.as_deref() == Some("--library-ui") {
            library::run();
        } else {
            desktop::run(argument.as_deref() != Some("--background"));
        }
    }
}
