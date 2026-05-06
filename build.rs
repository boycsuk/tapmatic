fn main() {
    println!("cargo:rerun-if-changed=tapmatic.ico");
    println!("cargo:rerun-if-changed=app.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let _ = embed_resource::compile("app.rc", embed_resource::NONE);
    }
}
