//! Starting child processes for command-backed variables and Git, with a
//! timeout, and without flashing a console window on Windows.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use apicraft_core::sources::{CommandOutput, CommandSpec};

/// A `Command` that won't open a console window from the GUI app.
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Runs a [`CommandSpec`] to completion and collects its output.
pub fn run(spec: &CommandSpec) -> Result<CommandOutput, String> {
    let mut cmd = if spec.shell.is_empty() {
        let program = resolve_program(&spec.program);
        let mut c = command(&program);
        c.args(&spec.args);
        c
    } else if cfg!(windows) {
        let mut c = command("cmd");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Passed as typed: cmd has its own quoting rules.
            c.arg("/D").arg("/S").arg("/C").raw_arg(format!("\"{}\"", spec.shell));
        }
        c
    } else {
        let mut c = command("sh");
        c.arg("-c").arg(&spec.shell);
        c
    };
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    if !spec.cwd.is_empty() {
        cmd.current_dir(&spec.cwd);
    }
    let timeout = Duration::from_millis(if spec.timeout_ms == 0 { 60_000 } else { spec.timeout_ms as u64 });
    run_command(cmd, timeout).map_err(|e| {
        let what = if spec.shell.is_empty() { spec.program.as_str() } else { spec.shell.as_str() };
        format!("could not run {what}: {e}")
    })
}

/// Runs a prepared command, giving up after `timeout`.
pub fn run_command(mut cmd: Command, timeout: Duration) -> Result<CommandOutput, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    // Read both pipes on threads so a chatty child can't fill one and stall.
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let mut err = child.stderr.take().ok_or("no stderr")?;
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(15)),
        }
    };
    let stdout = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
    Ok(CommandOutput { status: status.code().unwrap_or(-1), stdout, stderr })
}

/// On Windows, npm-installed tools such as `bw` are `.cmd` shims, which
/// `Command` won't find from the bare name; look for them on PATH.
fn resolve_program(program: &str) -> String {
    if !cfg!(windows) || program.contains(['/', '\\', '.']) {
        return program.to_string();
    }
    let Some(path) = std::env::var_os("PATH") else { return program.to_string() };
    for dir in std::env::split_paths(&path) {
        for ext in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{program}.{ext}"));
            if candidate.is_file() {
                return candidate.display().to_string();
            }
        }
    }
    program.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_shell_lines() {
        let out = run(&CommandSpec { shell: "echo hello".into(), ..Default::default() }).unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hello");
        let out = run(&CommandSpec { shell: "exit 3".into(), ..Default::default() }).unwrap();
        assert_eq!(out.status, 3);
    }
}
