use anyhow::Error;
use std::io::{self, Write};

fn main() {
    let exit_code = match tlaunch::run() {
        Ok(exit_code) => exit_code,
        Err(error) => {
            report_error(&error);
            1
        }
    };
    std::process::exit(exit_code);
}

fn report_error(error: &Error) {
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "Error: {error:#}");
    let _ = stderr.flush();
}
