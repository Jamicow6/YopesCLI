use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::{Child as ProcessChild, Command as ProcessCommand, Stdio},
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::*;

#[derive(Deserialize)]
struct RunCommandArgs {
    command: String,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

pub(super) fn run_command(arguments: &str) -> Result<String> {
    let args: RunCommandArgs = parse(arguments)?;
    let command = args.command.trim();
    if command.is_empty() {
        bail!("command cannot be empty");
    }
    let timeout = args
        .timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);

    let log_path = temporary_log_path();
    let file =
        fs::File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;
    let stderr = file
        .try_clone()
        .with_context(|| format!("opening {}", log_path.display()))?;

    let mut process = shell_process(command);
    process
        .current_dir(workspace())
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(stderr));

    let mut child = process
        .spawn()
        .with_context(|| format!("running `{command}`"))?;
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for the command")? {
            break Some(status);
        }
        if started.elapsed() >= Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };

    let output = read_log(&log_path);
    let _ = fs::remove_file(&log_path);
    let mut result = String::new();
    if timed_out {
        result.push_str(&format!("Command timed out after {timeout}s.\n"));
    } else if let Some(status) = status {
        if !status.success() {
            result.push_str(&format!("Exit code: {}.\n", status.code().unwrap_or(-1)));
        }
    }
    if output.is_empty() {
        result.push_str("(no output)");
    } else {
        result.push_str(&output);
    }
    Ok(truncate_output(result))
}

/// One background command: where it writes, when it started, and how to stop
/// it. `exit` is set the first time its output is read after finishing.
struct BashJob {
    started: Instant,
    timeout: u64,
    log_path: PathBuf,
    child: Option<ProcessChild>,
    exit: Option<i32>,
}

const MAX_BASH_JOBS: usize = 16;
static BASH_JOBS: OnceLock<Mutex<HashMap<u32, BashJob>>> = OnceLock::new();
static JOB_SEQ: AtomicU32 = AtomicU32::new(1);

fn bash_jobs() -> MutexGuard<'static, HashMap<u32, BashJob>> {
    BASH_JOBS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The cached bash executable. On Windows it arrives with Git for Windows;
/// without it the shell tools refuse to run and `run_command` stays on cmd.
static BASH_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

pub(super) fn bash_path() -> Option<&'static PathBuf> {
    BASH_PATH.get_or_init(detect_bash).as_ref()
}

fn detect_bash() -> Option<PathBuf> {
    if !cfg!(windows) {
        return Some(PathBuf::from("bash"));
    }
    let probe = ProcessCommand::new("where").arg("git").output().ok()?;
    if !probe.status.success() {
        return None;
    }
    let lossy = String::from_utf8_lossy(&probe.stdout);
    let git = lossy.lines().next()?.trim();
    // git.exe usually lives in …\cmd or …\bin; bash.exe in …\bin or …\usr\bin.
    let root = PathBuf::from(git).parent()?.to_path_buf();
    let step_up = root.parent().map(|root| root.join("bin").join("bash.exe"));
    vec![
        root.join("bash.exe"),
        root.join("bin").join("bash.exe"),
        root.join("usr").join("bin").join("bash.exe"),
    ]
    .into_iter()
    .chain(step_up)
    .find(|candidate| candidate.is_file())
}

/// The process wrapper every command runs through: git-bash when it is
/// installed, otherwise the platform default. That is the graceful disable.
fn shell_process(command: &str) -> ProcessCommand {
    if let Some(bash) = bash_path() {
        let mut process = ProcessCommand::new(bash);
        process.args(["-lc", command]);
        return process;
    }
    if cfg!(windows) {
        let mut process = ProcessCommand::new("cmd");
        process.args(["/C", command]);
        process
    } else {
        let mut process = ProcessCommand::new("sh");
        process.args(["-c", command]);
        process
    }
}

#[derive(Deserialize)]
struct BashBackgroundArgs {
    command: String,
    #[serde(default)]
    risk: bool,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Deserialize)]
struct BashJobArgs {
    job_id: u32,
}

pub(super) fn bash_background(arguments: &str) -> Result<String> {
    let Some(bash) = bash_path().cloned() else {
        bail!(
            "bash is not available on this machine; install Git for Windows and \
             restart yopes to run background jobs"
        );
    };
    let args: BashBackgroundArgs = parse(arguments)?;
    let command = args.command.trim();
    if command.is_empty() {
        bail!("command cannot be empty");
    }
    let timeout = args
        .timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);
    let mut jobs = bash_jobs();
    // Finished jobs forget themselves on read, but those the model
    // never reads again stay; drop them here so the cap cannot be
    // exhausted by finished work.
    jobs.retain(|_, job| job.child.is_some());
    if jobs.len() >= MAX_BASH_JOBS {
        bail!(
            "too many bash jobs are waiting or running ({}, the limit is \
             {MAX_BASH_JOBS}); read their output with bash_output first",
            jobs.len()
        );
    }
    let id = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    let log_path = temporary_log_path();
    let file =
        fs::File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;
    let stderr = file
        .try_clone()
        .with_context(|| format!("opening {}", log_path.display()))?;
    let mut process = ProcessCommand::new(bash);
    process.args(["-lc", command]);
    let child = process
        .current_dir(workspace())
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("running `{command}`"))?;
    jobs.insert(
        id,
        BashJob {
            started: Instant::now(),
            timeout,
            log_path,
            child: Some(child),
            exit: None,
        },
    );
    Ok(format!(
        "Started bash job {id} (timeout {timeout}s). Call bash_output with job id {id} \
         to read it, and bash_kill to stop it."
    ))
}

pub(super) fn bash_output(arguments: &str) -> Result<String> {
    let args: BashJobArgs = parse(arguments)?;
    let mut jobs = bash_jobs();
    let Some(job) = jobs.get_mut(&args.job_id) else {
        bail!("no bash job with id {}; it was never started or was already read", args.job_id);
    };
    // Ownership of the child moves through this block so the job can finish
    // between calls; on kill and exit the child is consumed and dropped.
    let mut held = job.child.take();
    if let Some(child) = held.as_mut() {
        match child.try_wait().context("waiting for the bash job")? {
            Some(status) => {
                job.exit = Some(status.code().unwrap_or(-1));
                held = None;
            }
            None if job.started.elapsed() >= Duration::from_secs(job.timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                job.exit = Some(-1);
                held = None;
            }
            None => {}
        }
    }
    if job.exit.is_none() {
        job.child = held;
    }
    let mut result = String::new();
    match job.exit {
        Some(exit) => result.push_str(&format!("Exit code: {exit}.\n")),
        None => result.push_str("Still running.\n"),
    }
    let output = read_log(&job.log_path);
    if output.is_empty() {
        if job.child.is_some() {
            result.push_str("(no output yet)");
        } else {
            result.push_str("(no output)");
        }
    } else {
        result.push_str(&output);
    }
    // A job whose child is gone and whose exit was read frees its slot.
    if job.exit.is_some() && job.child.is_none() {
        jobs.remove(&args.job_id);
    }
    Ok(truncate_output(result))
}

pub(super) fn bash_kill(arguments: &str) -> Result<String> {
    let args: BashJobArgs = parse(arguments)?;
    let mut jobs = bash_jobs();
    let Some(job) = jobs.get_mut(&args.job_id) else {
        bail!("no bash job with id {}; it was never started or was already read", args.job_id);
    };
    let Some(child) = job.child.as_mut() else {
        return Ok(format!("Bash job {} already finished.", args.job_id));
    };
    let _ = child.kill();
    let _ = child.wait();
    job.child = None;
    job.exit = Some(-1);
    Ok(format!("Killed bash job {}. It reported no exit code.", args.job_id))
}
