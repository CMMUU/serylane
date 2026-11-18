use std::{env, fs, path::PathBuf};

fn main() {
    let directory = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join("../locales");
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut files = fs::read_dir(directory)
        .expect("locate application languages")
        .map(|file| file.expect("read language resource").path())
        .filter(|file| file.extension().is_some_and(|extension| extension == "ftl"))
        .collect::<Vec<_>>();
    files.sort();
    let mut content = String::from("&[\n");
    for file in files {
        let tag = file
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("locale ID");
        println!("cargo:rerun-if-changed={}", file.display());
        content.push_str(&format!(
            "    ({tag:?}, include_str!({:?})),\n",
            file.to_str().expect("UTF-8 locale path")
        ));
    }
    content.push_str("]\n");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output"));
    fs::write(output.join("locale_resources.rs"), content).expect("embed locale resources");
    tauri_build::build()
}
