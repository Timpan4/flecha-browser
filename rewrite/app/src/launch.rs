use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use flecha_ui::Options;

pub const USAGE: &str = "\
Usage: flecha [--profile DIR] [--open URL | URL]
       flecha --verify
       flecha --help | -h

Desktop profile: --profile, FLECHA_PROFILE, or $XDG_DATA_HOME/flecha-rewrite
(defaults to $HOME/.local/share/flecha-rewrite when XDG_DATA_HOME is unset).
Use rewrite/tools/isolated-run.sh for --verify.
";

const ISOLATED_LAUNCHER: &str = "Flecha must be started by rewrite/tools/isolated-run.sh";

#[derive(Default)]
pub struct Environment {
    isolated: bool,
    home: Option<PathBuf>,
    runtime_dir: Option<PathBuf>,
    profile: Option<PathBuf>,
    data_home: Option<PathBuf>,
    config_home: Option<PathBuf>,
    chrome_config_home: Option<PathBuf>,
    adblock_fixture_port: Option<OsString>,
}

impl Environment {
    pub fn read() -> Self {
        let path = |name| std::env::var_os(name).map(PathBuf::from);
        Self {
            isolated: std::env::var_os("FLECHA_LAUNCHER").as_deref()
                == Some(OsStr::new("isolated")),
            home: path("HOME"),
            runtime_dir: path("XDG_RUNTIME_DIR"),
            profile: path("FLECHA_PROFILE"),
            data_home: path("XDG_DATA_HOME"),
            config_home: path("XDG_CONFIG_HOME"),
            chrome_config_home: path("CHROME_CONFIG_HOME"),
            adblock_fixture_port: std::env::var_os("FLECHA_VERIFY_ADBLOCK_PORT"),
        }
    }

    fn isolated_profile(&self) -> Result<PathBuf> {
        // Compare the raw strings so an alias cannot stand in for a launcher path.
        ensure!(
            self.isolated
                && self.home.as_deref().map(Path::as_os_str)
                    == Some(OsStr::new("/mnt/scratch/home"))
                && self.runtime_dir.as_deref().map(Path::as_os_str)
                    == Some(OsStr::new("/mnt/scratch/runtime")),
            "{ISOLATED_LAUNCHER}"
        );
        let profile = PathBuf::from("/mnt/scratch/profile");
        ensure!(
            self.profile.as_deref().map(Path::as_os_str) == Some(profile.as_os_str()),
            "the launcher must supply a scratch profile"
        );
        Ok(profile)
    }

    fn desktop_profile(&self, argument: Option<&Path>) -> Result<PathBuf> {
        let home = absolute(self.home.as_deref()).with_context(|| {
            format!("Desktop launch requires an absolute HOME. {ISOLATED_LAUNCHER} for isolation")
        })?;
        let profile = match argument.or(self.profile.as_deref()) {
            Some(path) => {
                ensure!(!path.as_os_str().is_empty(), "Flecha profile path is empty");
                path.to_path_buf()
            }
            None => absolute(self.data_home.as_deref())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| home.join(".local/share"))
                .join("flecha-rewrite"),
        };
        let profile = resolve_path(&profile)?;
        let config_homes = [
            Some(home.join(".config")),
            absolute(self.config_home.as_deref()).map(Path::to_path_buf),
            absolute(self.chrome_config_home.as_deref()).map(Path::to_path_buf),
        ];
        for config_home in config_homes.into_iter().flatten() {
            let shipping = config_home.join("flecha-browser");
            ensure!(
                !profile.starts_with(resolve_path(&shipping)?),
                "Flecha profile must be outside the shipping profile at {}",
                shipping.display()
            );
        }
        Ok(profile)
    }
}

fn absolute(path: Option<&Path>) -> Option<&Path> {
    path.filter(|path| path.is_absolute())
}

fn resolve_path(path: &Path) -> Result<PathBuf> {
    // Resolve symlinks before interpreting "..", including with a new profile leaf.
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                match resolved.canonicalize() {
                    Ok(existing) => resolved = existing,
                    Err(error) if error.kind() == ErrorKind::NotFound && !resolved.is_symlink() => {
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("resolve {}", resolved.display()));
                    }
                }
            }
            _ => resolved.push(component),
        }
    }
    Ok(resolved)
}

pub enum Launch {
    Help,
    CefChild,
    Browser { profile: PathBuf, options: Options },
}

impl Launch {
    pub fn resolve(args: &[String], env: &Environment) -> Result<Self> {
        let verify = args.iter().any(|arg| arg == "--verify");
        let isolated_profile = (verify || env.isolated)
            .then(|| env.isolated_profile())
            .transpose()?;
        if args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
        {
            return Ok(Self::Help);
        }
        // CEF re-executes this binary with its own switches.
        if args.iter().any(|arg| arg.starts_with("--type=")) {
            return Ok(Self::CefChild);
        }

        let mut initial = None;
        let mut profile_arg = None;
        let mut args = args.iter().map(String::as_str);
        while let Some(arg) = args.next() {
            match arg {
                "--open" => {
                    let url = args.next().filter(|url| !url.starts_with('-'));
                    initial = Some(url.context("--open requires an address")?.to_owned());
                }
                "--profile" => {
                    let path = args.next().filter(|path| !path.starts_with('-'));
                    profile_arg = Some(Path::new(path.context("--profile requires a directory")?));
                }
                "--verify" => {}
                _ if !arg.starts_with('-') => {
                    ensure!(initial.is_none(), "expected a single URL");
                    initial = Some(arg.to_owned());
                }
                _ => bail!("unknown Flecha option"),
            }
        }
        let profile = match isolated_profile {
            Some(profile) => {
                ensure!(
                    profile_arg.is_none(),
                    "--profile is only available for desktop launch"
                );
                profile
            }
            None => env.desktop_profile(profile_arg)?,
        };
        let adblock_fixture_port = env
            .adblock_fixture_port
            .as_deref()
            .filter(|_| verify)
            .map(|port| {
                port.to_str()
                    .and_then(|port| port.parse().ok())
                    .context("FLECHA_VERIFY_ADBLOCK_PORT must be a nonzero TCP port")
            })
            .transpose()?;
        Ok(Self::Browser {
            profile,
            options: Options {
                initial,
                verify,
                adblock_fixture_port,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop_env() -> Environment {
        let home = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join("flecha-launch-fixture");
        Environment {
            home: Some(home),
            ..Environment::default()
        }
    }

    fn resolve(args: &[&str], env: &Environment) -> Result<Launch> {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        Launch::resolve(&args, env)
    }

    fn browser(args: &[&str], env: &Environment) -> (PathBuf, Options) {
        match resolve(args, env).unwrap() {
            Launch::Browser { profile, options } => (profile, options),
            _ => panic!("expected a browser launch"),
        }
    }

    #[test]
    fn verify_requires_the_isolated_launcher() {
        let error = resolve(&["--verify"], &desktop_env()).err().unwrap();
        assert_eq!(error.to_string(), ISOLATED_LAUNCHER);
    }

    #[test]
    fn isolated_launcher_requires_exact_scratch_paths() {
        let mut env = Environment {
            isolated: true,
            ..desktop_env()
        };
        let error = resolve(&[], &env).err().unwrap();
        assert_eq!(error.to_string(), ISOLATED_LAUNCHER);

        env.home = Some("/mnt/scratch/home".into());
        env.runtime_dir = Some("/mnt/scratch/runtime".into());
        env.profile = Some("/mnt/scratch/profile/".into());
        let error = resolve(&[], &env).err().unwrap();
        assert_eq!(
            error.to_string(),
            "the launcher must supply a scratch profile"
        );

        env.profile = Some("/mnt/scratch/profile".into());
        let (profile, options) = browser(&["--verify"], &env);
        assert_eq!(profile, Path::new("/mnt/scratch/profile"));
        assert!(options.verify);
    }

    #[test]
    fn desktop_profile_uses_xdg_data_home_or_home() {
        let mut env = desktop_env();
        let home = env.home.clone().unwrap();
        let default = home.join(".local/share/flecha-rewrite");
        assert_eq!(browser(&[], &env).0, default);
        env.data_home = Some("relative".into());
        assert_eq!(browser(&[], &env).0, default);
        let data = home.join("data");
        env.data_home = Some(data.clone());
        assert_eq!(browser(&[], &env).0, data.join("flecha-rewrite"));
    }

    #[test]
    fn profile_flag_takes_precedence_and_both_url_forms_work() {
        let mut env = desktop_env();
        let home = env.home.clone().unwrap();
        let env_profile = home.join("env-profile");
        env.profile = Some(env_profile.clone());
        let url = "https://example.com/";
        let (profile, options) = browser(&[url], &env);
        assert_eq!(profile, env_profile);
        assert_eq!(options.initial.as_deref(), Some(url));
        assert!(!options.verify);

        let cli_profile = home.join("cli-profile");
        let (profile, options) = browser(
            &["--profile", cli_profile.to_str().unwrap(), "--open", url],
            &env,
        );
        assert_eq!(profile, cli_profile);
        assert_eq!(options.initial.as_deref(), Some(url));
    }

    #[test]
    fn desktop_profile_rejects_shipping_profile_and_descendants() {
        let env = desktop_env();
        let home = env.home.as_ref().unwrap();
        for path in [
            home.join(".config/flecha-browser"),
            home.join(".config/flecha-browser/nested"),
            home.join("missing/../.config/flecha-browser"),
        ] {
            let error = resolve(&["--profile", path.to_str().unwrap()], &env)
                .err()
                .unwrap();
            assert!(error.to_string().contains("outside the shipping profile"));
        }
        let sibling = home.join(".config/flecha-browser-rewrite");
        assert_eq!(
            browser(&["--profile", sibling.to_str().unwrap()], &env).0,
            sibling
        );
    }

    #[test]
    fn help_needs_no_desktop_or_cef_environment() {
        for flag in ["--help", "-h"] {
            assert!(matches!(
                resolve(&[flag], &Environment::default()).unwrap(),
                Launch::Help
            ));
        }
    }

    #[test]
    fn cef_child_dispatch_keeps_the_isolation_gate() {
        let mut env = desktop_env();
        let args = ["--type=renderer", "--lang=en-US"];
        assert!(matches!(resolve(&args, &env).unwrap(), Launch::CefChild));
        env.isolated = true;
        assert_eq!(
            resolve(&args, &env).err().unwrap().to_string(),
            ISOLATED_LAUNCHER
        );
    }
}
