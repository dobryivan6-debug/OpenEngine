#![windows_subsystem = "windows"]

mod archive;
mod audio;
mod data;
mod desktop;
mod i18n;
mod library;
mod particles;
mod perf;
mod scene;
mod scene_gl;
mod video;
mod win;
mod workshop;
mod workshop_ui;

fn main() {
    i18n::set(data::language());
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
