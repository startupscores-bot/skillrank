//! The single SkillRank eval harness. Official baselines, audit re-runs, and
//! community runs all execute this code so their numbers are comparable. It runs
//! forced-mode paired trials (control = no skill, treatment = skill installed)
//! against a pinned fixture, with the verifier applied only after the agent exits
//! (verifier isolation), and produces an [`EvalBundle`].
//!
//! The orchestrator depends on traits ([`AgentRunner`], [`FixtureProvider`],
//! [`Verifier`]) so it is fully unit-testable with stubs; real implementations
//! live in [`agent`] and [`fixture`].

pub mod agent;
pub mod fixture;

use crate::types::{
    EnvironmentCell, EvalBundle, HarnessInfo, ReferenceEnv, ResolveResponse, Suite, TrialArm,
    TrialRecord,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Identifies this runner in every bundle (part of the environment cell so
/// official-vs-community results stay comparable).
pub const HARNESS_NAME: &str = "skillrank-runner";
pub const HARNESS_VERSION: &str = "0.1.0";

/// How the fixture workspace was prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Fixture + verifier isolated in container layers.
    Docker,
    /// Pinned-commit clone in a temp dir (no container). Results are Self-reported
    /// only (cannot guarantee verifier isolation as strongly as containers).
    Worktree,
}

impl Isolation {
    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::Docker => "docker",
            Isolation::Worktree => "worktree",
        }
    }
}

/// One agent invocation request.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub working_dir: PathBuf,
    pub instruction: String,
    pub model: String,
    /// Treatment arm installs the skill into the workspace surface.
    pub skill_installed: bool,
    pub skill_content: String,
    pub skill_slug: String,
    pub timeout_sec: u32,
}

/// Measured result of one agent invocation.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost_usd: Option<f64>,
    pub duration_ms: i64,
    pub turns: i64,
    pub trajectory_digest: String,
    pub agent_error: bool,
}

/// Deterministic scoring of one task after the agent run.
#[derive(Debug, Clone, Copy, Default)]
pub struct Verdict {
    pub pass: bool,
    pub verifier_error: bool,
}

/// Invokes the user's coding agent for one task in one workspace.
pub trait AgentRunner {
    /// Provider tag ("claude_code" | "codex").
    fn agent_name(&self) -> String;
    /// Reference-comparable version band.
    fn agent_version_band(&self) -> String;
    fn run_task(&self, spec: &RunSpec) -> Result<RunOutcome, String>;
}

/// A prepared per-trial workspace; the temp root is removed on drop.
pub struct PreparedWorkspace {
    pub path: PathBuf,
    pub cleanup_root: Option<PathBuf>,
}

impl Drop for PreparedWorkspace {
    fn drop(&mut self) {
        if let Some(root) = &self.cleanup_root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// Prepares an isolated workspace for one task arm.
pub trait FixtureProvider {
    fn prepare(&self, task_id: &str) -> Result<PreparedWorkspace, String>;
    fn isolation(&self) -> Isolation;
}

/// Applies the (isolated, post-run) verifier to a completed workspace.
pub trait Verifier {
    fn verify(&self, working_dir: &Path, task_id: &str) -> Result<Verdict, String>;
}

/// Parameterizes an eval run.
#[derive(Debug, Clone)]
pub struct Config {
    /// Trials per arm per task.
    pub trials: u32,
    /// Model id used and recorded.
    pub model: String,
    /// Optional directory where completed trial workspaces are copied before cleanup.
    pub artifacts_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
struct TrialPosition {
    arm: TrialArm,
    number: u32,
}

/// Paired per-task summary printed locally.
///
/// Pass rate answers "did the skill make the agent correct", which most skills do
/// not change — on a small suite the honest outcome is usually "both arms pass".
/// What a skill moves is effort, so the duration/turn/cost rollups are first-class
/// alongside tokens: a skill that spends more tokens to halve the turns and finish
/// sooner is an improvement, and `token_delta_pct` alone would call it a regression.
///
/// Additive wire type (shared with BuildBetter ZeroShot): the effort fields carry
/// `#[serde(default)]` so a report written by an older CLI still deserializes, and
/// they are appended so an older client keeps parsing a newer report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDelta {
    pub task_id: String,
    pub control_pass_rate: f64,
    pub treatment_pass_rate: f64,
    pub pass_rate_delta: f64,
    pub control_avg_tokens: f64,
    pub treatment_avg_tokens: f64,
    pub token_delta_pct: f64,
    /// Mean wall-clock per trial, averaged over every trial in the arm — including
    /// `agent_error`/`verifier_error` ones, like the token rollups above. A skill
    /// that makes the agent stall until the timeout has to show up here.
    #[serde(default)]
    pub control_avg_duration_ms: f64,
    #[serde(default)]
    pub treatment_avg_duration_ms: f64,
    /// Treatment vs control, percent. `0.0` when there is no control baseline to
    /// divide by, matching `token_delta_pct`.
    #[serde(default)]
    pub duration_delta_pct: f64,
    /// Mean agent turns (tool-call round trips) per trial, same denominator as
    /// `control_avg_duration_ms`.
    #[serde(default)]
    pub control_avg_turns: f64,
    #[serde(default)]
    pub treatment_avg_turns: f64,
    #[serde(default)]
    pub turn_delta_pct: f64,
    /// Mean USD over the trials that actually reported a cost, or `None` when none
    /// of them did (`codex` reports no cost at all; a timed-out or unparseable
    /// `claude` run reports none for that trial). A missing cost is never counted
    /// as `$0` — averaging it in would understate what the run cost. Read it with
    /// `control_cost_trials` to know how much of the arm it covers.
    #[serde(default)]
    pub control_avg_cost_usd: Option<f64>,
    #[serde(default)]
    pub treatment_avg_cost_usd: Option<f64>,
    /// Trials that reported a cost: the denominator of `control_avg_cost_usd`.
    /// Below `Report::trials_per_arm` means the arm's cost is a partial sample.
    #[serde(default)]
    pub control_cost_trials: i64,
    #[serde(default)]
    pub treatment_cost_trials: i64,
    /// `Some` only when both arms priced at least one trial and the control mean is
    /// above zero. A ratio against a missing or zero baseline is not a number worth
    /// reporting, so it stays `None` instead of collapsing to `0.0`.
    #[serde(default)]
    pub cost_delta_pct: Option<f64>,
}

/// Local paired analysis with a low-N caveat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub deltas: Vec<TaskDelta>,
    pub trials_per_arm: u32,
    pub low_n_caveat: bool,
    pub isolation: String,
    /// Trials across the whole run whose agent reported no cost. `0` means every
    /// cost number in `deltas` covers its full arm; anything higher means the cost
    /// rollups are a subset of the run and must be presented as one.
    #[serde(default)]
    pub cost_missing_trials: i64,
}

/// The produced bundle plus its conformance flag and local report.
pub struct EvalResult {
    pub bundle: EvalBundle,
    pub conforming: bool,
    pub report: Report,
}

/// Estimated token and USD range for a suite run (control + treatment arms).
pub fn estimate_cost(suite: &Suite, cfg: &Config) -> (i64, f64) {
    let trials = if cfg.trials == 0 { 3 } else { cfg.trials } as i64;
    let mut tokens = 0i64;
    let mut cost = 0.0f64;
    for task in &suite.tasks {
        tokens += task.est_tokens * trials * 2;
        cost += task.est_cost_usd * trials as f64 * 2.0;
    }
    (tokens, cost)
}

/// Canonicalize the run parameters that must match for two bundles to be the same
/// configuration (dedup key on ingest). Deterministic, independent of outcomes.
pub fn compute_config_hash(
    suite: &Suite,
    skill: &ResolveResponse,
    cfg: &Config,
    cell: &EnvironmentCell,
) -> String {
    let canonical = format!(
        "harness={HARNESS_NAME}/{HARNESS_VERSION}|suite={}@{}|skill={}@{}|trials={}|agent={}|band={}|model={}|os={}|isolation={}",
        suite.id, suite.version, skill.slug, skill.content_hash, cfg.trials,
        cell.agent, cell.agent_version_band, cell.model, cell.os, cell.isolation
    );
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// Execute forced-mode paired trials and build a bundle + local report.
pub fn run_eval(
    suite: &Suite,
    skill: &ResolveResponse,
    cfg: &Config,
    agent: &dyn AgentRunner,
    fixtures: &dyn FixtureProvider,
    verifier: &dyn Verifier,
) -> Result<EvalResult, String> {
    let trials = if cfg.trials == 0 { 3 } else { cfg.trials };
    if suite.tasks.is_empty() {
        return Err(format!("suite {} has no tasks", suite.id));
    }

    struct Acc {
        ctrl: ArmAcc,
        treat: ArmAcc,
    }
    let mut records: Vec<TrialRecord> = Vec::new();
    let mut by_task: Vec<(String, Acc)> = Vec::new();

    for task in &suite.tasks {
        let mut acc = Acc {
            ctrl: ArmAcc::default(),
            treat: ArmAcc::default(),
        };
        for arm in [TrialArm::Control, TrialArm::Treatment] {
            for i in 0..trials {
                let rec = run_one_trial(
                    task,
                    TrialPosition { arm, number: i + 1 },
                    skill,
                    cfg,
                    agent,
                    fixtures,
                    verifier,
                )
                .map_err(|e| {
                    format!("task {} arm {} trial {}: {e}", task.id, arm.as_str(), i + 1)
                })?;
                match arm {
                    TrialArm::Control => acc.ctrl.record(&rec),
                    TrialArm::Treatment => acc.treat.record(&rec),
                }
                records.push(rec);
            }
        }
        by_task.push((task.id.clone(), acc));
    }

    let cell = EnvironmentCell {
        agent: agent.agent_name(),
        agent_version_band: agent.agent_version_band(),
        model: cfg.model.clone(),
        os: std::env::consts::OS.to_string(),
        isolation: fixtures.isolation().as_str().to_string(),
    };
    let bundle = EvalBundle {
        bundle_version: 1,
        skill_slug: skill.slug.clone(),
        skill_content_hash: skill.content_hash.clone(),
        suite_id: suite.id.clone(),
        suite_version: suite.version.clone(),
        harness: HarnessInfo {
            name: HARNESS_NAME.into(),
            version: HARNESS_VERSION.into(),
        },
        environment_cell: cell.clone(),
        trials: records,
        config_hash: compute_config_hash(suite, skill, cfg, &cell),
        created_at: String::new(),
    };

    let mut deltas = Vec::new();
    let mut cost_missing_trials = 0i64;
    for (task_id, a) in &by_task {
        let control_pass_rate = rate(a.ctrl.pass, a.ctrl.total);
        let treatment_pass_rate = rate(a.treat.pass, a.treat.total);
        let control_avg_tokens = a.ctrl.avg_tokens();
        let treatment_avg_tokens = a.treat.avg_tokens();
        let control_avg_duration_ms = a.ctrl.avg_duration_ms();
        let treatment_avg_duration_ms = a.treat.avg_duration_ms();
        let control_avg_turns = a.ctrl.avg_turns();
        let treatment_avg_turns = a.treat.avg_turns();
        let control_avg_cost_usd = a.ctrl.avg_cost_usd();
        let treatment_avg_cost_usd = a.treat.avg_cost_usd();
        cost_missing_trials += a.ctrl.unpriced_trials() + a.treat.unpriced_trials();
        deltas.push(TaskDelta {
            task_id: task_id.clone(),
            control_pass_rate,
            treatment_pass_rate,
            pass_rate_delta: treatment_pass_rate - control_pass_rate,
            control_avg_tokens,
            treatment_avg_tokens,
            token_delta_pct: delta_pct(control_avg_tokens, treatment_avg_tokens),
            control_avg_duration_ms,
            treatment_avg_duration_ms,
            duration_delta_pct: delta_pct(control_avg_duration_ms, treatment_avg_duration_ms),
            control_avg_turns,
            treatment_avg_turns,
            turn_delta_pct: delta_pct(control_avg_turns, treatment_avg_turns),
            control_avg_cost_usd,
            treatment_avg_cost_usd,
            control_cost_trials: a.ctrl.cost_trials as i64,
            treatment_cost_trials: a.treat.cost_trials as i64,
            cost_delta_pct: cost_delta_pct(control_avg_cost_usd, treatment_avg_cost_usd),
        });
    }
    deltas.sort_by(|a, b| a.task_id.cmp(&b.task_id));

    let report = Report {
        deltas,
        trials_per_arm: trials,
        low_n_caveat: trials < 5,
        isolation: fixtures.isolation().as_str().to_string(),
        cost_missing_trials,
    };
    let conforming = is_conforming(&suite.reference_env, &cell);
    Ok(EvalResult {
        bundle,
        conforming,
        report,
    })
}

fn run_one_trial(
    task: &crate::types::SuiteTask,
    position: TrialPosition,
    skill: &ResolveResponse,
    cfg: &Config,
    agent: &dyn AgentRunner,
    fixtures: &dyn FixtureProvider,
    verifier: &dyn Verifier,
) -> Result<TrialRecord, String> {
    let arm = position.arm;
    // The workspace lives until the end of this function, so the verifier can run
    // against it after the agent exits.
    let workspace = fixtures.prepare(&task.id)?;
    let spec = RunSpec {
        working_dir: workspace.path.clone(),
        instruction: task.instruction.clone(),
        model: cfg.model.clone(),
        skill_installed: arm == TrialArm::Treatment,
        skill_content: skill.inline_content.clone(),
        skill_slug: skill.slug.clone(),
        timeout_sec: task.timeout_sec.max(0) as u32,
    };
    let outcome = agent.run_task(&spec)?;

    let mut rec = TrialRecord {
        task_id: task.id.clone(),
        arm,
        verdict: String::new(),
        input_tokens: outcome.input_tokens,
        output_tokens: outcome.output_tokens,
        cache_read_tokens: outcome.cache_read,
        cache_write_tokens: outcome.cache_write,
        cost_usd: outcome.cost_usd,
        duration_ms: outcome.duration_ms,
        turns: outcome.turns,
        trajectory_digest: outcome.trajectory_digest,
    };
    if outcome.agent_error {
        rec.verdict = "agent_error".into();
    } else {
        // Verifier isolation: only now, after the agent process has exited, do we
        // apply the verifier.
        match verifier.verify(&workspace.path, &task.id) {
            Err(_) => rec.verdict = "verifier_error".into(),
            Ok(v) if v.verifier_error => rec.verdict = "verifier_error".into(),
            Ok(v) if v.pass => rec.verdict = "pass".into(),
            Ok(_) => rec.verdict = "fail".into(),
        }
    }
    if let Some(root) = &cfg.artifacts_dir {
        let destination = root
            .join(&task.id)
            .join(arm.as_str())
            .join(format!("trial-{}", position.number));
        if destination.exists() {
            return Err(format!(
                "artifact destination already exists: {}",
                destination.display()
            ));
        }
        std::fs::create_dir_all(&destination)
            .map_err(|e| format!("create artifact directory: {e}"))?;
        fixture::copy_tree(&workspace.path, &destination)
            .map_err(|e| format!("preserve trial workspace: {e}"))?;
    }
    Ok(rec)
}

fn is_conforming(reference: &ReferenceEnv, cell: &EnvironmentCell) -> bool {
    if cell.isolation != Isolation::Docker.as_str() {
        return false; // non-Docker runs are Self-reported only
    }
    if !reference.agent_version_band.is_empty()
        && reference.agent_version_band != cell.agent_version_band
    {
        return false;
    }
    if !reference.models.is_empty() && !reference.models.contains(&cell.model) {
        return false;
    }
    true
}

/// Running totals for one arm of one task. Every trial is counted, whatever its
/// verdict — an arm that errors or times out is part of that arm's effort.
///
/// Cost is the one exception, and deliberately so: `cost_usd` is optional per trial,
/// so it keeps its own denominator (`cost_trials`) instead of dividing a partial sum
/// by the full trial count, which would report a run as cheaper than it was.
#[derive(Debug, Clone, Default)]
struct ArmAcc {
    pass: u32,
    total: u32,
    tokens: f64,
    duration_ms: f64,
    turns: f64,
    cost_usd: f64,
    cost_trials: u32,
}

impl ArmAcc {
    fn record(&mut self, rec: &TrialRecord) {
        self.total += 1;
        self.tokens += (rec.input_tokens + rec.output_tokens) as f64;
        self.duration_ms += rec.duration_ms as f64;
        self.turns += rec.turns as f64;
        if let Some(cost) = rec.cost_usd {
            self.cost_usd += cost;
            self.cost_trials += 1;
        }
        if rec.verdict == "pass" {
            self.pass += 1;
        }
    }

    fn avg_tokens(&self) -> f64 {
        avg(self.tokens, self.total)
    }

    fn avg_duration_ms(&self) -> f64 {
        avg(self.duration_ms, self.total)
    }

    fn avg_turns(&self) -> f64 {
        avg(self.turns, self.total)
    }

    /// Mean over priced trials only; `None` when the agent priced none of them.
    fn avg_cost_usd(&self) -> Option<f64> {
        if self.cost_trials == 0 {
            None
        } else {
            Some(self.cost_usd / self.cost_trials as f64)
        }
    }

    fn unpriced_trials(&self) -> i64 {
        self.total.saturating_sub(self.cost_trials) as i64
    }
}

fn rate(pass: u32, total: u32) -> f64 {
    if total == 0 {
        0.0
    } else {
        pass as f64 / total as f64
    }
}

fn avg(sum: f64, total: u32) -> f64 {
    if total == 0 {
        0.0
    } else {
        sum / total as f64
    }
}

/// Treatment vs control as a percentage. `0.0` with no baseline to divide by, which
/// is the behaviour `token_delta_pct` has always had.
fn delta_pct(control: f64, treatment: f64) -> f64 {
    if control > 0.0 {
        (treatment - control) / control * 100.0
    } else {
        0.0
    }
}

/// Cost is only comparable when both arms priced something and the control mean is
/// non-zero; otherwise there is no delta to report, and `None` says so.
fn cost_delta_pct(control: Option<f64>, treatment: Option<f64>) -> Option<f64> {
    match (control, treatment) {
        (Some(c), Some(t)) if c > 0.0 => Some((t - c) / c * 100.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ResolveResponse, ScanTier, Suite, SuiteFixture, SuiteTask};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    struct StubFixture;
    impl FixtureProvider for StubFixture {
        fn isolation(&self) -> Isolation {
            Isolation::Worktree
        }
        fn prepare(&self, _task_id: &str) -> Result<PreparedWorkspace, String> {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "skillrank-stubfx-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("README.md"), "fixture").map_err(|e| e.to_string())?;
            Ok(PreparedWorkspace {
                path: dir.clone(),
                cleanup_root: Some(dir),
            })
        }
    }

    struct StubAgent {
        saw_verifier: AtomicBool,
    }
    impl AgentRunner for StubAgent {
        fn agent_name(&self) -> String {
            "claude_code".into()
        }
        fn agent_version_band(&self) -> String {
            "2.1".into()
        }
        fn run_task(&self, spec: &RunSpec) -> Result<RunOutcome, String> {
            // Verifier isolation check: no verify.sh in the workspace at run time.
            if spec.working_dir.join("verify.sh").exists() {
                self.saw_verifier.store(true, Ordering::SeqCst);
            }
            let mut tokens = 1000;
            if spec.skill_installed {
                std::fs::write(spec.working_dir.join("solution.txt"), "done").ok();
                tokens = 700;
            }
            Ok(RunOutcome {
                input_tokens: tokens,
                output_tokens: 100,
                turns: 2,
                duration_ms: 10,
                ..Default::default()
            })
        }
    }

    /// Agent whose outcome is chosen per (arm, trial index), so a test can script an
    /// asymmetric or partially-priced run instead of a single fixed outcome.
    struct ScriptedAgent<F: Fn(bool, u32) -> RunOutcome> {
        outcome: F,
        control_calls: AtomicU32,
        treatment_calls: AtomicU32,
    }

    impl<F: Fn(bool, u32) -> RunOutcome> ScriptedAgent<F> {
        fn new(outcome: F) -> Self {
            Self {
                outcome,
                control_calls: AtomicU32::new(0),
                treatment_calls: AtomicU32::new(0),
            }
        }
    }

    impl<F: Fn(bool, u32) -> RunOutcome> AgentRunner for ScriptedAgent<F> {
        fn agent_name(&self) -> String {
            "claude_code".into()
        }
        fn agent_version_band(&self) -> String {
            "2.1".into()
        }
        fn run_task(&self, spec: &RunSpec) -> Result<RunOutcome, String> {
            let counter = if spec.skill_installed {
                &self.treatment_calls
            } else {
                &self.control_calls
            };
            let trial = counter.fetch_add(1, Ordering::SeqCst);
            if spec.skill_installed {
                std::fs::write(spec.working_dir.join("solution.txt"), "done").ok();
            }
            Ok((self.outcome)(spec.skill_installed, trial))
        }
    }

    /// One task, so the scripted trial index is unambiguous.
    fn one_task_suite() -> Suite {
        Suite {
            id: "launch/playwright".into(),
            version: "1".into(),
            tasks: vec![SuiteTask {
                id: "task-a".into(),
                instruction: "do a".into(),
                timeout_sec: 60,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn one_task_verifier() -> fixture::ScriptVerifier {
        let mut commands = HashMap::new();
        commands.insert(
            "task-a".to_string(),
            "test -f \"$1/solution.txt\"".to_string(),
        );
        fixture::ScriptVerifier::new(commands)
    }

    fn run_scripted<F: Fn(bool, u32) -> RunOutcome>(trials: u32, outcome: F) -> Report {
        let agent = ScriptedAgent::new(outcome);
        run_eval(
            &one_task_suite(),
            &demo_skill(),
            &Config {
                trials,
                model: "sonnet".into(),
                artifacts_dir: None,
            },
            &agent,
            &StubFixture,
            &one_task_verifier(),
        )
        .unwrap()
        .report
    }

    fn demo_suite() -> Suite {
        Suite {
            id: "launch/playwright".into(),
            version: "1".into(),
            fixture: SuiteFixture {
                git_url: "https://example/repo".into(),
                commit: "abc".into(),
                image: String::new(),
            },
            tasks: vec![
                SuiteTask {
                    id: "task-a".into(),
                    instruction: "do a".into(),
                    timeout_sec: 60,
                    est_tokens: 1000,
                    est_cost_usd: 0.01,
                    ..Default::default()
                },
                SuiteTask {
                    id: "task-b".into(),
                    instruction: "do b".into(),
                    timeout_sec: 60,
                    est_tokens: 1000,
                    est_cost_usd: 0.01,
                    ..Default::default()
                },
            ],
            reference_env: crate::types::ReferenceEnv {
                agent_version_band: "2.1".into(),
                models: vec!["sonnet".into()],
            },
        }
    }

    fn demo_skill() -> ResolveResponse {
        ResolveResponse {
            slug: "owner/skill".into(),
            content_hash: "sha256:aaa".into(),
            inline_content: "---\nname: skill\n---\nbody".into(),
            scan_tier: ScanTier::Safe,
            ..Default::default()
        }
    }

    #[test]
    fn arms_verifier_isolation_and_bundle() {
        let suite = demo_suite();
        let skill = demo_skill();
        let agent = StubAgent {
            saw_verifier: AtomicBool::new(false),
        };
        let fixtures = StubFixture;
        let mut commands = HashMap::new();
        commands.insert(
            "task-a".to_string(),
            "test -f \"$1/solution.txt\"".to_string(),
        );
        commands.insert(
            "task-b".to_string(),
            "test -f \"$1/solution.txt\"".to_string(),
        );
        let verifier = fixture::ScriptVerifier::new(commands);

        let cfg = Config {
            trials: 3,
            model: "sonnet".into(),
            artifacts_dir: None,
        };
        let result = run_eval(&suite, &skill, &cfg, &agent, &fixtures, &verifier).unwrap();

        assert_eq!(
            result.bundle.trials.len(),
            12,
            "2 tasks × 3 trials × 2 arms"
        );
        assert!(
            !agent.saw_verifier.load(Ordering::SeqCst),
            "verifier leaked into agent workspace"
        );
        for d in &result.report.deltas {
            assert_eq!(d.control_pass_rate, 0.0, "control should fail");
            assert_eq!(d.treatment_pass_rate, 1.0, "treatment should pass");
            assert!(d.token_delta_pct < 0.0, "treatment cheaper");
            assert_eq!(d.control_avg_turns, 2.0, "turns rolled up from the trials");
            assert_eq!(d.treatment_avg_turns, 2.0);
            assert_eq!(d.turn_delta_pct, 0.0);
            assert_eq!(d.control_avg_duration_ms, 10.0);
            assert_eq!(d.control_avg_cost_usd, None, "stub agent prices nothing");
            assert_eq!(d.control_cost_trials, 0);
            assert_eq!(d.cost_delta_pct, None);
        }
        assert_eq!(
            result.report.cost_missing_trials, 12,
            "every trial was unpriced"
        );
        assert!(result.report.low_n_caveat, "3 trials < 5");
        assert!(!result.conforming, "worktree isolation is not conforming");
        assert_eq!(result.bundle.environment_cell.isolation, "worktree");
        assert_eq!(result.bundle.harness.name, HARNESS_NAME);
    }

    #[test]
    fn preserves_each_trial_workspace_when_requested() {
        let root =
            std::env::temp_dir().join(format!("skillrank-artifact-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = Config {
            trials: 1,
            model: "sonnet".into(),
            artifacts_dir: Some(root.clone()),
        };
        let result = run_eval(
            &one_task_suite(),
            &demo_skill(),
            &cfg,
            &StubAgent {
                saw_verifier: AtomicBool::new(false),
            },
            &StubFixture,
            &one_task_verifier(),
        );
        assert!(result.is_ok());
        assert!(root.join("task-a/control/trial-1/README.md").is_file());
        assert!(root.join("task-a/treatment/trial-1/solution.txt").is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn config_hash_deterministic_and_sensitive() {
        let suite = Suite {
            id: "s".into(),
            version: "1".into(),
            ..Default::default()
        };
        let skill = ResolveResponse {
            slug: "sk".into(),
            content_hash: "h".into(),
            ..Default::default()
        };
        let cell = EnvironmentCell {
            agent: "claude_code".into(),
            agent_version_band: "2.1".into(),
            model: "sonnet".into(),
            os: "macos".into(),
            isolation: "docker".into(),
        };
        let h1 = compute_config_hash(
            &suite,
            &skill,
            &Config {
                trials: 3,
                model: "sonnet".into(),
                artifacts_dir: None,
            },
            &cell,
        );
        let h2 = compute_config_hash(
            &suite,
            &skill,
            &Config {
                trials: 3,
                model: "sonnet".into(),
                artifacts_dir: None,
            },
            &cell,
        );
        assert_eq!(h1, h2, "deterministic");
        let mut cell_opus = cell.clone();
        cell_opus.model = "opus".into();
        let h3 = compute_config_hash(
            &suite,
            &skill,
            &Config {
                trials: 3,
                model: "opus".into(),
                artifacts_dir: None,
            },
            &cell_opus,
        );
        assert_ne!(h1, h3, "changes with model");
        let h4 = compute_config_hash(
            &suite,
            &skill,
            &Config {
                trials: 5,
                model: "sonnet".into(),
                artifacts_dir: None,
            },
            &cell,
        );
        assert_ne!(h1, h4, "changes with trials");
    }

    #[test]
    fn estimate_cost_works() {
        let suite = Suite {
            tasks: vec![
                SuiteTask {
                    est_tokens: 1000,
                    est_cost_usd: 0.02,
                    ..Default::default()
                },
                SuiteTask {
                    est_tokens: 2000,
                    est_cost_usd: 0.04,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (tokens, cost) = estimate_cost(
            &suite,
            &Config {
                trials: 3,
                model: String::new(),
                artifacts_dir: None,
            },
        );
        assert_eq!(tokens, 18000);
        assert!((cost - 0.36).abs() < 0.001);
    }

    // ---- effort rollups ----

    /// The case the summary used to hide: the skill spends MORE tokens and is still
    /// clearly better, because it halves the turns and the wall-clock and costs less.
    #[test]
    fn effort_rolls_up_per_arm_and_survives_a_token_regression() {
        let report = run_scripted(2, |treatment, trial| {
            if treatment {
                RunOutcome {
                    input_tokens: 1400,
                    output_tokens: 400,
                    turns: 3,
                    duration_ms: 15_000,
                    cost_usd: Some(0.12),
                    ..Default::default()
                }
            } else {
                // 40s/8 turns then 20s/4 turns → means of 30s and 6 turns.
                RunOutcome {
                    input_tokens: 1000,
                    output_tokens: 200,
                    turns: if trial == 0 { 8 } else { 4 },
                    duration_ms: if trial == 0 { 40_000 } else { 20_000 },
                    cost_usd: Some(0.20),
                    ..Default::default()
                }
            }
        });
        let d = &report.deltas[0];

        assert_eq!(d.control_avg_tokens, 1200.0);
        assert_eq!(d.treatment_avg_tokens, 1800.0);
        assert_eq!(d.token_delta_pct, 50.0, "tokens got worse");

        assert_eq!(d.control_avg_duration_ms, 30_000.0);
        assert_eq!(d.treatment_avg_duration_ms, 15_000.0);
        assert_eq!(d.duration_delta_pct, -50.0);

        assert_eq!(d.control_avg_turns, 6.0);
        assert_eq!(d.treatment_avg_turns, 3.0);
        assert_eq!(d.turn_delta_pct, -50.0);

        assert_eq!(d.control_avg_cost_usd, Some(0.20));
        assert_eq!(d.treatment_avg_cost_usd, Some(0.12));
        assert_eq!(d.control_cost_trials, 2);
        assert_eq!(d.treatment_cost_trials, 2);
        assert!((d.cost_delta_pct.unwrap() - -40.0).abs() < 1e-9);
        assert_eq!(report.cost_missing_trials, 0, "fully priced run");
    }

    /// A partially-priced arm is averaged over the trials that reported a cost, not
    /// over all of them: dividing a one-trial sum by three trials would report the
    /// run at a third of what it actually cost.
    #[test]
    fn partial_cost_averages_only_priced_trials() {
        let report = run_scripted(3, |treatment, trial| {
            let cost = if treatment {
                Some(0.05)
            } else if trial == 0 {
                Some(0.30)
            } else {
                None // e.g. a timed-out run, or an agent that prices nothing
            };
            RunOutcome {
                input_tokens: 900,
                output_tokens: 100,
                turns: 4,
                duration_ms: 12_000,
                cost_usd: cost,
                ..Default::default()
            }
        });
        let d = &report.deltas[0];

        assert_eq!(
            d.control_avg_cost_usd,
            Some(0.30),
            "mean of the priced trial, not 0.30/3"
        );
        assert_eq!(d.control_cost_trials, 1, "denominator is visible");
        assert_eq!(d.treatment_cost_trials, 3);
        assert!((d.treatment_avg_cost_usd.unwrap() - 0.05).abs() < 1e-9);
        assert!((d.cost_delta_pct.unwrap() - -83.333_333).abs() < 1e-5);
        assert_eq!(
            report.cost_missing_trials, 2,
            "the run must admit which trials had no price"
        );
        // The metrics that are always recorded stay whole-arm.
        assert_eq!(d.control_avg_duration_ms, 12_000.0);
        assert_eq!(d.control_avg_turns, 4.0);
    }

    /// An agent that prices nothing (codex today) reports no cost at all rather than
    /// a free run, and no delta is invented from it.
    #[test]
    fn an_unpriced_agent_reports_no_cost_rather_than_zero() {
        let report = run_scripted(2, |_, _| RunOutcome {
            input_tokens: 500,
            output_tokens: 100,
            turns: 2,
            duration_ms: 5_000,
            cost_usd: None,
            ..Default::default()
        });
        let d = &report.deltas[0];
        assert_eq!(d.control_avg_cost_usd, None);
        assert_eq!(d.treatment_avg_cost_usd, None);
        assert_eq!(d.control_cost_trials, 0);
        assert_eq!(d.treatment_cost_trials, 0);
        assert_eq!(d.cost_delta_pct, None, "no baseline, no percentage");
        assert_eq!(report.cost_missing_trials, 4);
        assert_eq!(d.control_avg_turns, 2.0, "turns still measured");
    }

    /// One trial per arm: the rollup is that trial, and the low-N caveat stands.
    #[test]
    fn single_trial_per_arm_reports_that_trial() {
        let report = run_scripted(1, |treatment, _| RunOutcome {
            input_tokens: 1000,
            output_tokens: 0,
            turns: if treatment { 2 } else { 7 },
            duration_ms: if treatment { 8_000 } else { 32_000 },
            cost_usd: Some(if treatment { 0.04 } else { 0.10 }),
            ..Default::default()
        });
        assert_eq!(report.trials_per_arm, 1);
        assert!(report.low_n_caveat);
        let d = &report.deltas[0];
        assert_eq!(d.control_avg_turns, 7.0);
        assert_eq!(d.treatment_avg_turns, 2.0);
        assert!((d.turn_delta_pct - -71.428_571).abs() < 1e-5);
        assert_eq!(d.control_avg_duration_ms, 32_000.0);
        assert_eq!(d.treatment_avg_duration_ms, 8_000.0);
        assert_eq!(d.control_cost_trials, 1);
        assert!((d.cost_delta_pct.unwrap() - -60.0).abs() < 1e-9);
    }

    /// `trials: 0` is not a zero-trial run — it falls back to 3, so the rollups have a
    /// real denominator instead of dividing by zero.
    #[test]
    fn zero_trials_falls_back_and_an_empty_suite_is_rejected() {
        let report = run_scripted(0, |_, _| RunOutcome {
            input_tokens: 100,
            output_tokens: 0,
            turns: 1,
            duration_ms: 1_000,
            cost_usd: Some(0.01),
            ..Default::default()
        });
        assert_eq!(report.trials_per_arm, 3);
        let d = &report.deltas[0];
        assert_eq!(d.control_avg_turns, 1.0);
        assert_eq!(d.control_avg_duration_ms, 1_000.0);
        assert_eq!(d.control_cost_trials, 3);

        let empty_suite = run_eval(
            &Suite {
                id: "empty".into(),
                ..Default::default()
            },
            &demo_skill(),
            &Config {
                trials: 3,
                model: String::new(),
                artifacts_dir: None,
            },
            &ScriptedAgent::new(|_, _| RunOutcome::default()),
            &StubFixture,
            &one_task_verifier(),
        );
        match empty_suite {
            Err(e) => assert!(e.contains("no tasks"), "{e}"),
            Ok(_) => panic!("a suite with no tasks has nothing to roll up"),
        }
    }

    /// An empty arm divides by zero everywhere it could, and a zero baseline is not a
    /// percentage. These are the guards the rollups above rely on.
    #[test]
    fn empty_and_zero_baselines_do_not_divide_by_zero() {
        let empty = ArmAcc::default();
        assert_eq!(empty.avg_tokens(), 0.0);
        assert_eq!(empty.avg_duration_ms(), 0.0);
        assert_eq!(empty.avg_turns(), 0.0);
        assert_eq!(empty.avg_cost_usd(), None);
        assert_eq!(empty.unpriced_trials(), 0);
        assert_eq!(rate(0, 0), 0.0);
        assert_eq!(avg(0.0, 0), 0.0);

        assert_eq!(delta_pct(0.0, 5.0), 0.0, "no baseline → no percentage");
        assert_eq!(delta_pct(4.0, 2.0), -50.0);
        assert_eq!(cost_delta_pct(Some(0.0), Some(1.0)), None);
        assert_eq!(cost_delta_pct(None, Some(1.0)), None);
        assert_eq!(cost_delta_pct(Some(1.0), None), None);
        assert_eq!(cost_delta_pct(Some(0.2), Some(0.1)), Some(-50.0));
    }

    // ---- wire compatibility (these types are shared with BuildBetter ZeroShot) ----

    /// The pre-effort shape of the report, exactly as an already-compiled client
    /// models it. Deserializing a NEW report into it must still work — the effort
    /// fields are additions, not a new contract.
    #[derive(Deserialize)]
    struct LegacyTaskDelta {
        task_id: String,
        control_pass_rate: f64,
        treatment_pass_rate: f64,
        pass_rate_delta: f64,
        control_avg_tokens: f64,
        treatment_avg_tokens: f64,
        token_delta_pct: f64,
    }

    #[derive(Deserialize)]
    struct LegacyReport {
        deltas: Vec<LegacyTaskDelta>,
        trials_per_arm: u32,
        low_n_caveat: bool,
        isolation: String,
    }

    #[test]
    fn an_older_client_still_parses_a_report_with_effort_metrics() {
        let report = run_scripted(2, |treatment, _| RunOutcome {
            input_tokens: 1000,
            output_tokens: 200,
            turns: if treatment { 3 } else { 6 },
            duration_ms: if treatment { 9_000 } else { 30_000 },
            cost_usd: Some(if treatment { 0.05 } else { 0.10 }),
            ..Default::default()
        });
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"treatment_avg_duration_ms\""), "{json}");

        let legacy: LegacyReport = serde_json::from_str(&json).unwrap();
        assert_eq!(legacy.trials_per_arm, 2);
        assert!(legacy.low_n_caveat);
        assert_eq!(legacy.isolation, "worktree");
        assert_eq!(legacy.deltas.len(), 1);
        let d = &legacy.deltas[0];
        assert_eq!(d.task_id, "task-a");
        assert_eq!(d.control_pass_rate, 0.0);
        assert_eq!(d.treatment_pass_rate, 1.0);
        assert_eq!(d.pass_rate_delta, 1.0);
        assert_eq!(d.control_avg_tokens, 1200.0);
        assert_eq!(d.treatment_avg_tokens, 1200.0);
        assert_eq!(d.token_delta_pct, 0.0);
    }

    /// And the other direction: a report written by an older CLI (no effort fields at
    /// all) still deserializes, with the effort rollups reading as absent, not as a
    /// run that took no time and cost nothing meaningful.
    #[test]
    fn a_report_from_an_older_cli_still_deserializes() {
        let older = r#"{
          "deltas":[{"task_id":"task-a","control_pass_rate":1.0,"treatment_pass_rate":1.0,
            "pass_rate_delta":0.0,"control_avg_tokens":1200.0,"treatment_avg_tokens":800.0,
            "token_delta_pct":-33.3}],
          "trials_per_arm":3,"low_n_caveat":true,"isolation":"docker","future_field":{"a":1}
        }"#;
        let report: Report = serde_json::from_str(older).unwrap();
        assert_eq!(report.trials_per_arm, 3);
        assert_eq!(report.cost_missing_trials, 0);
        let d = &report.deltas[0];
        assert_eq!(d.token_delta_pct, -33.3);
        assert_eq!(d.control_avg_duration_ms, 0.0);
        assert_eq!(d.control_avg_turns, 0.0);
        assert_eq!(d.duration_delta_pct, 0.0);
        assert_eq!(
            d.control_avg_cost_usd, None,
            "absent cost must not read as $0"
        );
        assert_eq!(d.treatment_avg_cost_usd, None);
        assert_eq!(d.cost_delta_pct, None);
        assert_eq!(d.control_cost_trials, 0);
    }
}
