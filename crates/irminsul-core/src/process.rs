use anyhow::Result;

#[cfg(windows)]
pub fn game_process() -> Result<Option<String>> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::*;
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Ok(None);
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = Vec::new();
        let mut next = Process32FirstW(snapshot, &mut entry);
        while next.is_ok() {
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if name.eq_ignore_ascii_case("GenshinImpact.exe")
                || name.eq_ignore_ascii_case("YuanShen.exe")
            {
                found.push(entry.th32ProcessID);
            }
            next = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
        anyhow::ensure!(
            found.len() <= 1,
            "Multiple game processes detected. Close the extra game before capturing."
        );
        let Some(pid) = found.first() else {
            return Ok(None);
        };
        // The process can exit after the Toolhelp snapshot was taken. A missing
        // identity resets the seed hint; it must not terminate capture.
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, *pid) else {
            return Ok(None);
        };
        let (mut creation, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let result = GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        if result.is_err() {
            return Ok(None);
        }
        let time = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        Ok(Some(format!("{pid}:{time}")))
    }
}

#[cfg(not(windows))]
pub fn game_process() -> Result<Option<String>> {
    anyhow::bail!("The shared multi-account capture helper currently supports Windows.")
}

#[cfg(windows)]
pub fn launch_helper(port: u16, token: &str, mode: crate::CaptureMode) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{PCWSTR, w};
    let executable: Vec<u16> = std::env::current_exe()?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let arguments: Vec<u16> = format!(
        "--irminsul-capture-helper {port} {token} {}",
        mode.argument()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    unsafe {
        let mut options = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NO_CONSOLE | SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(executable.as_ptr()),
            lpParameters: PCWSTR(arguments.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = ShellExecuteExW(&mut options);
        CoUninitialize();
        result.map_err(|_| {
            anyhow::anyhow!("Capture permission was declined or the helper could not start.")
        })?;
        if !options.hProcess.is_invalid() {
            let _ = CloseHandle(options.hProcess);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn launch_helper(_: u16, _: &str, _: crate::CaptureMode) -> Result<()> {
    anyhow::bail!("Capture is currently available on Windows.")
}
