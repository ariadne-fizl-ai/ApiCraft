//! Starting child processes for command-backed variables and Git, with a
//! timeout, and without flashing a console window on Windows; or, for
//! commands that need the user to type something, in a terminal window.

use std::io::Read;
use std::path::{Path, PathBuf};
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
    if spec.window {
        return run_in_window(spec);
    }
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

/// The command line a spec stands for, for a shell script.
fn shell_line(spec: &CommandSpec) -> String {
    if !spec.shell.is_empty() {
        return spec.shell.clone();
    }
    let quote = |a: &str| {
        if !a.is_empty() && !a.contains([' ', '\t', '"', '\'', '&', '|', '<', '>', '^', '%', '$', ';', '(', ')']) {
            a.to_string()
        } else if cfg!(windows) {
            format!("\"{}\"", a.replace('"', "\"\""))
        } else {
            format!("'{}'", a.replace('\'', "'\\''"))
        }
    };
    std::iter::once(spec.program.as_str()).chain(spec.args.iter().map(String::as_str)).map(quote).collect::<Vec<_>>().join(" ")
}

/// Runs a spec in a terminal window of its own, so the user can answer its
/// prompts there. A script in a private temporary folder runs the command
/// with stdout sent to a file, then writes the exit status to another; both
/// are read back and the folder removed.
fn run_in_window(spec: &CommandSpec) -> Result<CommandOutput, String> {
    let what = shell_line(spec);
    let dir = std::env::temp_dir().join(format!("apicraft-{}", apicraft_core::ids::new_id()));
    std::fs::create_dir(&dir).map_err(|e| format!("could not run {what}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let timeout = Duration::from_millis(if spec.timeout_ms == 0 { 300_000 } else { spec.timeout_ms as u64 });
    let result = window_script(spec, &dir).and_then(|script| wait_for_window(spec, &script, &dir, timeout));
    let _ = std::fs::remove_dir_all(&dir);
    result.map_err(|e| format!("could not run {what}: {e}"))
}

fn window_script(spec: &CommandSpec, dir: &Path) -> Result<PathBuf, String> {
    let line = shell_line(spec);
    let (name, body) = if cfg!(windows) {
        // `call` so that .cmd shims (npm's `bw`) return to the script.
        let cd = if spec.cwd.is_empty() { String::new() } else { format!("cd /d \"{}\"\r\n", spec.cwd) };
        let body = format!(
            "@echo off\r\ntitle ApiCraft\r\n{cd}call {line} > \"%~dp0out.txt\"\r\nset code=%errorlevel%\r\n> \"%~dp0done.txt\" echo %code%\r\nif not \"%code%\"==\"0\" (echo. & echo Press any key to close this window. & pause > nul)\r\n"
        );
        ("run.cmd", body)
    } else {
        // Terminals started through a launcher don't always pass our
        // environment on, so it goes in the (private, short-lived) script.
        let q = |v: &str| format!("'{}'", v.replace('\'', "'\\''"));
        let env: String = spec.env.iter().map(|(k, v)| format!("export {k}={}\n", q(v))).collect();
        let cd = if spec.cwd.is_empty() { String::new() } else { format!("cd {}\n", q(&spec.cwd)) };
        let here = q(&dir.display().to_string());
        let body = format!(
            "#!/bin/sh\n{env}{cd}{line} > {here}/out.txt\ncode=$?\necho $code > {here}/done.txt\nif [ $code -ne 0 ]; then echo; echo 'Press Enter to close this window.'; read _; fi\n"
        );
        ("run.sh", body)
    };
    let script = dir.join(name);
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700));
    }
    Ok(script)
}

/// Opens the terminal running `script`, then waits for its status file.
fn wait_for_window(spec: &CommandSpec, script: &Path, dir: &Path, timeout: Duration) -> Result<CommandOutput, String> {
    let mut child = open_terminal(spec, script)?;
    let done = dir.join("done.txt");
    let started = Instant::now();
    let mut closed_at: Option<Instant> = None;
    let status = loop {
        if let Some(code) = std::fs::read_to_string(&done).ok().and_then(|t| t.trim().parse::<i32>().ok()) {
            break code;
        }
        // On Windows the launcher lasts as long as the window, so its end
        // means the window was closed; elsewhere it may hand off and exit at once.
        if closed_at.is_none() && child.try_wait().ok().flatten().is_some() {
            closed_at = Some(Instant::now());
        }
        if cfg!(windows) && closed_at.is_some_and(|t| t.elapsed() > Duration::from_millis(500)) {
            return Err("the window was closed".into());
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            return Err(format!("no answer after {} s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = std::fs::read_to_string(dir.join("out.txt")).unwrap_or_default();
    let stderr = if status == 0 { String::new() } else { "it failed; the window says why".to_string() };
    Ok(CommandOutput { status, stdout, stderr })
}

#[cfg(windows)]
fn open_terminal(spec: &CommandSpec, script: &Path) -> Result<std::process::Child, String> {
    use std::os::windows::process::CommandExt;
    // A hidden cmd runs `start /WAIT`, which opens a console window of its
    // own for the script and returns when that window closes.
    let mut c = command("cmd");
    c.raw_arg(format!("/D /C start \"ApiCraft\" /WAIT cmd /D /C \"{}\"", script.display()));
    for (k, v) in &spec.env {
        c.env(k, v);
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c.spawn().map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn open_terminal(_spec: &CommandSpec, script: &Path) -> Result<std::process::Child, String> {
    Command::new("open")
        .arg("-a")
        .arg("Terminal")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_terminal(_spec: &CommandSpec, script: &Path) -> Result<std::process::Child, String> {
    let script = script.display().to_string();
    let preferred = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty()).map(|t| (t, "-e"));
    let known = [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xfce4-terminal", "-x"),
        ("kitty", "--"),
        ("alacritty", "-e"),
        ("xterm", "-e"),
    ];
    let mut tried = Vec::new();
    for (term, flag) in preferred.into_iter().chain(known.iter().map(|(t, f)| (t.to_string(), *f))) {
        match Command::new(&term).arg(flag).arg("sh").arg(&script).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(child) => return Ok(child),
            Err(_) => tried.push(term),
        }
    }
    Err(format!("no terminal found (tried {}); set $TERMINAL", tried.join(", ")))
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

    #[test]
    fn quotes_program_lines() {
        let line = shell_line(&CommandSpec { program: "bw".into(), args: vec!["get".into(), "My item".into()], ..Default::default() });
        assert!(line.starts_with("bw get "), "{line}");
        assert!(line.ends_with(if cfg!(windows) { "\"My item\"" } else { "'My item'" }), "{line}");
    }

    /// Opens a real window, so only when asked: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn runs_in_a_window() {
        let out = run(&CommandSpec { shell: "echo from-window".into(), window: true, timeout_ms: 20_000, ..Default::default() }).unwrap();
        assert!(out.ok(), "{out:?}");
        assert_eq!(out.stdout.trim(), "from-window");
    }
}
