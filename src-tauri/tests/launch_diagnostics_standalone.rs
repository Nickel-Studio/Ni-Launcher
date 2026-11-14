//! 在 `lib.rs` 尚未接入诊断模块时，独立编译并运行新增模块的测试。
//!
//! runtime-core 接入后仍可保留此测试入口；它不依赖 Tauri / AppHandle，
//! 只验证纯逻辑、文件落盘和脱敏行为。

#[path = "../src/launch_log_store.rs"]
pub mod launch_log_store;

#[path = "../src/launch_diagnostics.rs"]
pub mod launch_diagnostics;

#[path = "../src/launch_diagnostics_tests.rs"]
mod cases;
