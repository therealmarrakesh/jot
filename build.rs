fn main() {
    println!("cargo:rerun-if-changed=assets/jot.rc");
    println!("cargo:rerun-if-changed=assets/jot.ico");
    embed_resource::compile("assets/jot.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
