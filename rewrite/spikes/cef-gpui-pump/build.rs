fn main() {
    // cef-dll-sys copies libcef.so and CEF's resources next to the binary on Linux.
    #[cfg(target_os = "linux")]
    println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
}
