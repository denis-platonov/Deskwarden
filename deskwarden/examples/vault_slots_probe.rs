//! Live proof of `vault_service`'s attachment slots across REAL processes.
//!
//! The unit tests drive one process against a fake kernel. The two facts the
//! design actually rests on cannot be established that way:
//!
//! 1. Two processes claim DIFFERENT slots -- the fixed slot space really is
//!    shared, which is the whole reason the in-process register was dropped.
//! 2. A killed process releases its slot -- the OS does it, with no `Drop`
//!    running anywhere, which is the case the design exists for.
//!
//! Deliberately NOT the Deskwarden binary: launching that would trip
//! `single_instance`'s takeover and kill a running app.
//!
//! ```text
//! cargo run --example vault_slots_probe -- hold 30    # claim a slot for 30s
//! cargo run --example vault_slots_probe -- look       # report who is attached
//! ```

use deskwarden::vault_service::{anyone_attached, attach, attach_slot_name, windows_env, SLOTS};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("look");
    let env = windows_env();

    let occupied: Vec<usize> =
        (0..SLOTS).filter(|slot| (env.is_held)(&attach_slot_name(*slot))).collect();
    println!("pid {} | occupied slots before: {occupied:?}", std::process::id());
    println!("anyone_attached: {}", anyone_attached(&env));

    if mode == "diagnose" {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::GetLastError;
        use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS};
        let name = attach_slot_name(0);
        for (label, rights) in [
            ("SYNCHRONIZE", 0x0010u32),
            ("MUTEX_ALL_ACCESS", 0x001F_0001),
            ("READ_CONTROL", 0x0002_0000),
        ] {
            unsafe {
                let r = OpenMutexW(SYNCHRONIZATION_ACCESS_RIGHTS(rights), false, &HSTRING::from(&name));
                match r {
                    Ok(_) => println!("{label}: opened"),
                    Err(e) => println!("{label}: {e:?} last_error={:?}", GetLastError()),
                }
            }
        }
        println!("name asked for: {name}");
        return;
    }

    if mode == "hold" {
        let seconds: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
        match attach(&env) {
            Some(held) => {
                println!("claimed slot {}", held.slot());
                println!("holding for {seconds}s -- kill this process to test the crash case");
                std::thread::sleep(std::time::Duration::from_secs(seconds));
                println!("releasing slot {}", held.slot());
            }
            None => println!("no free slot out of {SLOTS}"),
        }
    }
}
