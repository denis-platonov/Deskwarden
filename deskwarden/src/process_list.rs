use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};

pub struct ProcessInfo {
    pub pid: u32,
    pub exe_name: String,
}

pub fn list_processes() -> windows::core::Result<Vec<ProcessInfo>> {
    let mut result = Vec::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );
                result.push(ProcessInfo {
                    pid: entry.th32ProcessID,
                    exe_name: name,
                });

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_the_current_process() {
        let processes = list_processes().unwrap();
        let current_pid = std::process::id();
        assert!(
            processes.iter().any(|p| p.pid == current_pid),
            "expected current pid {current_pid} in process list of {} entries",
            processes.len()
        );
    }

    #[test]
    fn returns_a_nonempty_list() {
        let processes = list_processes().unwrap();
        assert!(!processes.is_empty());
    }
}
