use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");

    // Let the loader find libcef.so next to the binary without LD_LIBRARY_PATH.
    #[cfg(target_os = "linux")]
    println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");

    // Workaround for tauri-apps/cef-rs#522: at the pinned rev, cef-dll-sys copies the CEF
    // runtime to OUT_DIR's third parent, which under Cargo 1.100's default build-dir layout is
    // target/<profile>/build instead of target/<profile>. Copy it next to the binary here,
    // using the lookup from #522. Remove this once the pin includes that fix.
    #[cfg(target_os = "linux")]
    copy_cef_runtime().expect("copy CEF runtime next to the binary");
}

#[cfg(target_os = "linux")]
fn copy_cef_runtime() -> io::Result<()> {
    let cef_dir = PathBuf::from(
        env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR").expect("cef-dll-sys exports CEF_DIR"),
    );
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "build"))
        .and_then(Path::parent)
        .ok_or_else(|| {
            io::Error::other(format!(
                "OUT_DIR is not in a build dir: {}",
                out_dir.display()
            ))
        })?
        .to_path_buf();
    link_files(&cef_dir, &profile_dir)?;
    link_files(&cef_dir.join("locales"), &profile_dir.join("locales"))
}

/// Hard-links (or, across filesystems, copies) the regular files of `src` into `dst`.
#[cfg(target_os = "linux")]
fn link_files(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let target = dst.join(entry.file_name());
        match fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if fs::hard_link(entry.path(), &target).is_err() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
