//! Embeds the icon and version details in the Windows executable.
//! (`cargo xtask icons` regenerates `assets/fizl-apicraft.ico`.)

fn main() {
    println!("cargo:rerun-if-changed=assets/fizl-apicraft.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/fizl-apicraft.ico")
        .set("ProductName", "Fizl ApiCraft")
        .set("FileDescription", "Fizl ApiCraft")
        .set("CompanyName", "Fizl Ltd")
        .set("LegalCopyright", "Copyright (C) Fizl Ltd & Simon Soanes. Apache License 2.0");
    // A missing resource compiler only costs the icon, not the build.
    if let Err(e) = res.compile() {
        println!("cargo:warning=building without an .exe icon: {e}");
    }
}
