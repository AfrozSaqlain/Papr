use arboard::Clipboard;
use std::sync::Mutex;
use std::sync::OnceLock;

static CLIP: OnceLock<Mutex<Clipboard>> = OnceLock::new();

fn main() {
    let _ = CLIP.get_or_init(|| Mutex::new(Clipboard::new().unwrap()));
}
