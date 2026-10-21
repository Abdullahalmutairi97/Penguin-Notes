// The Windows .exe gets the penguin icon (Explorer, taskbar, Start menu).
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("windows resources");
    }
}
