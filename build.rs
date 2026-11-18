fn main() {
    println!("cargo:rerun-if-changed=assets/icon.res");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let resource = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("assets/icon.res");
        println!("cargo:rustc-link-arg={}", resource.display());
    }
}
