fn main() {
    // Debug builds carry the element info that lets scripted runs audit what screen readers see
    // (the `a11y` step); release builds don't need it.
    let debug = std::env::var("PROFILE").is_ok_and(|p| p == "debug");
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
