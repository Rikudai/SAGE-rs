//! Chamadas Win32 (bloqueantes). Todas devolvem `Result<_, String>` com mensagem em português.

use windows::core::BOOL;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetSystemMetrics, GetWindowRect, GetWindowTextW, IsIconic, IsWindowVisible, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE, SetCursorPos, SetForegroundWindow,
    ShowWindow,
};

const CF_UNICODETEXT: u32 = 13;

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` é um slice válido de INPUT e o tamanho é o da struct.
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        Err("O Windows bloqueou a entrada simulada (janela elevada ou sessão bloqueada?).".into())
    }
}

fn mouse_input(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn key_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

pub fn move_mouse(x: i32, y: i32) -> Result<(), String> {
    // SAFETY: chamada simples sem ponteiros.
    unsafe { SetCursorPos(x, y) }.map_err(|e| format!("Falha ao mover o mouse: {e}"))
}

pub fn click(x: i32, y: i32, button: &str, clicks: u32) -> Result<(), String> {
    let (down, up) = match button {
        "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        other => return Err(format!("Botão desconhecido: '{other}' (use left, right ou middle).")),
    };
    move_mouse(x, y)?;
    for _ in 0..clicks.clamp(1, 5) {
        send(&[mouse_input(down, 0), mouse_input(up, 0)])?;
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    Ok(())
}

pub fn scroll(amount: i32) -> Result<(), String> {
    send(&[mouse_input(MOUSEEVENTF_WHEEL, amount)])
}

pub fn type_text(text: &str) -> Result<(), String> {
    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        inputs.push(key_input(0, unit, KEYEVENTF_UNICODE));
        inputs.push(key_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    for chunk in inputs.chunks(200) {
        send(chunk)?;
    }
    Ok(())
}

fn is_extended(vk: u16) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0xAD..=0xB3)
}

pub fn press_vks(vks: &[u16]) -> Result<(), String> {
    let flag = |vk: u16| if is_extended(vk) { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
    let mut inputs: Vec<INPUT> = vks.iter().map(|&vk| key_input(vk, 0, flag(vk))).collect();
    inputs.extend(vks.iter().rev().map(|&vk| key_input(vk, 0, flag(vk) | KEYEVENTF_KEYUP)));
    send(&inputs)
}

pub fn lock_pc() -> Result<(), String> {
    // SAFETY: sem argumentos.
    unsafe { LockWorkStation() }.map_err(|e| format!("Falha ao bloquear o PC: {e}"))
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam aponta para o Vec vivo em `list_windows`.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
    // SAFETY: hwnd fornecido pelo EnumWindows.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        out.push(WindowInfo {
            hwnd: hwnd.0 as isize,
            title,
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
        });
    }
    BOOL(1)
}

pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    let mut out: Vec<WindowInfo> = Vec::new();
    // SAFETY: o ponteiro vale durante a chamada síncrona.
    unsafe { EnumWindows(Some(enum_cb), LPARAM(&mut out as *mut _ as isize)) }
        .map_err(|e| format!("Falha ao listar janelas: {e}"))?;
    Ok(out)
}

pub fn focus_window(title: &str) -> Result<String, String> {
    let needle = title.to_lowercase();
    let w = list_windows()?
        .into_iter()
        .find(|w| w.title.to_lowercase().contains(&needle))
        .ok_or_else(|| format!("Nenhuma janela com '{title}' no título."))?;
    let hwnd = HWND(w.hwnd as *mut _);
    // SAFETY: hwnd veio do EnumWindows.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            return Err("O Windows não permitiu trazer a janela para frente.".into());
        }
    }
    Ok(w.title)
}

/// Captura a tela virtual inteira e devolve (largura, altura, RGBA).
pub fn capture_screen() -> Result<(u32, u32, Vec<u8>), String> {
    // SAFETY: sequência GDI clássica; todos os handles são liberados ao final.
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w <= 0 || h <= 0 {
            return Err("Não consegui medir a tela.".into());
        }
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let blt = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY);

        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // de cima para baixo
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr().cast()), &mut bi, DIB_RGB_COLORS);

        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        blt.map_err(|e| format!("Falha na captura (BitBlt): {e}"))?;
        if lines == 0 {
            return Err("Falha ao ler os pixels da captura.".into());
        }
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2); // BGRA -> RGBA
            px[3] = 255;
        }
        Ok((w as u32, h as u32, buf))
    }
}

pub fn clipboard_get() -> Result<String, String> {
    // SAFETY: Open/Close pareados; o bloco global é travado só durante a leitura.
    unsafe {
        OpenClipboard(None).map_err(|e| format!("Não consegui abrir a área de transferência: {e}"))?;
        let result = (|| {
            let h = GetClipboardData(CF_UNICODETEXT).map_err(|_| "A área de transferência não tem texto.".to_string())?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return Err("Não consegui ler a área de transferência.".to_string());
            }
            let mut len = 0usize;
            while *p.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let _ = GlobalUnlock(g);
            Ok(s)
        })();
        let _ = CloseClipboard();
        result
    }
}

pub fn clipboard_set(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    // SAFETY: bloco alocado com o tamanho exato; após SetClipboardData o Windows é o dono.
    unsafe {
        OpenClipboard(None).map_err(|e| format!("Não consegui abrir a área de transferência: {e}"))?;
        let result = (|| {
            EmptyClipboard().map_err(|e| format!("Falha ao limpar a área de transferência: {e}"))?;
            let g = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| format!("Sem memória: {e}"))?;
            let p = GlobalLock(g) as *mut u16;
            if p.is_null() {
                return Err("Não consegui travar a memória.".to_string());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(g);
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(g.0)))
                .map(|_| ())
                .map_err(|e| format!("Falha ao gravar na área de transferência: {e}"))
        })();
        let _ = CloseClipboard();
        result
    }
}
