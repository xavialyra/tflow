//! The shipped setup wizard (`distribution/init.toml`) is what writes a user's
//! suite manifest, settings and workflow tree. Two defects there were invisible
//! to the hand-written fixture suite and cost the user a working launcher:
//!
//! 1. the cache marker latched on the first copy, so re-running the wizard
//!    reinstalled the same stale workflows no matter how often it was rerun;
//! 2. the install merged into the target directory, so files the source had
//!    dropped (a dead `items.sh`, an old `items.py`) survived every reinstall.
//!
//! These tests drive the wizard's embedded scripts against a private copy of the
//! workflow source, so they never touch the shared fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod support;
use support::{binary_path, temporary_root};

fn wizard_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("distribution/init.toml")
}

fn wizard_script(command: &str) -> String {
    let text = fs::read_to_string(wizard_path()).expect("the shipped wizard must be readable");
    let parsed: toml::Value =
        toml::from_str(&text).expect("distribution/init.toml must be valid TOML");
    parsed["commands"][command]["handler"]["script"]
        .as_str()
        .unwrap_or_else(|| panic!("distribution/init.toml must declare {command} script"))
        .to_string()
}

/// A wizard run against a throwaway source tree, cache and config directory.
struct Sandbox {
    root: PathBuf,
    source: PathBuf,
    config: PathBuf,
}

impl Sandbox {
    /// `source` is a private copy of the workflow packages, so a test may edit
    /// it to prove the cache follows the source.
    fn new() -> Self {
        let root = temporary_root();
        let source = root.join("source");
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/workflows");
        copy_tree(&fixtures, &source);
        fs::create_dir_all(root.join("cache")).expect("could not create the cache root");
        fs::create_dir_all(root.join("config")).expect("could not create the config root");
        Self {
            config: root.join("config/tlaunch"),
            root,
            source,
        }
    }

    fn script_path(&self) -> PathBuf {
        let path = self.root.join("wizard-install.py");
        if !path.exists() {
            fs::write(&path, wizard_script("install")).expect("could not stage the wizard script");
        }
        path
    }

    /// Run the wizard's install handler and return the message it would print.
    fn install(&self, selected: &[&str]) -> String {
        let request = serde_json::json!({
            "version": 1,
            "entrypoint": "command",
            "context": { "parameters": { "selected": selected } },
        });
        let output = Command::new("python3")
            .arg(self.script_path())
            .env("TLAUNCH_BIN", binary_path())
            .env(
                "WORKFLOW_DIR",
                Path::new(env!("CARGO_MANIFEST_DIR")).join("distribution"),
            )
            .env("TLAUNCH_WORKFLOWS_BOOTSTRAP_DIR", &self.source)
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env_remove("TLAUNCH_SUITE")
            .env_remove("TLAUNCH_SETTINGS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .expect("piped stdin")
                    .write_all(request.to_string().as_bytes())?;
                child.wait_with_output()
            })
            .expect("python3 must be able to run the wizard script");
        assert!(
            output.status.success(),
            "wizard install failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let operation: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("the wizard emits one operation");
        operation["operation"]["argv"]
            .as_array()
            .and_then(|argv| argv.last())
            .and_then(|message| message.as_str())
            .expect("the install operation prints a summary")
            .to_string()
    }

    fn suite(&self) -> toml::Value {
        let text = fs::read_to_string(self.config.join("default.toml"))
            .expect("the wizard must write a suite manifest");
        toml::from_str(&text).expect("the wizard must write strict TOML")
    }

    fn installed(&self, package: &str) -> PathBuf {
        self.config.join("workflows").join(package)
    }

    fn installed_packages(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.config.join("workflows"))
            .expect("the wizard must write a workflow tree")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().join("workflow.toml").is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Every file below `root`, as (relative path, contents) pairs.
fn tree_contents(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("readable directory") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("path below root")
                    .to_string_lossy()
                    .into_owned();
                entries.push((relative, fs::read(&path).expect("readable file")));
            }
        }
    }
    entries.sort();
    entries
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("could not create the destination");
    for entry in fs::read_dir(from).expect("readable source") {
        let path = entry.expect("readable entry").path();
        if path.file_name().is_some_and(|name| name == "__pycache__") {
            continue;
        }
        let target = to.join(path.file_name().expect("named entry"));
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            fs::copy(&path, &target).expect("could not copy the file");
        }
    }
}

#[test]
fn shipped_wizard_compiles() {
    let output = Command::new(binary_path())
        .args(["-w"])
        .arg(wizard_path())
        .arg("--check")
        .output()
        .expect("could not run the launcher");
    assert!(
        output.status.success(),
        "distribution/init.toml must validate: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn wizard_install_writes_a_valid_suite_from_the_source() {
    let sandbox = Sandbox::new();
    let message = sandbox.install(&["apps", "sys"]);

    assert!(
        message.contains("Config Check:     ok"),
        "the wizard's own post-install check must pass:\n{message}"
    );
    assert!(
        message.contains(&sandbox.source.display().to_string()),
        "the summary must name the source it copied from:\n{message}"
    );

    let suite = sandbox.suite();
    let sources = suite["suite"]["entrypoint"]["query"]["sources"]
        .as_array()
        .expect("the wizard must configure the aggregated sources");
    assert_eq!(
        sources
            .iter()
            .filter_map(|s| s.as_str())
            .collect::<Vec<_>>(),
        vec!["apps:main", "sys:main"]
    );
    // Read from the core package instead of assuming the library's name: the
    // fixtures declare `core:default`, the shipped library `core:main`.
    assert_eq!(
        suite["suite"]["entrypoint"]["target"].as_str(),
        Some("core:default")
    );
    let mut mounted: Vec<&str> = suite["workflows"]
        .as_table()
        .expect("the wizard must mount workflows")
        .keys()
        .map(String::as_str)
        .collect();
    mounted.sort_unstable();
    assert_eq!(mounted, vec!["apps", "core", "sys"]);

    assert_eq!(sandbox.installed_packages(), vec!["apps", "core", "sys"]);
    for package in ["apps", "core", "sys"] {
        let installed = tree_contents(&sandbox.installed(package));
        assert!(
            !installed
                .iter()
                .any(|(path, _)| path.contains("__pycache__")),
            "installed {package} must not carry bytecode"
        );
        assert_eq!(
            installed,
            tree_contents(&sandbox.source.join(package)),
            "installed {package} must be the source tree"
        );
    }

    let settings = fs::read_to_string(sandbox.config.join("settings.toml"))
        .expect("the wizard must write settings");
    let settings: toml::Value = toml::from_str(&settings).expect("settings must be strict TOML");
    assert_eq!(
        settings["defaults"]["picker"]["left_prefix"].as_str(),
        Some("$route")
    );
    assert_eq!(
        settings["defaults"]["picker"]["left_prefix_backspace"].as_str(),
        Some("root")
    );
}

#[test]
fn wizard_install_replaces_stale_files_instead_of_merging() {
    let sandbox = Sandbox::new();
    sandbox.install(&["apps"]);
    let stale = sandbox
        .installed("core")
        .join("scripts/left_over_from_an_old_install.py");
    fs::write(&stale, "#!/bin/sh\necho stale\n").expect("could not plant the stale file");

    sandbox.install(&["apps"]);

    assert!(
        !stale.exists(),
        "a file the source no longer ships must not survive a reinstall"
    );
}

#[test]
fn wizard_install_refreshes_a_cache_built_from_an_older_source() {
    let sandbox = Sandbox::new();
    sandbox.install(&["apps"]);

    let added = sandbox.source.join("core/scripts/added_upstream.py");
    fs::write(&added, "# added after the first install\n").expect("could not extend the source");
    sandbox.install(&["apps"]);

    assert!(
        sandbox
            .installed("core")
            .join("scripts/added_upstream.py")
            .is_file(),
        "the cache must follow the source instead of latching on the first copy"
    );
}
