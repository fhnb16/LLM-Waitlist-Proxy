fn main() {
    // Нативное окно: стиль fluent следует светлой/тёмной теме Windows. Исходные строки
    // интерфейса — русские; английский перевод вшивается из translations/en.
    let cfg = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", cfg).expect("slint ui");
    println!("cargo:rerun-if-changed=translations");
    tauri_build::build();
}
