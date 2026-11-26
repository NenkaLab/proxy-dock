fn main() {
    // the exe files' icon in Explorer and the taskbar (Windows resource; needs the Windows SDK's rc.exe)
    // (skipped until examples/make_ico.rs has made the .ico: that example builds this crate first)
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=icons/proxydock.ico");
    // (embed-resource is a build dependency on Windows only: Windows builds are made on Windows)
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::path::Path::new("icons/proxydock.ico").exists()
    {
        embed_resource::compile("app.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
    let config = slint_build::CompilerConfiguration::new().with_style("material".into());
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
}
