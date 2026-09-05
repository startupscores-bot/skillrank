//! Fixture provider (pinned-commit clone, worktree isolation) and the isolated
//! post-run verifier.

use super::{FixtureProvider, Isolation, PreparedWorkspace, Verdict, Verifier};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Prepares a pinned-commit clone of the fixture repo into a fresh temp dir per
/// trial (worktree isolation). Docker-container isolation is a planned increment.
pub struct GitFixtureProvider {
    pub git_url: String,
    pub commit: String,
    tmp_root: std::sync::Mutex<Option<std::path::PathBuf>>,
    base_checkout: std::sync::Mutex<Option<std::path::PathBuf>>,
}

static TRIAL_COUNTER: AtomicU64 = AtomicU64::new(0);

impl GitFixtureProvider {
    pub fn new(git_url: &str, commit: &str) -> Self {
        GitFixtureProvider {
            git_url: git_url.to_string(),
            commit: commit.to_string(),
            tmp_root: std::sync::Mutex::new(None),
            base_checkout: std::sync::Mutex::new(None),
        }
    }

    fn ensure_base_checkout(&self) -> Result<std::path::PathBuf, String> {
        {
            let base = self.base_checkout.lock().unwrap();
            if let Some(p) = base.as_ref() {
                return Ok(p.clone());
            }
        }
        let root = std::env::temp_dir().join(format!(
            "skillrank-fixture-{}-{}",
            std::process::id(),
            TRIAL_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let checkout = root.join("base");

        if !is_safe_git_url(&self.git_url) {
            return Err(format!(
                "refusing to clone unsafe fixture URL {:?}: only https/ssh remotes are allowed \
                 (git's ext:: transport executes arbitrary commands)",
                self.git_url
            ));
        }

        // `-c protocol.*.allow=never` blocks git's command-executing transports
        // (ext::, and local file:// clones); `--` stops a `-`-leading URL from
        // being parsed as an option. The URL is suite-supplied, i.e. untrusted.
        let out = Command::new("git")
            .args([
                "-c",
                "protocol.ext.allow=never",
                "-c",
                "protocol.file.allow=never",
                "clone",
                "--no-checkout",
                "--",
            ])
            .arg(&self.git_url)
            .arg(&checkout)
            .output()
            .map_err(|e| format!("clone fixture {}: {e}", self.git_url))?;
        if !out.status.success() {
            return Err(format!(
                "clone fixture {}: {}",
                self.git_url,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        if !self.commit.trim().is_empty() {
            let co = Command::new("git")
                .args(["checkout", &self.commit])
                .current_dir(&checkout)
                .output()
                .map_err(|e| e.to_string())?;
            if !co.status.success() {
                return Err(format!(
                    "checkout fixture commit {}: {}",
                    self.commit,
                    String::from_utf8_lossy(&co.stderr).trim()
                ));
            }
        } else {
            let _ = Command::new("git")
                .args(["checkout", "HEAD"])
                .current_dir(&checkout)
                .output();
        }

        *self.tmp_root.lock().unwrap() = Some(root);
        *self.base_checkout.lock().unwrap() = Some(checkout.clone());
        Ok(checkout)
    }
}

impl FixtureProvider for GitFixtureProvider {
    fn isolation(&self) -> Isolation {
        Isolation::Worktree
    }

    fn prepare(&self, _task_id: &str) -> Result<PreparedWorkspace, String> {
        let base = self.ensure_base_checkout()?;
        let root = self
            .tmp_root
            .lock()
            .unwrap()
            .clone()
            .ok_or("fixture temp root missing")?;
        let work_dir = root.join(format!(
            "trial-{}",
            TRIAL_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
        copy_tree(&base, &work_dir).inspect_err(|_e| {
            let _ = std::fs::remove_dir_all(&work_dir);
        })?;
        Ok(PreparedWorkspace {
            path: work_dir.clone(),
            cleanup_root: Some(work_dir),
        })
    }
}

impl Drop for GitFixtureProvider {
    fn drop(&mut self) {
        if let Some(root) = self.tmp_root.lock().unwrap().as_ref() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// True when a suite-supplied fixture remote is safe to hand to `git clone`.
///
/// Fixture URLs come from the registry, so they are untrusted input. Git's
/// `ext::` transport executes arbitrary shell, `file://`/local paths can point
/// anywhere on disk, and a leading `-` would be parsed as a command-line
/// option. Only plain https/ssh remotes are accepted.
pub fn is_safe_git_url(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() || u.starts_with('-') || u.contains(char::is_whitespace) {
        return false;
    }
    u.starts_with("https://") || u.starts_with("ssh://") || u.starts_with("git@")
}

/// Whether a docker binary is on PATH.
pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Recursively copy src into an existing dst directory.
pub(crate) fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            copy_tree(&entry.path(), &target)?;
        } else if file_type.is_symlink() {
            let link_target = std::fs::read_link(entry.path()).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link_target, &target).map_err(|e| e.to_string())?;
            #[cfg(not(unix))]
            std::fs::copy(entry.path(), &target)
                .map(|_| ())
                .map_err(|e| e.to_string())?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Runs a per-task verifier command in a location OUTSIDE the workspace. The
/// verifier is materialized only inside `verify` (never in the workspace during
/// the agent run), enforcing verifier isolation structurally.
pub struct ScriptVerifier {
    /// Maps task_id -> shell command run with the workspace as $1. Exit 0 = pass.
    pub commands: HashMap<String, String>,
    /// Interpreter (default: "bash").
    pub shell: String,
}

impl ScriptVerifier {
    pub fn new(commands: HashMap<String, String>) -> Self {
        ScriptVerifier {
            commands,
            shell: "bash".to_string(),
        }
    }
}

impl Verifier for ScriptVerifier {
    fn verify(&self, working_dir: &Path, task_id: &str) -> Result<Verdict, String> {
        let command = match self.commands.get(task_id) {
            Some(c) if !c.trim().is_empty() => c,
            _ => {
                return Err(format!("no verifier for task {task_id}"));
            }
        };
        let shell = if self.shell.is_empty() {
            "bash"
        } else {
            &self.shell
        };
        // Materialize the verifier in a temp dir OUTSIDE the workspace.
        let verifier_dir = std::env::temp_dir().join(format!(
            "skillrank-verifier-{}-{}",
            std::process::id(),
            TRIAL_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&verifier_dir).map_err(|e| e.to_string())?;
        let script_path = verifier_dir.join("verify.sh");
        std::fs::write(&script_path, command).map_err(|e| e.to_string())?;

        let status = Command::new(shell)
            .arg(&script_path)
            .arg(working_dir)
            .current_dir(working_dir)
            .status();
        let _ = std::fs::remove_dir_all(&verifier_dir);

        match status {
            Ok(s) if s.success() => Ok(Verdict {
                pass: true,
                verifier_error: false,
            }),
            Ok(_) => Ok(Verdict {
                pass: false,
                verifier_error: false,
            }),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_safe_git_url;

    #[test]
    fn accepts_normal_remotes() {
        assert!(is_safe_git_url("https://github.com/owner/repo.git"));
        assert!(is_safe_git_url("ssh://git@github.com/owner/repo.git"));
        assert!(is_safe_git_url("git@github.com:owner/repo.git"));
    }

    #[test]
    fn rejects_command_executing_and_option_like_urls() {
        // git's ext:: transport runs arbitrary shell.
        assert!(!is_safe_git_url("ext::sh -c 'touch /tmp/pwned'"));
        assert!(!is_safe_git_url("file:///etc"));
        assert!(!is_safe_git_url("/etc/passwd"));
        // A leading '-' would be parsed by git as an option.
        assert!(!is_safe_git_url("--upload-pack=touch /tmp/pwned"));
        assert!(!is_safe_git_url("-x"));
        assert!(!is_safe_git_url(""));
        assert!(!is_safe_git_url("   "));
        assert!(!is_safe_git_url("https://example.com/a repo"));
    }
}
