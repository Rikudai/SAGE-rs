fn main() {
    println!("cargo:rerun-if-changed=sage.rc");
    println!("cargo:rerun-if-changed=../assets/app_icon.ico");
    let _ = embed_resource::compile("sage.rc", embed_resource::NONE);
}
