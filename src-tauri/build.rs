fn main() {
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // 只提供现有资源的搜索目录，cfg(test) 模块显式链接，避免 bin 资源被重复加入。
        if let Some(out) = std::env::var_os("OUT_DIR") {
            println!(
                "cargo:rustc-link-search=native={}",
                std::path::PathBuf::from(out).display()
            );
        }
    }
}
