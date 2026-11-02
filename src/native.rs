use crate::config::Binding;
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeSet, HashSet},
    ffi::c_void,
    mem::{size_of, zeroed},
    path::PathBuf,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Dwm::*,
        Storage::FileSystem::*,
        System::{Diagnostics::Debug::ReadProcessMemory, Memory::*, Threading::*},
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::{BOOL, PWSTR},
};

pub struct Handle(pub HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub struct Process {
    pub handle: Handle,
    pub pid: u32,
}
fn epoch(ft: FILETIME) -> f64 {
    (((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64) as f64 / 10_000_000.
        - 11_644_473_600.
}
impl Process {
    pub fn open(pid: u32) -> Result<Self> {
        unsafe {
            Ok(Self {
                pid,
                handle: Handle(OpenProcess(
                    PROCESS_VM_READ | PROCESS_QUERY_INFORMATION | PROCESS_DUP_HANDLE,
                    false,
                    pid,
                )?),
            })
        }
    }
    pub fn creation(&self) -> Result<f64> {
        unsafe {
            let (mut c, mut e, mut k, mut u) = (zeroed(), zeroed(), zeroed(), zeroed());
            GetProcessTimes(self.handle.0, &mut c, &mut e, &mut k, &mut u)?;
            Ok(epoch(c))
        }
    }
    pub fn image(&self) -> Result<String> {
        unsafe {
            let mut b = vec![0u16; 32768];
            let mut n = b.len() as u32;
            QueryFullProcessImageNameW(
                self.handle.0,
                PROCESS_NAME_WIN32,
                PWSTR(b.as_mut_ptr()),
                &mut n,
            )?;
            Ok(String::from_utf16_lossy(&b[..n as usize]))
        }
    }
    pub fn bound(binding: &Binding) -> Result<Self> {
        let p = Self::open(binding.pid)?;
        let image = p.image()?.to_lowercase();
        if !(image.ends_with("\\weixin.exe") || image.ends_with("\\wechat.exe"))
            || (p.creation()? - binding.created_at).abs() > 0.01
        {
            bail!("微信进程已改变，请重新绑定；未切换其他账号");
        }
        Ok(p)
    }
    pub fn read(&self, addr: usize, n: usize) -> Option<Vec<u8>> {
        unsafe {
            let mut b = vec![0; n];
            let mut read = 0;
            let _ = ReadProcessMemory(
                self.handle.0,
                addr as *const _,
                b.as_mut_ptr() as *mut _,
                n,
                Some(&mut read),
            );
            if read == 0 {
                None
            } else {
                b.truncate(read);
                Some(b)
            }
        }
    }
    pub fn find(&self, needles: &[Vec<u8>]) -> Vec<Vec<usize>> {
        let mut hits = vec![vec![]; needles.len()];
        let mut addr = 0usize;
        unsafe {
            loop {
                let mut mbi: MEMORY_BASIC_INFORMATION = zeroed();
                if VirtualQueryEx(
                    self.handle.0,
                    Some(addr as *const _),
                    &mut mbi,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                ) == 0
                {
                    break;
                }
                let base = mbi.BaseAddress as usize;
                let len = mbi.RegionSize;
                if mbi.State == MEM_COMMIT
                    && mbi.Protect.0 & 0xe6 != 0
                    && mbi.Protect.0 & PAGE_GUARD.0 == 0
                    && len > 0
                    && len < 0x10000000
                {
                    if let Some(data) = self.read(base, len) {
                        for (i, n) in needles.iter().enumerate() {
                            for pos in memchr::memmem::find_iter(&data, n) {
                                hits[i].push(base + pos);
                            }
                        }
                    }
                }
                let Some(next) = base.checked_add(len) else {
                    break;
                };
                if next <= addr {
                    break;
                }
                addr = next;
            }
        }
        hits
    }
    pub fn account_root(&self) -> Result<PathBuf> {
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Entry {
            object: usize,
            pid: usize,
            handle: usize,
            access: u32,
            trace: u16,
            kind: u16,
            attributes: u32,
            reserved: u32,
        }
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQuerySystemInformation(
                class: u32,
                buffer: *mut c_void,
                size: u32,
                needed: *mut u32,
            ) -> i32;
        }
        let mut buffer = vec![0u8; 4 * 1024 * 1024];
        loop {
            let mut needed = 0;
            let status = unsafe {
                NtQuerySystemInformation(
                    64,
                    buffer.as_mut_ptr() as _,
                    buffer.len() as u32,
                    &mut needed,
                )
            };
            if status >= 0 {
                break;
            }
            if status as u32 != 0xc0000004 {
                bail!("无法读取绑定进程的数据库句柄");
            }
            buffer.resize((needed as usize + 65536).max(buffer.len() * 2), 0);
            if buffer.len() > 256 * 1024 * 1024 {
                bail!("句柄表过大");
            }
        }
        let count = usize::from_ne_bytes(buffer[..size_of::<usize>()].try_into()?);
        let mut roots = BTreeSet::new();
        for i in 0..count {
            let offset = 2 * size_of::<usize>() + i * size_of::<Entry>();
            if offset + size_of::<Entry>() > buffer.len() {
                break;
            }
            let item =
                unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(offset) as *const Entry) };
            if item.pid != self.pid as usize {
                continue;
            }
            unsafe {
                let mut copied = HANDLE::default();
                if DuplicateHandle(
                    self.handle.0,
                    HANDLE(item.handle as *mut _),
                    GetCurrentProcess(),
                    &mut copied,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )
                .is_err()
                {
                    continue;
                }
                let copied = Handle(copied);
                if GetFileType(copied.0) != FILE_TYPE_DISK {
                    continue;
                }
                let mut path = vec![0u16; 32768];
                let n = GetFinalPathNameByHandleW(copied.0, &mut path, FILE_NAME_NORMALIZED);
                if n == 0 || n as usize >= path.len() {
                    continue;
                }
                let path = PathBuf::from(String::from_utf16_lossy(&path[..n as usize]));
                for parent in path.ancestors() {
                    if parent
                        .file_name()
                        .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("db_storage"))
                    {
                        if let Some(p) = parent.parent() {
                            roots.insert(p.to_path_buf());
                        }
                        break;
                    }
                }
            }
        }
        if roots.len() != 1 {
            bail!("绑定微信未登录或数据库账号不唯一；未自动切换账号");
        }
        Ok(roots.into_iter().next().unwrap())
    }
}
pub fn dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
pub fn capture(h: HWND) -> Result<image::RgbImage> {
    capture_window(h, 2)
}
pub fn capture_gdi(h: HWND) -> Result<image::RgbImage> {
    capture_window(h, 0)
}
fn capture_window(h: HWND, flags: u32) -> Result<image::RgbImage> {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let mut r = RECT::default();
        GetWindowRect(h, &mut r)?;
        let (w, hgt) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || hgt <= 0 || w * hgt > 8_000_000 {
            bail!("窗口尺寸无效");
        }
        let source = GetWindowDC(Some(h));
        if source.is_invalid() {
            bail!("窗口绘图句柄不可用");
        }
        let memory = CreateCompatibleDC(Some(source));
        let bitmap = CreateCompatibleBitmap(source, w, hgt);
        let previous = SelectObject(memory, bitmap.into());
        let result = (|| -> Result<image::RgbImage> {
            if !windows::Win32::Storage::Xps::PrintWindow(
                h,
                memory,
                windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(flags),
            )
            .as_bool()
            {
                BitBlt(memory, 0, 0, w, hgt, Some(source), 0, 0, SRCCOPY)?;
            }
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as _,
                    biWidth: w,
                    biHeight: -hgt,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..zeroed()
                },
                ..zeroed()
            };
            let mut raw = vec![0u8; w as usize * hgt as usize * 4];
            if GetDIBits(
                memory,
                bitmap,
                0,
                hgt as _,
                Some(raw.as_mut_ptr() as _),
                &mut info,
                DIB_RGB_COLORS,
            ) == 0
            {
                bail!("窗口抓图不可用");
            }
            let rgb: Vec<_> = raw
                .chunks_exact(4)
                .flat_map(|v| [v[2], v[1], v[0]])
                .collect();
            Ok(image::RgbImage::from_raw(w as _, hgt as _, rgb).context("窗口像素尺寸错误")?)
        })();
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(Some(h), source);
        result
    }
}
pub fn hwnd(binding: &Binding) -> HWND {
    HWND(binding.hwnd as *mut c_void)
}
#[derive(Clone, PartialEq, Eq)]
pub struct InputSnapshot {
    pub foreground: isize,
    pub focus: isize,
    pub cursor: (i32, i32),
    pub clipboard: u32,
}
pub fn input_snapshot() -> InputSnapshot {
    unsafe {
        let foreground = GetForegroundWindow();
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let thread = GetWindowThreadProcessId(foreground, None);
        let _ = GetGUIThreadInfo(thread, &mut info);
        InputSnapshot {
            foreground: foreground.0 as isize,
            focus: info.hwndFocus.0 as isize,
            cursor: (cursor.x, cursor.y),
            clipboard: windows::Win32::System::DataExchange::GetClipboardSequenceNumber(),
        }
    }
}
pub fn thread_windows(h: HWND) -> Vec<HWND> {
    struct Search {
        root: HWND,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn cb(h: HWND, p: LPARAM) -> BOOL {
        unsafe {
            let s = &mut *(p.0 as *mut Search);
            if h != s.root && IsWindowVisible(h).as_bool() {
                s.found.push(h);
            }
        }
        BOOL(1)
    }
    let mut s = Search {
        root: h,
        found: vec![],
    };
    unsafe {
        let thread = GetWindowThreadProcessId(h, None);
        let _ = EnumThreadWindows(thread, Some(cb), LPARAM(&mut s as *mut _ as isize));
    }
    s.found
}
pub fn validate_window(binding: &Binding) -> Result<HWND> {
    let _ = Process::bound(binding)?;
    let h = hwnd(binding);
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            bail!("绑定微信窗口已关闭");
        }
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid != binding.pid {
            bail!("窗口所属进程已改变");
        }
    }
    Ok(h)
}
pub fn restore_chat(binding: &Binding, data: &std::path::Path) -> Result<()> {
    unsafe {
        let h = validate_window(binding)?;
        let process = Process::bound(binding)?;
        let module = PathBuf::from(process.image()?)
            .parent()
            .unwrap()
            .join("4.1.15.13/Weixin.dll");
        if std::fs::metadata(module).map(|m| m.len()).unwrap_or(0) != 201552944 {
            bail!("当前版本没有已验证的微信原生恢复入口");
        }
        let _guard = crate::popup::Guard::install(h, data)?;
        struct Search {
            pid: u32,
            found: Vec<HWND>,
        }
        unsafe extern "system" fn cb(w: HWND, p: LPARAM) -> BOOL {
            unsafe {
                let s = &mut *(p.0 as *mut Search);
                let mut pid = 0;
                GetWindowThreadProcessId(w, Some(&mut pid));
                let mut class = [0u16; 128];
                let n = GetClassNameW(w, &mut class);
                if pid == s.pid
                    && String::from_utf16_lossy(&class[..n as usize])
                        .ends_with("WxTrayIconMessageWindowClass")
                {
                    s.found.push(w);
                }
            }
            BOOL(1)
        }
        let mut search = Search {
            pid: binding.pid,
            found: vec![],
        };
        EnumWindows(Some(cb), LPARAM(&mut search as *mut _ as isize))?;
        if search.found.len() != 1 {
            bail!("绑定微信的原生托盘窗口不唯一；未自动选择其他账号");
        }
        let foreground = GetForegroundWindow();
        PostMessageW(Some(search.found[0]), 0x8385, WPARAM(0), LPARAM(0x203))?;
        std::thread::sleep(std::time::Duration::from_millis(500));
        offscreen(h)?;
        if GetForegroundWindow() != foreground {
            bail!("原生恢复改变了前台窗口；未继续发送");
        }
        Ok(())
    }
}
pub fn offscreen(h: HWND) -> Result<()> {
    unsafe {
        if !IsWindowVisible(h).as_bool() {
            let mut stored = RECT::default();
            GetWindowRect(h, &mut stored)?;
            if stored.right >= GetSystemMetrics(SM_XVIRTUALSCREEN)
                || stored.bottom >= GetSystemMetrics(SM_YVIRTUALSCREEN)
            {
                bail!("隐藏窗口没有已验证的离屏位置；未显示窗口");
            }
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        }
        if IsIconic(h).as_bool() {
            let mut placement = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as _,
                ..zeroed()
            };
            GetWindowPlacement(h, &mut placement)?;
            if placement.rcNormalPosition.right >= GetSystemMetrics(SM_XVIRTUALSCREEN)
                && placement.rcNormalPosition.bottom >= GetSystemMetrics(SM_YVIRTUALSCREEN)
            {
                bail!("最小化窗口没有已验证的离屏位置，请先绑定完整窗口");
            }
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        }
        let mut r = RECT::default();
        GetWindowRect(h, &mut r)?;
        let w = r.right - r.left;
        let height = r.bottom - r.top;
        if w < 600 || height < 400 {
            bail!("当前不是已登录的完整微信聊天窗口");
        }
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        if r.right >= left || r.bottom >= top {
            SetWindowPos(
                h,
                None,
                left - w - 1024,
                top - height - 1024,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            )?;
        }
        GetWindowRect(h, &mut r)?;
        if r.right >= left || r.bottom >= top {
            bail!("无法保证微信窗口离屏；已停止发送");
        }
        Ok(())
    }
}
#[derive(Clone, serde::Serialize)]
pub struct Window {
    pub binding: Binding,
    pub title: String,
    pub offscreen: bool,
}
pub fn windows() -> Vec<Window> {
    unsafe extern "system" fn callback(h: HWND, p: LPARAM) -> BOOL {
        let result = unsafe { &mut *(p.0 as *mut Vec<Window>) };
        unsafe {
            if !IsWindowVisible(h).as_bool() {
                return BOOL(1);
            }
            let mut class = [0u16; 256];
            let n = GetClassNameW(h, &mut class);
            if !String::from_utf16_lossy(&class[..n as usize]).starts_with("Qt") {
                return BOOL(1);
            }
            let mut pid = 0;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let Ok(process) = Process::open(pid) else {
                return BOOL(1);
            };
            let Ok(image) = process.image() else {
                return BOOL(1);
            };
            if !image.to_lowercase().ends_with("\\weixin.exe")
                && !image.to_lowercase().ends_with("\\wechat.exe")
            {
                return BOOL(1);
            }
            let mut rect = RECT::default();
            if DwmGetWindowAttribute(
                h,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut _ as _,
                size_of::<RECT>() as _,
            )
            .is_err()
                || rect.right - rect.left < 580
                || rect.bottom - rect.top < 380
            {
                return BOOL(1);
            }
            let mut title = [0u16; 512];
            let n = GetWindowTextW(h, &mut title);
            if let Ok(created_at) = process.creation() {
                result.push(Window {
                    binding: Binding {
                        pid,
                        created_at,
                        hwnd: h.0 as isize,
                        bound_at: crate::config::now(),
                    },
                    title: String::from_utf16_lossy(&title[..n as usize]),
                    offscreen: rect.right < GetSystemMetrics(SM_XVIRTUALSCREEN)
                        && rect.bottom < GetSystemMetrics(SM_YVIRTUALSCREEN),
                });
            }
        }
        BOOL(1)
    }
    let mut result = vec![];
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut result as *mut _ as isize));
    }
    result
}

pub fn keys(
    process: &Process,
    pages: &[(String, Vec<u8>)],
) -> Result<std::collections::HashMap<String, Vec<u8>>> {
    let name = b"com.Tencent.WCDB.Config.Cipher";
    let needles = process.find(&[name.to_vec()]).remove(0);
    let pointers: Vec<_> = needles
        .iter()
        .map(|p| {
            [
                p.to_le_bytes().as_slice(),
                (name.len() as u64).to_le_bytes().as_slice(),
            ]
            .concat()
        })
        .collect();
    let objects = process.find(&pointers);
    let mask = hex::decode("d2c7442458020000004889442450488b450048844c2448488944254048584c24")?;
    let regex = regex::bytes::Regex::new("[xX]'([0-9a-fA-F]{64,192})'")?;
    let mut tested = HashSet::new();
    let mut keys = std::collections::HashMap::new();
    for addresses in objects {
        for addr in addresses {
            let Some(node) = process.read(addr.saturating_sub(16), 80) else {
                continue;
            };
            let ptr = u64::from_le_bytes(node[40..48].try_into()?) as usize;
            if !(0x10000..0x800000000000).contains(&ptr) {
                continue;
            }
            let Some(obj) = process.read(ptr + 0x88, 40) else {
                continue;
            };
            if obj.len() < 24 {
                continue;
            }
            let data = u64::from_le_bytes(obj[8..16].try_into()?) as usize;
            let len = u64::from_le_bytes(obj[16..24].try_into()?) as usize;
            if !(1..=1024).contains(&len) {
                continue;
            }
            let Some(mut blob) = process.read(data, len) else {
                continue;
            };
            for (i, b) in blob.iter_mut().enumerate() {
                *b ^= mask[i % mask.len()];
            }
            for capture in regex.captures_iter(&blob) {
                let run = &capture[1];
                let mut starts = vec![0];
                if run.len() > 96 {
                    starts.extend((0..run.len() - 63).step_by(32));
                    starts.push(run.len() - 64);
                }
                for s in starts {
                    if s + 64 > run.len() {
                        continue;
                    }
                    let key = hex::decode(&run[s..s + 64])?;
                    if !tested.insert(key.clone()) {
                        continue;
                    }
                    for (rel, page) in pages {
                        if keys.contains_key(rel) {
                            continue;
                        }
                        if crate::database::verify_key(&key, page) {
                            keys.insert(rel.clone(), key.clone());
                        } else if s + 96 <= run.len() {
                            let extended = hex::decode(&run[s..s + 96])?;
                            if crate::database::verify_key(&extended, page) {
                                keys.insert(rel.clone(), extended);
                            }
                        }
                    }
                }
            }
        }
    }
    if keys.len() != pages.len() {
        bail!(
            "绑定账号数据库密钥验证未完成（{}/{}），未扫描其他账号",
            keys.len(),
            pages.len()
        );
    }
    Ok(keys)
}
