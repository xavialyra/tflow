mod support;

use std::process::Command;

use support::{binary_path, project_config};

#[test]
fn check_loads_the_project_configuration() {
    let output = Command::new(binary_path())
        .args(["--check", "--config"])
        .arg(project_config())
        .output()
        .expect("could not run tui-launcher --check");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("configuration is valid: {}\n", project_config().display())
    );
}
