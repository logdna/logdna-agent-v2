fn main() {
    #[cfg(feature = "dep_audit")]
    auditable_build::collect_dependency_list();

    // only run if target os is windows
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        if cfg!(unix) {
            // windres tool
            if let Ok(path) = std::env::var("XWIN_TOOLKIT_BIN_PATH") {
                res.set_toolkit_path(&path);
            }
        }
        /*
        // Set more stuff in the resource file
        res.set_language(0x0409)
            .set_manifest_file("manifest.xml");

        */

        if let Err(e) = res.compile() {
            println!("cargo:error={}", e);
            std::process::exit(1);
        }
    }
}
