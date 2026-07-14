use arboard::Clipboard;
fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
fn main() {
    assert_send::<Clipboard>();
    assert_sync::<Clipboard>();
}
