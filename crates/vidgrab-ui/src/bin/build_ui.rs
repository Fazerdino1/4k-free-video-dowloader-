//! Builds and minifies the static `dist/index.html` asset for `vidgrab-ui`.

use std::path::PathBuf;
use vidgrab_shared::Settings;
use vidgrab_ui::pages::render_app_shell;

fn minify_html_whitespace(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let html = minify_html_whitespace(&render_app_shell(&[], &Settings::default()));
    let dist_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist");
    std::fs::create_dir_all(&dist_dir)?;
    let out_path = dist_dir.join("index.html");
    std::fs::write(&out_path, html)?;
    println!("Generated {}", out_path.display());
    Ok(())
}
