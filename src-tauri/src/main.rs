// 发布版使用 GUI 子系统，避免 Windows 为桌面进程创建控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ni_launcher_lib::run()
}
