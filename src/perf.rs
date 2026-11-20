use crate::win::wide;
use std::{
    collections::HashMap,
    mem::size_of,
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver, TrySendError},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    System::{Performance::*, ProcessStatus::*, Threading::*},
    UI::WindowsAndMessaging::*,
};

pub const WALLPAPER_FPS: u32 = WM_APP + 10;

#[derive(Default)]
pub struct Snapshot {
    pub ram: Option<f64>,
    pub vram: Option<f64>,
    pub cpu: Option<f64>,
    pub gpu: Option<f64>,
    pub fps: Option<f64>,
}

pub fn monitor() -> Receiver<Snapshot> {
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || unsafe {
        let query = GpuQuery::open();
        let mut last: Option<(Instant, u32, u64)> = None;
        let cpus = GetActiveProcessorCount(0xffff).max(1);
        let library = GetCurrentProcessId();
        let mut counter = [0; 1];
        loop {
            let engine = FindWindowW(wide("OpenEngineControl").as_ptr(), null());
            let mut pid = 0;
            if !engine.is_null() {
                GetWindowThreadProcessId(engine, &mut pid);
            }
            let mut pids = vec![library];
            if pid != 0 && pid != library {
                pids.push(pid);
            }
            let now = Instant::now();
            let totals = pids.iter().try_fold((0_u64, 0_u64), |(ram, time), &pid| {
                let (rss, ticks) = process(pid)?;
                Some((ram + rss, time + ticks))
            });
            let (ram, cpu) = if let Some((ram, ticks)) = totals {
                let cpu = last.and_then(|(then, prev_pid, old)| {
                    if prev_pid != pid {
                        return None;
                    }
                    let wall = now.duration_since(then).as_secs_f64();
                    Some(
                        (ticks.saturating_sub(old) as f64 * 0.00001 / wall / f64::from(cpus))
                            .clamp(0.0, 100.0),
                    )
                });
                last = Some((now, pid, ticks));
                (Some(ram as f64 / 1_048_576.0), cpu)
            } else {
                last = None;
                (None, None)
            };
            let (gpu, vram) = query.as_ref().map_or((None, None), |q| q.sample(&pids));
            let fps = if !engine.is_null()
                && SendMessageTimeoutW(
                    engine,
                    WALLPAPER_FPS,
                    0,
                    0,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    180,
                    counter.as_mut_ptr(),
                ) != 0
                && counter[0] != 0
            {
                Some((counter[0] - 1) as f64 / 10.0)
            } else {
                None
            };
            let snapshot = Snapshot {
                ram,
                vram,
                cpu,
                gpu,
                fps,
            };
            if matches!(send.try_send(snapshot), Err(TrySendError::Disconnected(_))) {
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
    receive
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

unsafe fn process(pid: u32) -> Option<(u64, u64)> {
    let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
    if handle.is_null() {
        return None;
    }
    let handle = Process(handle);
    let mut memory = PROCESS_MEMORY_COUNTERS_EX2::default();
    let size = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
    memory.cb = size;
    if K32GetProcessMemoryInfo(
        handle.0,
        (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX2).cast(),
        size,
    ) == 0
    {
        return None;
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) == 0 {
        return None;
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Some((
        memory.PrivateWorkingSetSize as u64,
        ticks(kernel) + ticks(user),
    ))
}

struct GpuQuery {
    query: PDH_HQUERY,
    busy: PDH_HCOUNTER,
    dedicated: PDH_HCOUNTER,
}

impl Drop for GpuQuery {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

impl GpuQuery {
    unsafe fn open() -> Option<Self> {
        let mut query = null_mut();
        if PdhOpenQueryW(null(), 0, &mut query) != 0 {
            return None;
        }
        let mut counters = Self {
            query,
            busy: null_mut(),
            dedicated: null_mut(),
        };
        for (path, slot) in [
            (r"\GPU Engine(*)\Utilization Percentage", &mut counters.busy),
            (
                r"\GPU Process Memory(*)\Dedicated Usage",
                &mut counters.dedicated,
            ),
        ] {
            if PdhAddEnglishCounterW(query, wide(path).as_ptr(), 0, slot) != 0 {
                *slot = null_mut();
            }
        }
        Some(counters)
    }

    unsafe fn sample(&self, pids: &[u32]) -> (Option<f64>, Option<f64>) {
        if PdhCollectQueryData(self.query) != 0 {
            return (None, None);
        }
        let gpu = rows(self.busy, pids).map(|items| {
            let mut engines = HashMap::<String, f64>::new();
            for (engine, value) in items {
                *engines.entry(engine).or_default() += value;
            }
            engines.values().copied().fold(0.0, f64::max).min(100.0)
        });
        let vram = rows(self.dedicated, pids).map(|items| {
            items
                .iter()
                .map(|(_, value)| *value)
                .fold(0.0, |sum, v| sum + v)
                / 1_048_576.0
        });
        (gpu, vram)
    }
}

unsafe fn rows(counter: PDH_HCOUNTER, pids: &[u32]) -> Option<Vec<(String, f64)>> {
    if counter.is_null() {
        return None;
    }
    let mut size = 0u32;
    let mut count = 0u32;
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, null_mut())
        != PDH_MORE_DATA
        || size > 4 * 1024 * 1024
    {
        return None;
    }
    let mut buffer = vec![0_usize; (size as usize).div_ceil(size_of::<usize>())];
    let start = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, start) != 0
        || count as usize > (size as usize / size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
    {
        return None;
    }
    let mut values = Vec::new();
    for entry in std::slice::from_raw_parts(start, count as usize) {
        if entry.szName.is_null()
            || !matches!(
                entry.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            )
        {
            continue;
        }
        let Some(byte_offset) = (entry.szName as usize).checked_sub(buffer.as_ptr() as usize)
        else {
            continue;
        };
        if byte_offset >= size as usize || byte_offset % 2 != 0 {
            continue;
        }
        let name = std::slice::from_raw_parts(entry.szName, (size as usize - byte_offset) / 2);
        let length = name.iter().position(|&n| n == 0).unwrap_or(name.len());
        let name = String::from_utf16_lossy(&name[..length]);
        let Some((pid, engine)) = name
            .strip_prefix("pid_")
            .and_then(|name| name.split_once('_'))
        else {
            continue;
        };
        let value = entry.FmtValue.Anonymous.doubleValue;
        if pid.parse::<u32>().is_ok_and(|pid| pids.contains(&pid))
            && value.is_finite()
            && value >= 0.0
        {
            values.push((engine.split('#').next().unwrap_or(engine).to_owned(), value));
        }
    }
    Some(values)
}
