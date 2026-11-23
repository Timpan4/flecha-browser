fn main() {
    // The runner stages libcef.so next to the binary (see scripts/run-checks.sh).
    // Build scripts run on the host, so check the target through Cargo's env.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
