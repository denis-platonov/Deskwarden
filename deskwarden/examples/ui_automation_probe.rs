use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

fn main() {
    let hwnd = unsafe { GetForegroundWindow() };
    println!(
        "Probing foreground window {:?} for two Edit controls...",
        hwnd.0
    );

    match deskwarden::injector::ui_automation::fill_via_ui_automation(
        hwnd.0 as isize,
        "probe-username",
        "probe-password",
    ) {
        Ok(true) => println!("Found and filled two edit controls."),
        Ok(false) => println!("Did not find two edit controls (fallback would trigger)."),
        Err(e) => println!("COM error: {e:?}"),
    }
}
