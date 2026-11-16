//! Compiles Helix's own symbolic icons (`icons/`) into a GResource that
//! [`helix_app_common::register_icons`] loads at startup.

use std::path::Path;
use std::process::Command;

fn main() {
    let src = Path::new("icons");
    println!("cargo:rerun-if-changed=icons");
    let mut files = Vec::new();
    collect(src, src, &mut files);
    files.sort();
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gresources>\n  <gresource prefix=\"/org/helixdesktop/Apps/icons\">\n",
    );
    for f in &files {
        xml.push_str(&format!("    <file>{f}</file>\n"));
    }
    xml.push_str("  </gresource>\n</gresources>\n");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let xml_path = Path::new(&out).join("icons.gresource.xml");
    std::fs::write(&xml_path, xml).expect("write gresource xml");
    let target = Path::new(&out).join("icons.gresource");
    let status = Command::new("glib-compile-resources")
        .arg("--sourcedir")
        .arg(src)
        .arg("--target")
        .arg(&target)
        .arg(&xml_path)
        .status()
        .expect("glib-compile-resources (libglib2.0-dev-bin) is needed to build helix-app-common");
    assert!(status.success(), "glib-compile-resources failed");
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read icons dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "svg") {
            let rel = path.strip_prefix(root).expect("relative icon path");
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}
