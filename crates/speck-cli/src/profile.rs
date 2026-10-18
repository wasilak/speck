//! Profiles: several independent Speck instances on one Mac, like
//! `colima -p <name>`.
//!
//! The `default` profile lives in `~/.speck` (unchanged, so existing installs
//! keep their data); `<name>` lives in `~/.speck/profiles/<name>`. Each
//! profile has its own launchd agent, docker context and socket. Downloaded
//! guest assets (kernel, initrd, the pristine rootfs) are shared through the
//! default home, so a new profile does not download them again.
//!
//! `SPECK_HOME` stays a raw override for tests: such an instance gets no
//! launchd agent and no docker context, and keeps its assets to itself.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Context as _;

pub const DEFAULT: &str = "default";
const LAUNCHD_LABEL: &str = "io.speck.vm";
const DOCKER_CONTEXT: &str = "speck";
/// File in the default home holding the profile set by `spk profile use`.
const CURRENT_FILE: &str = "current-profile";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Profile name; `default` for the default profile and for a raw
    /// `SPECK_HOME`.
    pub name: String,
    /// Directory with this instance's disks, config, logs and sockets.
    pub home: PathBuf,
    /// Directory the downloaded kernel and initrd are read from.
    pub assets: PathBuf,
    /// Set when the home came from `SPECK_HOME` instead of a profile name.
    pub raw_home: bool,
}

/// Where the selected profile name came from; shown in error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env,
    CurrentFile,
    Default,
}

impl Profile {
    /// The profile `name` under the user's home directory `user_home`.
    pub fn named(name: &str, user_home: &Path) -> anyhow::Result<Self> {
        validate_name(name)?;
        let base = default_home(user_home);
        let home = if name == DEFAULT {
            base.clone()
        } else {
            base.join("profiles").join(name)
        };
        Ok(Self {
            name: name.to_owned(),
            home,
            assets: base,
            raw_home: false,
        })
    }

    /// An instance in an explicit `SPECK_HOME` directory.
    pub fn raw(home: PathBuf) -> Self {
        Self {
            name: DEFAULT.to_owned(),
            assets: home.clone(),
            home,
            raw_home: true,
        }
    }

    pub fn is_default(&self) -> bool {
        self.name == DEFAULT
    }

    /// Whether this instance runs through launchd and owns a docker context.
    pub fn managed(&self) -> bool {
        !self.raw_home
    }

    pub fn launchd_label(&self) -> String {
        if self.is_default() {
            LAUNCHD_LABEL.to_owned()
        } else {
            format!("{LAUNCHD_LABEL}.{}", self.name)
        }
    }

    /// `~/Library/LaunchAgents/<label>.plist`.
    pub fn plist_path(&self) -> anyhow::Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", self.launchd_label())))
    }

    pub fn docker_context(&self) -> String {
        if self.is_default() {
            DOCKER_CONTEXT.to_owned()
        } else {
            format!("{DOCKER_CONTEXT}-{}", self.name)
        }
    }

    /// Text naming this instance in user-facing messages, e.g. ` "work"`.
    /// Empty for the default profile, so single-instance output is unchanged.
    pub fn display_suffix(&self) -> String {
        if self.is_default() {
            String::new()
        } else {
            format!(" (profile \"{}\")", self.name)
        }
    }
}

/// `[a-z0-9][a-z0-9-]{0,31}`.
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    let mut chars = name.chars();
    let valid = name.len() <= 32
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    anyhow::ensure!(
        valid,
        "invalid profile name `{name}`: use lowercase letters, digits and `-`, \
         starting with a letter or digit, at most 32 characters"
    );
    Ok(())
}

pub fn default_home(user_home: &Path) -> PathBuf {
    user_home.join(".speck")
}

fn user_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// Profile set by `spk profile use`, if any.
pub fn stored_current(user_home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(default_home(user_home).join(CURRENT_FILE)).ok()?;
    let name = text.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Store `name` as the current profile; `default` removes the file.
pub fn store_current(user_home: &Path, name: &str) -> anyhow::Result<()> {
    validate_name(name)?;
    let path = default_home(user_home).join(CURRENT_FILE);
    if name == DEFAULT {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("failed to remove {}", path.display()));
            }
            _ => return Ok(()),
        }
    }
    std::fs::create_dir_all(default_home(user_home))?;
    std::fs::write(&path, format!("{name}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Pick the profile: `-p` > `SPECK_PROFILE` > `spk profile use` > `default`.
/// `SPECK_HOME` is a raw override and cannot be combined with a profile.
pub fn resolve_with(
    flag: Option<&str>,
    env_profile: Option<&str>,
    env_home: Option<&str>,
    user_home: &Path,
) -> anyhow::Result<(Profile, Source)> {
    if let Some(home) = env_home.filter(|h| !h.is_empty()) {
        anyhow::ensure!(
            flag.is_none() && env_profile.is_none(),
            "SPECK_HOME and a profile (-p / SPECK_PROFILE) cannot be used together"
        );
        return Ok((Profile::raw(PathBuf::from(home)), Source::Default));
    }
    let (name, source) = if let Some(name) = flag {
        (name.to_owned(), Source::Flag)
    } else if let Some(name) = env_profile.filter(|n| !n.is_empty()) {
        (name.to_owned(), Source::Env)
    } else if let Some(name) = stored_current(user_home) {
        (name, Source::CurrentFile)
    } else {
        (DEFAULT.to_owned(), Source::Default)
    };
    let profile = Profile::named(&name, user_home).with_context(|| match source {
        Source::Env => "from SPECK_PROFILE".to_owned(),
        Source::CurrentFile => format!(
            "from {}; fix it with `spk profile use default`",
            default_home(user_home).join(CURRENT_FILE).display()
        ),
        _ => "from -p".to_owned(),
    })?;
    Ok((profile, source))
}

static SELECTED: OnceLock<(Profile, Source)> = OnceLock::new();

/// Resolve the profile for this process from the `-p` flag and the
/// environment, and remember it for [`current`].
pub fn select(flag: Option<&str>) -> anyhow::Result<&'static Profile> {
    let selected = resolve_with(
        flag,
        std::env::var("SPECK_PROFILE").ok().as_deref(),
        std::env::var("SPECK_HOME").ok().as_deref(),
        &user_home(),
    )?;
    Ok(&SELECTED.get_or_init(|| selected).0)
}

/// Where [`select`] took the profile name from.
pub fn selected_source() -> Option<Source> {
    SELECTED.get().map(|(_, source)| *source)
}

/// The profile this process acts on. Without an earlier [`select`] (unit
/// tests), resolves from the environment without remembering it.
pub fn current() -> Profile {
    if let Some((profile, _)) = SELECTED.get() {
        return profile.clone();
    }
    resolve_with(
        None,
        std::env::var("SPECK_PROFILE").ok().as_deref(),
        std::env::var("SPECK_HOME").ok().as_deref(),
        &user_home(),
    )
    .map(|(p, _)| p)
    .unwrap_or_else(|_| Profile::named(DEFAULT, &user_home()).expect("default is valid"))
}

/// All profiles: `default` first, then every directory in
/// `~/.speck/profiles/`, sorted by name.
pub fn all(user_home: &Path) -> Vec<Profile> {
    let mut names: Vec<String> = std::fs::read_dir(default_home(user_home).join("profiles"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| validate_name(n).is_ok() && n != DEFAULT)
        .collect();
    names.sort();
    std::iter::once(DEFAULT.to_owned())
        .chain(names)
        .filter_map(|n| Profile::named(&n, user_home).ok())
        .collect()
}

/// macOS limits a Unix socket path to 104 bytes including the NUL.
const SUN_PATH_MAX: usize = 103;

/// Fail early when `path` is too long to bind as a Unix socket.
pub fn check_socket_path(path: &Path) -> anyhow::Result<()> {
    let len = path.as_os_str().len();
    anyhow::ensure!(
        len <= SUN_PATH_MAX,
        "socket path {} is {len} bytes; macOS allows at most {SUN_PATH_MAX}. \
         Use a shorter SPECK_HOME or profile name",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "/Users/x";

    fn temp_user(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("speck-profile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn resolve(
        flag: Option<&str>,
        env: Option<&str>,
        home: Option<&str>,
    ) -> anyhow::Result<(Profile, Source)> {
        resolve_with(flag, env, home, Path::new(USER))
    }

    #[test]
    fn names_follow_the_colima_rules() {
        for ok in ["default", "work", "ci-42", "0", &"a".repeat(32)] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "Work", "a_b", "a.b", "a/b", &"a".repeat(33)] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn default_keeps_the_old_layout_and_names() {
        let p = Profile::named("default", Path::new(USER)).unwrap();
        assert_eq!(p.home, Path::new("/Users/x/.speck"));
        assert_eq!(p.assets, Path::new("/Users/x/.speck"));
        assert_eq!(p.launchd_label(), "io.speck.vm");
        assert_eq!(p.docker_context(), "speck");
        assert_eq!(p.display_suffix(), "");
        assert!(p.managed());
    }

    #[test]
    fn named_profiles_get_their_own_home_label_and_context() {
        let p = Profile::named("work", Path::new(USER)).unwrap();
        assert_eq!(p.home, Path::new("/Users/x/.speck/profiles/work"));
        assert_eq!(p.assets, Path::new("/Users/x/.speck"));
        assert_eq!(p.launchd_label(), "io.speck.vm.work");
        assert_eq!(p.docker_context(), "speck-work");
        assert_eq!(p.display_suffix(), " (profile \"work\")");
    }

    #[test]
    fn flag_beats_env_beats_default() {
        let (p, s) = resolve(Some("a"), Some("b"), None).unwrap();
        assert_eq!((p.name.as_str(), s), ("a", Source::Flag));
        let (p, s) = resolve(None, Some("b"), None).unwrap();
        assert_eq!((p.name.as_str(), s), ("b", Source::Env));
        let (p, s) = resolve(None, None, None).unwrap();
        assert_eq!((p.name.as_str(), s), ("default", Source::Default));
        let (p, _) = resolve(None, Some(""), None).unwrap();
        assert_eq!(p.name, "default");
    }

    #[test]
    fn stored_current_profile_sits_between_env_and_default() {
        let dir = temp_user("current");
        let user = dir.as_path();
        store_current(user, "work").unwrap();
        let (p, s) = resolve_with(None, None, None, user).unwrap();
        assert_eq!((p.name.as_str(), s), ("work", Source::CurrentFile));
        let (p, _) = resolve_with(None, Some("ci"), None, user).unwrap();
        assert_eq!(p.name, "ci");
        let (p, _) = resolve_with(Some("default"), None, None, user).unwrap();
        assert_eq!(p.name, "default");
        store_current(user, "default").unwrap();
        assert_eq!(stored_current(user), None);
        let (p, _) = resolve_with(None, None, None, user).unwrap();
        assert_eq!(p.name, "default");
    }

    #[test]
    fn speck_home_is_a_raw_override_that_excludes_profiles() {
        let (p, _) = resolve(None, None, Some("/tmp/spkt")).unwrap();
        assert!(p.raw_home && !p.managed());
        assert_eq!(p.home, Path::new("/tmp/spkt"));
        assert_eq!(p.assets, Path::new("/tmp/spkt"));
        assert!(resolve(Some("work"), None, Some("/tmp/spkt")).is_err());
        assert!(resolve(None, Some("work"), Some("/tmp/spkt")).is_err());
    }

    #[test]
    fn invalid_names_say_where_they_came_from() {
        let err = resolve(None, Some("Bad"), None).unwrap_err();
        assert!(format!("{err:#}").contains("SPECK_PROFILE"), "{err:#}");
    }

    #[test]
    fn all_lists_default_then_profile_dirs() {
        let dir = temp_user("all");
        let profiles = default_home(&dir).join("profiles");
        for name in ["work", "ci-1", "Not_Valid"] {
            std::fs::create_dir_all(profiles.join(name)).unwrap();
        }
        std::fs::write(profiles.join("stray-file"), "").unwrap();
        let names: Vec<String> = all(&dir).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["default", "ci-1", "work"]);
    }

    #[test]
    fn long_socket_paths_are_rejected() {
        assert!(check_socket_path(Path::new("/Users/x/.speck/speck.sock")).is_ok());
        let long = format!("/private/var/folders/{}/speck.sock", "x".repeat(90));
        assert!(check_socket_path(Path::new(&long)).is_err());
    }
}
