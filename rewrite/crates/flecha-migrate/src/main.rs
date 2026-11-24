use std::process::ExitCode;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() -> ExitCode {
    use std::path::Path;

    const USAGE: &str = "Usage: flecha-migrate <user-data-dir> <profile-name> <destination>";

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (user_data_dir, profile_name, destination) = match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            println!(
                "\nClose Chromium before migrating. Use absolute paths and a new destination outside the Chromium user data directory."
            );
            return ExitCode::SUCCESS;
        }
        [user_data_dir, profile_name, destination] => (
            Path::new(user_data_dir),
            profile_name,
            Path::new(destination),
        ),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match flecha_migrate::migrate_profile(user_data_dir, profile_name, destination) {
        Ok(report) => {
            println!("Spaces: {}", report.spaces);
            println!("Windows: {}", report.windows);
            println!("Open tabs: {}", report.open_tabs);
            println!("Archive entries: {}", report.archive_entries);
            println!("Ad-block sites: {}", report.ad_block_sites);
            println!("Boosts: {}", report.boosts);
            let yes_no = |copied| if copied { "yes" } else { "no" };
            println!("Cookies copied: {}", yes_no(report.cookies_copied));
            println!("Web storage copied: {}", yes_no(report.web_storage_copied));
            println!(
                "OSCrypt settings copied: {}",
                yes_no(report.os_crypt_copied)
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("flecha-migrate: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() -> ExitCode {
    eprintln!("flecha-migrate is supported on Linux and macOS only.");
    ExitCode::FAILURE
}
