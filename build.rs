fn main() {
    embed_resource::compile("assets/jot.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
