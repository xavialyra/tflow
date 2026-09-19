use anyhow::{Context, Result, bail};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shebang {
    pub(crate) interpreter: String,
    pub(crate) args: Vec<String>,
}

pub(crate) fn parse_shebang(script: &str) -> Shebang {
    let first_line = script.lines().next().unwrap_or("").trim_end();
    if let Some(shebang_str) = first_line.strip_prefix("#!") {
        let trimmed = shebang_str.trim();
        if !trimmed.is_empty()
            && let Ok(words) = shell_words::split(trimmed)
            && let Some((interpreter, args)) = words.split_first()
        {
            return Shebang {
                interpreter: interpreter.clone(),
                args: args.to_vec(),
            };
        }
    }
    Shebang {
        interpreter: "/bin/sh".to_string(),
        args: Vec::new(),
    }
}

pub(crate) fn verify_interpreter(interpreter: &str) -> Result<PathBuf> {
    if interpreter.is_empty() {
        bail!("script interpreter must not be empty");
    }
    if interpreter.contains('/') {
        let path = PathBuf::from(interpreter);
        if is_executable_file(&path) {
            return Ok(path);
        }
        bail!(
            "script interpreter {:?} was not found or is not executable",
            interpreter
        );
    }
    if let Some(path_var) = env::var_os("PATH") {
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join(interpreter);
            if is_executable_file(&candidate) {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "script interpreter {:?} was not found or is not executable on $PATH",
        interpreter
    )
}

fn is_executable_file(path: &Path) -> bool {
    if let Ok(metadata) = fs::metadata(path)
        && metadata.is_file()
    {
        let mode = metadata.mode();
        return (mode & 0o111) != 0;
    }
    false
}

pub(crate) fn scripts_cache_dir() -> PathBuf {
    if let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime_dir).join("tlaunch/scripts");
        if path.exists() || fs::create_dir_all(&path).is_ok() {
            return path;
        }
    }
    if let Some(cache_home) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(cache_home).join("tlaunch/scripts");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/tlaunch/scripts");
    }
    env::temp_dir().join("tlaunch/scripts")
}

fn sanitize_identifier(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "script".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn format_attributed_script(
    workflow_id: &str,
    source_label: &str,
    script_body: &str,
) -> String {
    let attribution =
        format!("# [tlaunch] source: workflows/{workflow_id}.toml -> [{source_label}]");
    let mut lines = script_body.lines();
    if let Some(first_line) = lines.next()
        && first_line.starts_with("#!")
    {
        let mut result = String::new();
        result.push_str(first_line);
        result.push('\n');
        result.push_str(&attribution);
        result.push('\n');
        for line in lines {
            result.push_str(line);
            result.push('\n');
        }
        return result;
    }
    format!("#!/bin/sh\n{attribution}\n{script_body}\n")
}

pub(crate) fn materialize_inline_script(
    workflow_id: &str,
    source_label: &str,
    script_body: &str,
) -> Result<PathBuf> {
    let dir = scripts_cache_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("could not create scripts directory {}", dir.display()))?;

    let mut hasher = DefaultHasher::new();
    script_body.hash(&mut hasher);
    let hash = format!("{:016x}", hasher.finish());

    let clean_wf = sanitize_identifier(workflow_id);
    let clean_src = sanitize_identifier(source_label);
    let filename = format!("{clean_wf}_{clean_src}_{hash}");
    let dest_path = dir.join(&filename);

    if dest_path.is_file()
        && let Ok(meta) = fs::metadata(&dest_path)
        && meta.len() > 0
    {
        return Ok(dest_path);
    }

    let attributed = format_attributed_script(workflow_id, source_label, script_body);
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!("{filename}.{}.{seq}.tmp", std::process::id()));

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp_path)
        .with_context(|| format!("could not create temporary script {}", tmp_path.display()))?;

    file.write_all(attributed.as_bytes())
        .with_context(|| format!("could not write temporary script {}", tmp_path.display()))?;
    file.flush()
        .with_context(|| format!("could not flush temporary script {}", tmp_path.display()))?;
    drop(file);

    fs::rename(&tmp_path, &dest_path)
        .with_context(|| format!("could not commit temporary script {}", dest_path.display()))?;

    Ok(dest_path)
}

pub(crate) fn prepare_inline_script_command(
    workflow_id: &str,
    source_label: &str,
    script_body: &str,
    args: &[String],
) -> Result<Vec<String>> {
    let shebang = parse_shebang(script_body);
    let interpreter_path = verify_interpreter(&shebang.interpreter)?;
    let script_path = materialize_inline_script(workflow_id, source_label, script_body)?;
    let mut argv = Vec::new();
    argv.push(interpreter_path.to_string_lossy().into_owned());
    argv.extend(shebang.args);
    argv.push(script_path.to_string_lossy().into_owned());
    argv.extend(args.iter().cloned());
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shebang_multiline_env() {
        let script = "#!/usr/bin/env -S bash -euo pipefail\necho hello\n";
        let shebang = parse_shebang(script);
        assert_eq!(shebang.interpreter, "/usr/bin/env");
        assert_eq!(shebang.args, vec!["-S", "bash", "-euo", "pipefail"]);
    }

    #[test]
    fn test_parse_shebang_default() {
        let script = "echo hello\n";
        let shebang = parse_shebang(script);
        assert_eq!(shebang.interpreter, "/bin/sh");
        assert!(shebang.args.is_empty());
    }

    #[test]
    fn test_attribution_format() {
        let script = "#!/usr/bin/env bash\necho 123";
        let attributed = format_attributed_script("git", "views.main.commands.commit", script);
        let mut lines = attributed.lines();
        assert_eq!(lines.next(), Some("#!/usr/bin/env bash"));
        assert_eq!(
            lines.next(),
            Some("# [tlaunch] source: workflows/git.toml -> [views.main.commands.commit]")
        );
        assert_eq!(lines.next(), Some("echo 123"));
    }

    #[test]
    fn test_materialize_and_permissions() {
        let script = "#!/bin/sh\necho test\n";
        let path = materialize_inline_script("demo", "test_cmd", script).unwrap();
        assert!(path.is_file());
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("[tlaunch] source: workflows/demo.toml"));
    }
}
