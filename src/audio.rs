use crate::{scene::Track, win::wide};
use std::{fs, io, os::windows::process::CommandExt, path::Path, process::Command, ptr::null_mut};

#[link(name = "winmm")]
extern "system" {
    fn mciSendStringW(
        command: *const u16,
        result: *mut u16,
        size: u32,
        callback: *mut std::ffi::c_void,
    ) -> u32;
}

fn mci(command: &str, result: &mut [u16]) -> bool {
    unsafe {
        mciSendStringW(
            wide(command).as_ptr(),
            result.as_mut_ptr(),
            result.len() as u32,
            null_mut(),
        ) == 0
    }
}

fn send(command: &str) -> bool {
    mci(command, &mut [])
}

pub fn stage(tracks: &[Track<'_>], dir: &Path) -> io::Result<()> {
    let mut manifest = String::new();
    for (i, track) in tracks.iter().enumerate() {
        let name = format!("audio-{i}.{}", track.extension);
        let original = dir.join(&name);
        fs::write(&original, track.file)?;
        let supported = if matches!(track.extension.as_str(), "wav" | "mp3") {
            Some(name)
        } else {
            let name = format!("audio-{i}.mp3");
            let result = crate::video::find_ffmpeg().and_then(|exe| {
                Command::new(exe)
                    .args([
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-nostdin",
                        "-protocol_whitelist",
                        "file",
                        "-f",
                        &track.extension,
                        "-i",
                    ])
                    .arg(&original)
                    .args(["-vn", "-t", "1800", "-codec:a", "libmp3lame", "-q:a", "6"])
                    .arg(dir.join(&name))
                    .creation_flags(0x08000000)
                    .output()
                    .ok()
            });
            fs::remove_file(original)?;
            result
                .filter(|result| result.status.success())
                .map(|_| name)
        };
        if let Some(name) = supported {
            use std::fmt::Write;
            let _ = writeln!(
                manifest,
                "{i} {} {} {}",
                track.volume,
                u8::from(track.looping),
                name
            );
        }
    }
    if !manifest.is_empty() {
        fs::write(dir.join("audio.txt"), manifest)?;
    }
    Ok(())
}

#[derive(Default)]
pub struct Audio {
    tracks: Vec<Playing>,
    volume: u32,
    playing: bool,
}

struct Playing {
    alias: String,
    volume: u32,
    looping: bool,
    started: bool,
}

impl Audio {
    pub fn open(scene: &Path) -> Self {
        let manifest = fs::read_to_string(scene.with_file_name("audio.txt")).unwrap_or_default();
        let mut audio = Self::default();
        for line in manifest.lines().take(8) {
            let mut tokens = line.split_whitespace();
            let Some((i, volume, looping, name)) = (|| {
                Some((
                    tokens.next()?.parse::<usize>().ok()?,
                    tokens.next()?.parse::<u32>().ok()?,
                    tokens.next()? == "1",
                    tokens.next()?,
                ))
            })() else {
                continue;
            };
            if i > 7
                || ![format!("audio-{i}.wav"), format!("audio-{i}.mp3")]
                    .iter()
                    .any(|n| n == name)
            {
                continue;
            }
            let file = scene.with_file_name(name);
            let path = file.to_string_lossy();
            if path.contains('"') {
                continue;
            }
            let alias = format!("openengine_audio_{i}");
            if send(&format!("open \"{path}\" type mpegvideo alias {alias}")) {
                audio.tracks.push(Playing {
                    alias,
                    volume: volume.min(100),
                    looping,
                    started: false,
                });
            }
        }
        audio
    }

    pub fn update(&mut self, play: bool, volume: u32) {
        let play = play && volume > 0;
        let volume_changed = volume != self.volume;
        let play_changed = play != self.playing;
        self.volume = volume;
        self.playing = play;
        for track in &mut self.tracks {
            let alias = &track.alias;
            if volume_changed || (play_changed && play) {
                let value = volume * track.volume / 10;
                if !send(&format!("setaudio {alias} volume to {value}")) && play {
                    send(&format!("pause {alias}"));
                    continue;
                }
            }
            if !play {
                if play_changed {
                    send(&format!("pause {alias}"));
                }
                continue;
            }
            let mut mode = [0u16; 32];
            let ok = mci(&format!("status {alias} mode"), &mut mode);
            if !ok {
                continue;
            }
            let mode = String::from_utf16_lossy(
                &mode[..mode.iter().position(|&c| c == 0).unwrap_or(mode.len())],
            );
            if play_changed && mode == "paused" {
                if !send(&format!("resume {alias}")) {
                    send(&format!("play {alias}"));
                }
            } else if mode == "stopped" && (!track.started || track.looping) {
                let repeat = if track.looping { " repeat" } else { "" };
                track.started = send(&format!("play {alias} from 0{repeat}"));
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        for track in &self.tracks {
            send(&format!("close {}", track.alias));
        }
    }
}
