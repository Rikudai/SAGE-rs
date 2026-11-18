//! Tira a borda arredondada e a sombra que o Windows 11 (DWM) desenha em volta da
//! janela transparente do slime. A janela é achada entre as do próprio processo pelo
//! título, e os atributos são reaplicados de tempos em tempos (o winit os refaz).

#[cfg(windows)]
pub fn strip(title: &str) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_DONOTROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId};
    use windows::core::BOOL;

    struct Search {
        pid: u32,
        title: Vec<u16>,
        found: Vec<HWND>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam carrega o ponteiro para `Search` válido durante EnumWindows.
        let s = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == s.pid {
            let mut buf = [0u16; 128];
            let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
            if buf[..n] == s.title[..] {
                s.found.push(hwnd);
            }
        }
        BOOL(1)
    }

    let mut search = Search { pid: std::process::id(), title: title.encode_utf16().collect(), found: vec![] };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    // DWMWA_COLOR_NONE: sem borda colorida.
    const COLOR_NONE: u32 = 0xFFFF_FFFE;
    for hwnd in &search.found {
        unsafe {
            let none = COLOR_NONE;
            let corner = DWMWCP_DONOTROUND.0;
            let policy = DWMNCRP_DISABLED.0;
            let size = std::mem::size_of::<u32>() as u32;
            let _ = DwmSetWindowAttribute(*hwnd, DWMWA_BORDER_COLOR, (&none as *const u32).cast(), size);
            let _ = DwmSetWindowAttribute(*hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, (&corner as *const i32).cast(), size);
            let _ = DwmSetWindowAttribute(*hwnd, DWMWA_NCRENDERING_POLICY, (&policy as *const i32).cast(), size);
        }
    }
    !search.found.is_empty()
}

#[cfg(not(windows))]
pub fn strip(_title: &str) -> bool {
    true
}
