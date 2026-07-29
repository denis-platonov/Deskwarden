fn main() {
    println!("Watching foreground window changes. Alt-Tab between apps to see events; Ctrl+C to quit.");
    deskwarden::window_watch::watch_foreground_windows(|event| {
        println!("foreground: pid={} exe={} hwnd={}", event.pid, event.exe_name, event.hwnd);
    })
    .expect("failed to start window watcher");
}
