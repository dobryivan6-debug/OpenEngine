use std::sync::atomic::{AtomicUsize, Ordering};

static LANGUAGE: AtomicUsize = AtomicUsize::new(0);

pub fn set(index: usize) {
    LANGUAGE.store(index.min(2), Ordering::Relaxed);
}

pub fn t<'a>(ru: &'a str, en: &'a str, zh: &'a str) -> &'a str {
    [ru, en, zh][LANGUAGE.load(Ordering::Relaxed)]
}
