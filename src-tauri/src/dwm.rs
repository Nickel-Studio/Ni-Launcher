//! Windows 窗口外观微调（DWM）。
//!
//! 背景：本应用使用「`transparent: true` + `decorations: false`」的无边框透明窗口，
//! 圆角与所有视觉均由 CSS 绘制（`.ni-shell` 的 `border-radius`）。
//!
//! 但 Windows 仍会在窗口上画两种「非客户区」装饰，它们绘制在 WebView 内容之上、
//! 纯 CSS 无法消除，必须通过 DWM 属性关掉：
//!
//! 1. `DWMWA_BORDER_COLOR` —— Win11 给窗口画的 1px 边框。
//!    表现为「沿四边（含圆角）的一圈浅色细线」。
//!    → 设为 `DWMWA_COLOR_NONE` 让系统不画。
//!
//! 2. `DWMWA_NCRENDERING_POLICY` —— 窗口顶部残留的一条非客户区（caption 条）：
//!    表现为「**只有顶边**有一条 2~4px 的浅色细线」，侧边/底边没有。
//!    → 设为 `DWMNCRP_DISABLED` 让 DWM 完全不渲染非客户区。
//!    同时打开 `DWMWA_USE_IMMERSIVE_DARK_MODE`，让可能在别处残留的非客户区
//!    也用深色绘制（与本应用的深色基调一致）。
//!
//! 刻意使用 `dwmapi` 的裸 FFI，而不是引入 `windows` crate：
//! 零新依赖、不受 crate 版本对齐影响，且非 Windows 平台为零成本空实现。
//!
//! 注意：`NCRENDERING_POLICY = DISABLED` 会让 DWM 不再为该窗口绘制系统投影，
//! 窗口在桌面上的「悬浮感」会减弱 —— 这是用「绝对干净的边缘」换来的，可接受。

#[cfg(windows)]
pub fn apply_frameless_tweaks(hwnd: *mut std::ffi::c_void) -> bool {
    use std::ffi::c_void;

    /// DWM 窗口属性：非客户区渲染策略。
    const DWMWA_NCRENDERING_POLICY: u32 = 2;
    /// DWM 窗口属性：深色模式（Windows 10 1809+ 为 19，20 起为通用值）。
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    /// DWM 窗口属性：边框颜色（Windows 11 build 22000+）。
    const DWMWA_BORDER_COLOR: u32 = 34;

    /// 不渲染非客户区。
    const DWMNCRP_DISABLED: u32 = 2;
    /// `DWMWA_COLOR_NONE`：不绘制边框。
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            dw_attribute: u32,
            pv_attribute: *const c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    fn set_u32(hwnd: *mut c_void, attr: u32, value: u32) -> bool {
        // SAFETY: `hwnd` 来自 Tauri 的窗口句柄；`value` 在调用期间保持存活；
        // 传入的长度与实际类型一致。返回 0（S_OK）表示成功。
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attr,
                &value as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            ) == 0
        }
    }

    // 三条都是「尽力而为」：老系统不支持某个属性时返回失败，
    // 但不影响其它属性生效，因此用 & 汇总成"是否全部成功"。
    let no_caption = set_u32(hwnd, DWMWA_NCRENDERING_POLICY, DWMNCRP_DISABLED);
    let dark = set_u32(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    let no_border = set_u32(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);

    no_caption & dark & no_border
}

/// 非 Windows 平台：空操作（圆角与边框完全由 CSS 负责）。
#[cfg(not(windows))]
pub fn apply_frameless_tweaks(_hwnd: *mut std::ffi::c_void) -> bool {
    false
}

/// 诊断：把窗口的真实样式位与尺寸打印出来。
///
/// 用途：判断那条「顶边细线」到底是系统残留的原生框架（caption / sysmenu / thickframe），
/// 还是我们自己的 CSS 造成的。启动时会把结果打到 stderr。
#[cfg(windows)]
pub fn describe_window(hwnd: *mut std::ffi::c_void) -> String {
    use std::ffi::c_void;

    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongW(hwnd: *mut c_void, index: i32) -> i32;
        fn GetWindowRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
        fn GetClientRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
    }

    // SAFETY: hwnd 来自真实窗口；Rect 为 POD 且已初始化。
    let (style, ex_style, wr, cr) = unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut wr = Rect::default();
        let mut cr = Rect::default();
        GetWindowRect(hwnd, &mut wr);
        GetClientRect(hwnd, &mut cr);
        (style, ex_style, wr, cr)
    };

    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_THICKFRAME: u32 = 0x0004_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_BORDER: u32 = 0x0080_0000;
    const WS_DLGFRAME: u32 = 0x0040_0000;

    let flag = |mask: u32| if style & mask == mask { "Y" } else { "n" };

    format!(
        "style=0x{style:08X} [caption={} thickframe={} sysmenu={} popup={} border={} dlgframe={}] \
         exstyle=0x{ex_style:08X} window={}x{} client={}x{}",
        flag(WS_CAPTION),
        flag(WS_THICKFRAME),
        flag(WS_SYSMENU),
        flag(WS_POPUP),
        flag(WS_BORDER),
        flag(WS_DLGFRAME),
        wr.right - wr.left,
        wr.bottom - wr.top,
        cr.right - cr.left,
        cr.bottom - cr.top,
    )
}

/// 非 Windows 平台：无诊断信息。
#[cfg(not(windows))]
pub fn describe_window(_hwnd: *mut std::ffi::c_void) -> String {
    "（非 Windows 平台）".to_string()
}
