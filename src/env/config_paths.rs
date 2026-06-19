use crate::common::{BUILD_DIR, get_program_name};
use crate::{flog, flogf};
use fish_build_helper::workspace_root;
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
#[cfg(windows)]
use osfd_win::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// A struct of configuration directories, determined in main() that fish will optionally pass to
/// env_init.
pub struct ConfigPaths {
    pub sysconf: PathBuf,      // e.g., /usr/local/etc
    pub bin: Option<PathBuf>,  // e.g., /usr/local/bin
    pub data: Option<PathBuf>, // e.g., /usr/local/share
    pub man: Option<PathBuf>,  // e.g., /usr/local/share/fish/man
    pub doc: Option<PathBuf>,  // e.g., /usr/local/share/doc/fish
}

pub const PREFIX: &str = env!("PREFIX");
const SYSCONF_DIR: &str = env!("SYSCONFDIR");
const DATADIR: Option<&str> = option_env!("DATADIR");

impl ConfigPaths {
    pub fn new() -> Self {
        let exec_path = get_fish_path();
        flog!(
            config,
            match exec_path {
                FishPath::Absolute(path) => format!("executable path: {}", posix_display_exec(path)),
                FishPath::LookUpInPath => format!("executable path: {}", get_program_name()),
            }
        );
        let paths = Self::from_exec_path(exec_path);
        flogf!(
            config,
            "paths.sysconf: %s",
            posix_display(&paths.sysconf)
        );
        macro_rules! log_optional_path {
            ($field:ident) => {
                flogf!(
                    config,
                    "paths.%s: %s",
                    stringify!($field),
                    paths
                        .$field
                        .as_ref()
                        .map(|x| posix_display(x))
                        .unwrap_or("|not found|".to_string()),
                );
            };
        }
        log_optional_path!(bin);
        log_optional_path!(data);
        log_optional_path!(man);
        log_optional_path!(doc);
        paths
    }

    fn from_exec_path(unresolved_exec_path: &'static FishPath) -> Self {
        let default_layout = |exec_path_parent: Option<&Path>| {
            let data = DATADIR.map(|p| PathBuf::from(p).join("fish"));
            Self {
                sysconf: PathBuf::from(SYSCONF_DIR).join("fish"),
                bin: option_env!("BINDIR")
                    .map(PathBuf::from)
                    // N.B. the argument may be non-canonical here.
                    .or_else(|| exec_path_parent.map(|p| p.to_owned())),
                data: data.clone(),
                man: data.map(|data| data.join("man")),
                doc: option_env!("DOCDIR").map(PathBuf::from),
            }
        };

        let exec_path = {
            use FishPath::*;
            match unresolved_exec_path {
                Absolute(p) => {
                    let Ok(exec_path) = p.canonicalize() else {
                        flog!(
                            config,
                            format!(
                                "Failed to canonicalize executable path '{}'. Using default paths",
                                p.display()
                            )
                        );
                        return default_layout(p.parent());
                    };
                    // `Path::canonicalize` returns an extended-length (`\\?\`) verbatim path on
                    // Windows, but BUILD_DIR / CARGO_MANIFEST_DIR are stored as *plain* canonical
                    // paths (build.rs strips the prefix). Strip it here too so the
                    // `starts_with(BUILD_DIR)` build-tree detection below matches, instead of
                    // always falling through to the default (installed) paths.
                    #[cfg(windows)]
                    let exec_path = strip_verbatim_prefix(exec_path);
                    exec_path
                }
                LookUpInPath => {
                    flog!(
                        config,
                        "No absolute executable path available. Using default paths",
                    );
                    return default_layout(None);
                }
            }
        };

        let Some(exec_path_parent) = exec_path.parent() else {
            flog!(
                config,
                "Executable path reported to be the root directory?! Using default paths.",
            );
            return default_layout(None);
        };

        let workspace_root = workspace_root();
        // TODO(MSRV>=1.88): feature(let_chains)
        if cfg!(using_cmake) && exec_path_parent.ends_with("bin") && {
            let prefix = exec_path_parent.parent().unwrap();
            let data = prefix.join("share/fish");
            let sysconf = prefix.join("etc/fish");
            DATADIR.expect("cmake sets datadir").strip_prefix(PREFIX) == Some("/share") &&
            SYSCONF_DIR.strip_prefix(PREFIX) == Some("/etc") &&
            data.exists() && sysconf.exists()
            // Installations with prefix set to exactly the workspace root are not supported;
            // those will behave like non-installed builds inside the workspace.
            // Installing somewhere else inside the workspace is fine.
            && prefix != workspace_root
        } {
            flog!(config, "Running from relocatable tree");
            let prefix = exec_path_parent.parent().unwrap();
            let data = prefix.join("share/fish");
            Self {
                sysconf: prefix.join("etc/fish"),
                bin: Some(exec_path_parent.to_owned()),
                data: Some(data.clone()),
                man: Some(data.join("man")),
                doc: Some(prefix.join("share/doc/fish")),
            }
        } else if exec_path.starts_with(BUILD_DIR) {
            flog!(
                config,
                format!(
                    "Running out of build directory, using paths relative to $CARGO_MANIFEST_DIR ({})",
                    posix_display(&workspace_root)
                ),
            );
            let doc_join = |dir| {
                cfg!(using_cmake)
                    .then_some(Path::new(BUILD_DIR))
                    .map(|path| path.join("cargo").join("fish-docs").join(dir))
            };
            // If we're in Cargo's target directory or in CMake's build directory, use the source files.
            Self {
                sysconf: workspace_root.join("etc"),
                bin: Some(exec_path_parent.to_owned()),
                data: Some(workspace_root.join("share")),
                man: doc_join("man"),
                doc: doc_join("html"),
            }
        } else {
            flog!(
                config,
                "Not in a relocatable tree or build directory, using default paths"
            );
            default_layout(Some(exec_path_parent))
        }
    }
}

/// Strip Windows' extended-length (`\\?\`) verbatim prefix from a canonicalized path so it can be
/// compared, as a plain path, against the plain canonical BUILD_DIR / CARGO_MANIFEST_DIR baked in
/// at build time (see build.rs `canonicalize_plain`).
#[cfg(windows)]
fn strip_verbatim_prefix(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(stripped) => PathBuf::from(stripped),
        None => p,
    }
}

pub enum FishPath {
    Absolute(PathBuf),
    LookUpInPath,
}

/// Display a native filesystem path in fish's POSIX path domain (`/c/...`) for `config:`
/// diagnostics, so reported paths match the POSIX paths fish uses everywhere else. The stored
/// `PathBuf`s stay native (they feed `canonicalize`). On non-Windows this is the plain display.
#[cfg(windows)]
fn posix_display(path: &Path) -> String {
    posix_rt::pathconv::win_to_posix(path.as_os_str())
}
#[cfg(not(windows))]
fn posix_display(path: &Path) -> String {
    path.display().to_string()
}

/// Like [`posix_display`] but for the fish executable itself: the on-disk name is `fish.exe`,
/// yet fish's POSIX view of its own path omits the synthetic Windows `.exe` extension so
/// diagnostics read `.../fish`.
#[cfg(windows)]
fn posix_display_exec(path: &Path) -> String {
    let s = posix_rt::pathconv::win_to_posix(path.as_os_str());
    match s.get(s.len().saturating_sub(4)..) {
        Some(ext) if ext.eq_ignore_ascii_case(".exe") => s[..s.len() - 4].to_string(),
        _ => s,
    }
}
#[cfg(not(windows))]
fn posix_display_exec(path: &Path) -> String {
    path.display().to_string()
}

static FISH_PATH: LazyLock<FishPath> = LazyLock::new(compute_fish_path);

/// Get the absolute path to the fish executable itself
pub fn get_fish_path() -> &'static FishPath {
    &FISH_PATH
}

fn compute_fish_path() -> FishPath {
    use FishPath::*;
    let Ok(mut path) = std::env::current_exe() else {
        return LookUpInPath;
    };

    assert!(path.is_absolute());

    // When /proc/self/exe points to a file that was deleted (or overwritten on update!)
    // then linux adds a " (deleted)" suffix.
    // If that's not a valid path, let's remove that awkward suffix.
    if !path.exists() {
        if let (Some(filename), Some(parent)) = (path.file_name(), path.parent()) {
            if let Some(corrected_filename) = filename
                .as_bytes()
                .strip_suffix(b" (deleted)")
                .map(OsStr::from_bytes)
            {
                path = parent.join(corrected_filename);
            }
        }
    }

    Absolute(path)
}
