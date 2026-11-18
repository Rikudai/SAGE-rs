//! Tira a borda arredondada e a sombra que o Windows 11 (DWM) desenha em volta da
//! janela transparente do slime. A janela é achada entre as do próprio processo pelo
//! título, e os atributos são reaplicados de tempos em tempos (o winit os refaz).
//!
//! Só olha janelas da thread que chama (a thread da interface, dona das janelas do
//! winit): `GetWindowTextW` numa janela de outra thread manda `WM_GETTEXT` e trava se
//! aquela thread não processa mensagens (as janelas ocultas do driver/wgpu, por exemplo).

#[cfg(windows)]
pub fn strip(title: &str) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{CallWindowProcW, GWLP_WNDPROC, WM_NCCALCSIZE, WNDPROC};
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_DONOTROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW,
        SetWindowPos, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE,
        WS_EX_TOOLWINDOW, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };
    use windows::core::BOOL;

    use windows::Win32::System::Threading::GetCurrentThreadId;

    struct Search {
        tid: u32,
        title: Vec<u16>,
        found: Vec<HWND>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam carrega o ponteiro para `Search` válido durante EnumWindows.
        let s = unsafe { &mut *(lparam.0 as *mut Search) };
        let tid = unsafe { GetWindowThreadProcessId(hwnd, None) };
        if tid == s.tid {
            let mut buf = [0u16; 128];
            let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
            if buf[..n] == s.title[..] {
                s.found.push(hwnd);
            }
        }
        BOOL(1)
    }

    let mut search = Search { tid: unsafe { GetCurrentThreadId() }, title: title.encode_utf16().collect(), found: vec![] };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    // O winit, com a "sombra de janela sem moldura" que o egui-winit sempre pede no
    // Windows, responde WM_NCCALCSIZE com `top += 1`: sobra 1 px de moldura branca no
    // topo. Esta subclasse (via GWLP_WNDPROC, só user32 — as funções de subclasse do
    // comctl32 não são exportadas por nome sem manifesto v6) responde antes dele:
    // área cliente = janela inteira. Os procedimentos originais ficam por HWND.
    static ORIGINAL: std::sync::Mutex<Vec<(isize, isize)>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "system" fn no_frame(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NCCALCSIZE && wparam.0 != 0 {
            return LRESULT(0);
        }
        let orig = ORIGINAL.lock().ok().and_then(|v| v.iter().find(|(h, _)| *h == hwnd.0 as isize).map(|(_, p)| *p));
        match orig {
            // SAFETY: `p` é o WNDPROC original desta janela, guardado ao instalar a subclasse.
            Some(p) => unsafe {
                let proc: WNDPROC = std::mem::transmute::<isize, WNDPROC>(p);
                CallWindowProcW(proc, hwnd, msg, wparam, lparam)
            },
            None => unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
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

            // O winit deixa WS_CAPTION (e esconde a barra via WM_NCCALCSIZE); o Windows 11
            // ainda desenha 1 px de moldura no topo. Vira um popup puro, sem moldura nem
            // botão na barra de tarefas/Alt+Tab. Só mexe se mudou (SWP_FRAMECHANGED custa).
            let style = GetWindowLongPtrW(*hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(*hwnd, GWL_EXSTYLE) as u32;
            let frame = (WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0;
            let edges = (WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE | WS_EX_DLGMODALFRAME | WS_EX_APPWINDOW).0;
            let new_style = (style & !frame) | WS_POPUP.0;
            let new_ex = (ex & !edges) | WS_EX_TOOLWINDOW.0;
            let ours = no_frame as *const () as isize;
            let installed = GetWindowLongPtrW(*hwnd, GWLP_WNDPROC) == ours;
            if !installed && let Ok(mut v) = ORIGINAL.lock() {
                // Janela nova (HWND reaproveitado ou não): troca a entrada antiga.
                v.retain(|(h, _)| *h != hwnd.0 as isize);
                let prev = SetWindowLongPtrW(*hwnd, GWLP_WNDPROC, ours);
                if prev != 0 {
                    v.push((hwnd.0 as isize, prev));
                }
            }
            if new_style != style || new_ex != ex || !installed {
                SetWindowLongPtrW(*hwnd, GWL_STYLE, new_style as isize);
                SetWindowLongPtrW(*hwnd, GWL_EXSTYLE, new_ex as isize);
                let _ = SetWindowPos(
                    *hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }
    !search.found.is_empty()
}

#[cfg(not(windows))]
pub fn strip(_title: &str) -> bool {
    true
}
