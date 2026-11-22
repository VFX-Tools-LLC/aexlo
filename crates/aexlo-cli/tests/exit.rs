//! How the `aexlo` binary ends: it must not wait for a plugin's exit-time
//! handlers, and it must keep its exit code.
//!
//! `tests/hang_on_exit` is an ordinary effect whose DLL waits, in
//! `DLL_PROCESS_DETACH`, for a worker thread it started. `ExitProcess` kills
//! that thread before running the handler, so a process that ends by
//! returning from `main` hangs forever (Windows only; elsewhere a normal exit
//! leaves the thread alive and these tests pass either way). Every run below
//! has a hard wall-clock limit, after which the process tree is killed by pid
//! and the test fails.

mod common;

use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use common::*;

/// Far longer than any healthy run here takes, far shorter than a hang.
const LIMIT: Duration = Duration::from_secs(20);

struct Finished {
	/// `None` if the limit passed and the process had to be killed.
	status: Option<ExitStatus>,
	elapsed: Duration,
	stdout: String,
	stderr: String,
}

impl Finished {
	fn timed_out(&self) -> bool {
		self.status.is_none()
	}

	fn code(&self) -> Option<i32> {
		self.status.and_then(|s| s.code())
	}

	fn describe(&self) -> String {
		format!(
			"after {:?}: {:?}\nstdout:\n{}\nstderr:\n{}",
			self.elapsed, self.status, self.stdout, self.stderr
		)
	}
}

/// Kill `pid` and everything it started. A process stuck in a library's
/// detach handler can resist the first attempts, so try each in turn.
fn kill_tree(pid: u32, child: &mut std::process::Child) {
	if cfg!(windows) {
		let _ = Command::new("taskkill")
			.args(["/PID", &pid.to_string(), "/T", "/F"])
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.status();
	}
	let _ = child.kill();
	if cfg!(windows) {
		let _ = Command::new("powershell")
			.args([
				"-NoProfile",
				"-Command",
				&format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).Kill()"),
			])
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.status();
	}
	let _ = child.wait();
}

/// Run `aexlo <args>` in `cwd` with the extra environment `env`, for at most
/// `limit`.
fn aexlo_within(limit: Duration, args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Finished {
	let dir = scratch(&format!(
		"exit_run/{}-{:?}",
		std::process::id(),
		std::thread::current().id()
	));
	let (out_path, err_path) = (dir.join("stdout.txt"), dir.join("stderr.txt"));
	let mut command = Command::new(env!("CARGO_BIN_EXE_aexlo"));
	command
		.args(args)
		.current_dir(cwd)
		.stdin(Stdio::null())
		.stdout(std::fs::File::create(&out_path).unwrap())
		.stderr(std::fs::File::create(&err_path).unwrap());
	for (key, value) in env {
		command.env(key, value);
	}
	let started = Instant::now();
	let mut child = command.spawn().expect("spawning aexlo");
	let pid = child.id();
	let status = loop {
		if let Some(status) = child.try_wait().unwrap() {
			break Some(status);
		}
		if started.elapsed() > limit {
			kill_tree(pid, &mut child);
			break None;
		}
		std::thread::sleep(Duration::from_millis(50));
	};
	let read = |path: &Path| String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned();
	Finished {
		status,
		elapsed: started.elapsed(),
		stdout: read(&out_path),
		stderr: read(&err_path),
	}
}

/// A manifest running `plugin` once with no checks.
fn manifest_for(dir: &Path, plugin: &Path) -> std::path::PathBuf {
	manifest(
		dir,
		&format!(
			r#"
[plugin]
artifact = {}

[defaults]
size = [32, 24]
checks = []
golden = false

[[preset]]
name = "hang_on_exit"
"#,
			toml_path(plugin)
		),
	)
}

#[test]
fn compat_returns_for_a_plugin_that_hangs_at_process_exit() {
	let plugin = hang_on_exit_artifact();
	let run = aexlo_within(
		LIMIT,
		&[
			"compat",
			plugin.to_str().unwrap(),
			"--only",
			"load",
			"--format",
			"json",
			"--timeout",
			"5",
		],
		&workspace(),
		&[],
	);
	assert!(!run.timed_out(), "compat hung at exit {}", run.describe());
	assert_eq!(run.code(), Some(0), "{}", run.describe());
	let report: serde_json::Value =
		serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("{e}: {}", run.describe()));
	assert_eq!(report["plugins"][0]["load"]["status"], "ok", "{report:#}");
}

#[test]
fn test_returns_for_a_plugin_that_hangs_at_process_exit() {
	// `aexlo test` runs each variant in a `aexlo worker` process.
	let plugin = hang_on_exit_artifact();
	let dir = scratch("exit_hang_test");
	let manifest = manifest_for(&dir, &plugin);
	let run = aexlo_within(
		LIMIT,
		&["test", "--format", "json", "--manifest", manifest.to_str().unwrap()],
		&dir,
		&[],
	);
	assert!(!run.timed_out(), "test hung at exit {}", run.describe());
	assert_eq!(run.code(), Some(0), "{}", run.describe());
	let report: serde_json::Value =
		serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("{e}: {}", run.describe()));
	assert_eq!(
		outcomes(&report),
		[("hang_on_exit".to_string(), "pass".to_string())],
		"{report:#}"
	);
}

/// A plugin whose library hangs when the host unloads it (a destructor that
/// joins a thread of its own, as Lux's denoiser does) must not cost the result
/// of the run that started the thread, nor the run after it. The worker used to
/// unload the plugin before it answered, so the finished render was lost to a
/// timeout. Windows only: elsewhere `dlclose` runs no such deadlock.
#[test]
#[cfg(windows)]
fn test_keeps_the_results_of_a_plugin_that_hangs_when_unloaded() {
	let plugin = hang_on_unload_artifact();
	let dir = scratch("unload_hang_test");
	let manifest = manifest(
		&dir,
		&format!(
			r#"
[plugin]
artifact = {}

[defaults]
size = [32, 24]
checks = []
golden = false

[[preset]]
name = "first"

[[preset]]
name = "second"
"#,
			toml_path(&plugin)
		),
	);
	let run = aexlo_within(
		LIMIT,
		&["test", "--format", "json", "--manifest", manifest.to_str().unwrap()],
		&dir,
		&[],
	);
	assert!(!run.timed_out(), "test hung {}", run.describe());
	assert_eq!(run.code(), Some(0), "{}", run.describe());
	let report: serde_json::Value =
		serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("{e}: {}", run.describe()));
	assert_eq!(
		outcomes(&report),
		[
			("first".to_string(), "pass".to_string()),
			("second".to_string(), "pass".to_string())
		],
		"{report:#}"
	);
}

/// `AEXLO_CLEAN_EXIT=1` goes back to a returning `main`, which is how a
/// plugin's own teardown is debugged: here that is the hang again.
#[test]
#[cfg(windows)]
fn clean_exit_runs_the_plugins_exit_handlers() {
	let plugin = hang_on_exit_artifact();
	let run = aexlo_within(
		Duration::from_secs(8),
		&[
			"compat",
			plugin.to_str().unwrap(),
			"--only",
			"load",
			"--format",
			"json",
			"--timeout",
			"5",
		],
		&workspace(),
		&[("AEXLO_CLEAN_EXIT", "1")],
	);
	assert!(
		run.timed_out(),
		"AEXLO_CLEAN_EXIT=1 should leave the exit handlers running {}",
		run.describe()
	);
}

/// `AEXLO_CLEAN_EXIT=0` or unset is the default direct exit.
#[test]
fn clean_exit_zero_is_the_default() {
	let plugin = hang_on_exit_artifact();
	let run = aexlo_within(
		LIMIT,
		&[
			"compat",
			plugin.to_str().unwrap(),
			"--only",
			"load",
			"--format",
			"json",
			"--timeout",
			"5",
		],
		&workspace(),
		&[("AEXLO_CLEAN_EXIT", "0")],
	);
	assert!(!run.timed_out(), "{}", run.describe());
	assert_eq!(run.code(), Some(0), "{}", run.describe());
}

/// The toolkit's exit codes (`docs/toolkit.md` section 12) reach the caller,
/// with and without the direct exit.
#[test]
fn exit_codes_pass_through() {
	let plugin = misbehave_artifact();
	let dir = scratch("exit_codes");
	let ok = manifest_for(&dir, &plugin);

	let crashing = scratch("exit_codes_crash");
	let bad = manifest(
		&crashing,
		&format!(
			r#"
[plugin]
artifact = {}

[defaults]
size = [32, 24]
checks = []
golden = false

[[preset]]
name = "crashes"
params = {{ Mode = "Crash" }}
"#,
			toml_path(&plugin)
		),
	);

	let invalid = manifest(&scratch("exit_codes_invalid"), "this is = = not toml");
	let missing_crate = manifest(
		&scratch("exit_codes_harness"),
		"[plugin]\ncrate = 'no-such-crate'\n[[preset]]\nname = \"a\"\n",
	);

	for env in [&[][..], &[("AEXLO_CLEAN_EXIT", "1")][..]] {
		let code = |manifest: &Path| {
			let run = aexlo_within(
				LIMIT,
				&["test", "--manifest", manifest.to_str().unwrap()],
				manifest.parent().unwrap(),
				env,
			);
			assert!(!run.timed_out(), "{}", run.describe());
			(run.code(), run)
		};
		assert_eq!(
			code(&ok).0,
			Some(0),
			"all pass, env {env:?}: {}",
			code(&ok).1.describe()
		);
		assert_eq!(
			code(&bad).0,
			Some(1),
			"a crash is judged bad, env {env:?}: {}",
			code(&bad).1.describe()
		);
		assert_eq!(
			code(&invalid).0,
			Some(2),
			"invalid manifest, env {env:?}: {}",
			code(&invalid).1.describe()
		);
		assert_eq!(
			code(&missing_crate).0,
			Some(3),
			"harness error, env {env:?}: {}",
			code(&missing_crate).1.describe()
		);
	}
}
