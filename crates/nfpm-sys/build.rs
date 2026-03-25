use std::path::PathBuf;
use std::{env, fs, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=src/go");
    env::set_current_dir("src/go").unwrap();
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = PathBuf::from(&out_dir);

    // The docs.rs builder blocks network access, which would require vendoring everything. I don't
    // want to go through that hassle right now, so just generate a dummy file to allow the docs to
    // build.
    if env::var("DOCS_RS").is_ok() {
        fs::write(out_path.join("bindings.rs"), "").unwrap();
        return;
    }

    let status = Command::new("go")
        .args(["build", "-buildmode=c-archive", "-o"])
        .arg(format!("{out_dir}/libkoca-nfpm.a"))
        .arg("main.go")
        .status()
        .expect("`go build` failed. Is `go` installed and on the latest version?");

    if !status.success() {
        panic!("`go build` failed. Is `go` on the latest version?");
    }

    env::set_current_dir("../../").unwrap();
    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static=koca-nfpm");

    let bindings = bindgen::Builder::default()
        .header(format!("{out_dir}/libkoca-nfpm.h"))
        .allowlist_function("runBundle")
        .allowlist_item("STATUS_SUCCESS")
        .allowlist_item("STATUS_JSON")
        .allowlist_item("STATUS_OUTPUT_FILE")
        .allowlist_item("STATUS_PKG_CREATION")
        .generate()
        .unwrap();
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .unwrap();
}
