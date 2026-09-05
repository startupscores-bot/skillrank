//! `skillrank eval` — run a local forced-mode paired eval on the user's own agent
//! and optionally publish the result bundle.

use crate::commands::new_client;
use crate::flags::Flags;
use skillrank_core as core;
use skillrank_core::runner::{
    self,
    agent::{version_band, CliAgentRunner},
    fixture::{docker_available, GitFixtureProvider, ScriptVerifier},
    Config,
};
use std::path::PathBuf;
use std::process::Command;

pub fn run(args: &[String]) -> i32 {
    let f = Flags::parse(args);
    let Some(reference) = f.positionals.first().cloned() else {
        eprintln!("usage: eval <ref> --suite <id> [--trials N] [--agent claude|codex] [--model M] [--publish]");
        eprintln!("       --allow-verifier-exec   consent to running the suite's verifier scripts on this machine");
        eprintln!(
            "       --artifacts-dir <path>   preserve every completed trial workspace for review"
        );
        return 2;
    };
    let suite_id = f.value("suite");
    if suite_id.trim().is_empty() {
        eprintln!("error: --suite <id> is required");
        return 2;
    }
    let trials: u32 = f.value("trials").parse().unwrap_or(3).max(1);
    let provider = {
        let p = f.value("agent").trim().to_string();
        if p.is_empty() {
            detect_agent_provider()
        } else {
            p
        }
    };
    if provider != "claude" && provider != "codex" {
        eprintln!("error: could not find a supported agent CLI; install `claude` or `codex`, or pass --agent");
        return 1;
    }
    if !binary_available(&provider) {
        eprintln!("error: agent {provider:?} not found on PATH");
        return 1;
    }

    let client = new_client(&f);

    // Resolve the skill and ensure content for the treatment arm.
    let mut resolved = match client.resolve(&reference) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    if resolved.inline_content.trim().is_empty() && !resolved.raw_content_url.trim().is_empty() {
        match client.fetch_raw_content(&resolved.raw_content_url) {
            Ok(c) => resolved.inline_content = c,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }
    if resolved.inline_content.trim().is_empty() {
        eprintln!("error: registry did not provide skill content to evaluate");
        return 1;
    }

    let suite = match client.get_suite(suite_id) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let verifiers = match client.fetch_verifiers(suite_id, &suite.version) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: could not fetch verifiers for suite {suite_id}: {e}");
            return 1;
        }
    };

    // Registry-supplied code gate. The suite's verifier scripts and fixture
    // remote come from whoever published the suite, and running them executes
    // their code on this machine — there is no sandbox today (Docker presence
    // only labels the trust tier). This consent is deliberately separate from
    // the cost prompt below: `--yes` approves spend, never code execution.
    if !f.bool("allow-verifier-exec") {
        eprintln!(
            "⚠  Suite {}@{} ships verifier scripts that will execute on THIS machine.",
            suite.id, suite.version
        );
        eprintln!("   fixture repo: {}", suite.fixture.git_url);
        eprintln!("   verifier scripts: {}", verifiers.len());
        eprintln!("   These run directly on your host — there is no sandbox.");
        eprintln!("   Only continue for suites you trust.");
        if f.wants_json() || f.bool("yes") || f.bool("y") {
            eprintln!("Aborted: a non-interactive run must pass --allow-verifier-exec explicitly.");
            return 1;
        }
        if !crate::commands::confirm("Execute this suite's code on this machine?") {
            eprintln!("Aborted. Re-run with --allow-verifier-exec to skip this prompt.");
            return 1;
        }
    }

    let cfg = Config {
        trials,
        model: f.value("model").to_string(),
        artifacts_dir: match f.value("artifacts-dir").trim() {
            "" => None,
            value => Some(PathBuf::from(value)),
        },
    };

    // Cost estimate + confirmation.
    let (est_tokens, est_cost) = runner::estimate_cost(&suite, &cfg);
    if !f.wants_json() {
        println!(
            "Eval plan: skill {} vs no-skill on suite {}@{}",
            resolved.slug, suite.id, suite.version
        );
        println!(
            "  agent: {provider} | model: {} | {trials} trials/arm | {} tasks × 2 arms",
            or_dash(cfg.model.as_str()),
            suite.tasks.len()
        );
        println!(
            "  estimated: ~{} tokens, ~${:.2} on YOUR agent subscription",
            human_int(est_tokens),
            est_cost
        );
        if !docker_available() {
            println!("  note: Docker not detected → worktree isolation; results publish as Self-reported.");
        }
        if !f.bool("yes") && !f.bool("y") && !crate::commands::confirm("Proceed?") {
            println!("Aborted.");
            return 1;
        }
    }

    let agent = CliAgentRunner {
        provider: provider.clone(),
        binary: provider.clone(),
        version: detect_agent_version(&provider),
    };
    let fixtures = GitFixtureProvider::new(&suite.fixture.git_url, &suite.fixture.commit);
    let verifier = ScriptVerifier::new(verifiers);

    let result = match runner::run_eval(&suite, &resolved, &cfg, &agent, &fixtures, &verifier) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let mut bundle = result.bundle;
    bundle.created_at = core::install::now_rfc3339();

    let bundle_path = write_local_bundle(&bundle);

    if f.wants_json() {
        let out = serde_json::json!({
            "bundle": bundle,
            "report": result.report,
            "conforming": result.conforming,
            "bundlePath": bundle_path.clone().unwrap_or_default(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print_report(&result.report);
        if let Some(p) = &bundle_path {
            println!("\nBundle written: {p}");
        }
    }

    if f.bool("publish") {
        match client.submit_bundle(&bundle) {
            Ok(resp) => {
                if !f.wants_json() {
                    let tier = if resp.tier_state.is_empty() {
                        "self_reported"
                    } else {
                        &resp.tier_state
                    };
                    println!(
                        "Published (tier: {tier}{})",
                        conform_note(result.conforming)
                    );
                }
            }
            Err(e) => {
                eprintln!("publish failed: {e}");
                return 1;
            }
        }
    }
    if !f.wants_json() {
        crate::commands::print_zeroshot_tip();
    }
    0
}

fn print_report(report: &runner::Report) {
    for line in report_lines(report) {
        println!("{line}");
    }
}

/// The rendered report, one line per element. Split out from printing so the layout
/// is unit-testable — this is the only place a user sees whether a skill was worth
/// installing, so pass rate is not allowed to be the whole story.
fn report_lines(report: &runner::Report) -> Vec<String> {
    let mut lines = vec![format!(
        "\nResults ({} trials/arm, {} isolation):",
        report.trials_per_arm, report.isolation
    )];
    for d in &report.deltas {
        lines.push(format!(
            "  {:<24} pass {:.0}%→{:.0}% ({:+.0} pp), tokens {:+.1}%",
            d.task_id,
            d.control_pass_rate * 100.0,
            d.treatment_pass_rate * 100.0,
            d.pass_rate_delta * 100.0,
            d.token_delta_pct
        ));
        // Effort, on its own continuation line under the same label column: the
        // question most skills actually answer is "less work?", not "correct?".
        lines.push(format!(
            "  {:<24} turns {:.1}→{:.1} ({:+.1}%), time {}→{} ({:+.1}%), cost {}",
            "",
            d.control_avg_turns,
            d.treatment_avg_turns,
            d.turn_delta_pct,
            human_ms(d.control_avg_duration_ms),
            human_ms(d.treatment_avg_duration_ms),
            d.duration_delta_pct,
            cost_segment(d),
        ));
    }
    if report.low_n_caveat {
        lines
            .push("  (low N: <5 trials/arm — treat deltas as directional, not significant)".into());
    }
    if report.cost_missing_trials > 0 {
        let total = total_trials(report);
        lines.push(format!(
            "  (cost: {} of {total} trials reported no cost — cost above covers only the priced trials)",
            report.cost_missing_trials
        ));
    }
    lines
}

fn total_trials(report: &runner::Report) -> i64 {
    report.deltas.len() as i64 * report.trials_per_arm as i64 * 2
}

/// Cost is optional per trial, so say "n/a" rather than print a `$0.00` that reads
/// like a free run.
fn cost_segment(d: &runner::TaskDelta) -> String {
    match (d.control_avg_cost_usd, d.treatment_avg_cost_usd) {
        (Some(c), Some(t)) => match d.cost_delta_pct {
            Some(pct) => format!("{}→{} ({pct:+.1}%)", human_usd(c), human_usd(t)),
            None => format!("{}→{}", human_usd(c), human_usd(t)),
        },
        (Some(c), None) => format!("{}→n/a", human_usd(c)),
        (None, Some(t)) => format!("n/a→{}", human_usd(t)),
        (None, None) => "n/a (agent reports none)".to_string(),
    }
}

fn human_usd(usd: f64) -> String {
    if usd >= 1.0 {
        format!("${usd:.2}")
    } else {
        format!("${usd:.4}")
    }
}

fn human_ms(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0}ms")
    } else if ms < 60_000.0 {
        format!("{:.1}s", ms / 1000.0)
    } else {
        // Round to whole seconds *before* splitting: formatting the remainder
        // with `{:.0}` let a value like 119.6s render as "1m60s", because the
        // minute was floored from the unrounded value while the seconds
        // rounded up past 60.
        let total_secs = (ms / 1000.0).round() as i64;
        format!("{}m{:02}s", total_secs / 60, total_secs % 60)
    }
}

fn conform_note(conforming: bool) -> String {
    if conforming {
        String::new()
    } else {
        "; not on reference environment → not eligible for Community-reported aggregation".into()
    }
}

fn write_local_bundle(bundle: &core::EvalBundle) -> Option<String> {
    let home = core::config::home().ok()?;
    let dir = home.join("bundles");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!(
        "{}_{}_{}.json",
        sanitize(&bundle.skill_slug),
        sanitize(&bundle.suite_id),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let path = dir.join(name);
    let json = serde_json::to_string_pretty(bundle).ok()?;
    std::fs::write(&path, json).ok()?;
    Some(path.to_string_lossy().to_string())
}

fn detect_agent_provider() -> String {
    for candidate in ["claude", "codex"] {
        if binary_available(candidate) {
            return candidate.to_string();
        }
    }
    String::new()
}

fn binary_available(provider: &str) -> bool {
    Command::new(provider)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn detect_agent_version(provider: &str) -> String {
    let Ok(out) = Command::new(provider).arg("--version").output() else {
        return "unknown".to_string();
    };
    let s = String::from_utf8_lossy(&out.stdout);
    for field in s.split_whitespace() {
        if field
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            return field.to_string();
        }
    }
    let _ = version_band; // keep import meaningful for downstream
    s.trim().to_string()
}

fn or_dash(s: &str) -> String {
    if s.trim().is_empty() {
        "(agent default)".to_string()
    } else {
        s.to_string()
    }
}

fn human_int(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use skillrank_core::runner::{Report, TaskDelta};

    fn delta() -> TaskDelta {
        TaskDelta {
            task_id: "task-a".into(),
            control_pass_rate: 1.0,
            treatment_pass_rate: 1.0,
            pass_rate_delta: 0.0,
            control_avg_tokens: 1200.0,
            treatment_avg_tokens: 1800.0,
            token_delta_pct: 50.0,
            control_avg_duration_ms: 30_000.0,
            treatment_avg_duration_ms: 15_000.0,
            duration_delta_pct: -50.0,
            control_avg_turns: 6.0,
            treatment_avg_turns: 3.0,
            turn_delta_pct: -50.0,
            control_avg_cost_usd: Some(0.20),
            treatment_avg_cost_usd: Some(0.12),
            control_cost_trials: 3,
            treatment_cost_trials: 3,
            cost_delta_pct: Some(-40.0),
        }
    }

    fn report(deltas: Vec<TaskDelta>, cost_missing_trials: i64) -> Report {
        Report {
            deltas,
            trials_per_arm: 3,
            low_n_caveat: true,
            isolation: "worktree".into(),
            cost_missing_trials,
        }
    }

    /// The single-task, both-arms-pass run — the usual honest outcome. Pass rate says
    /// nothing, so the effort line has to carry the answer.
    #[test]
    fn effort_is_printed_when_pass_rate_says_nothing() {
        let lines = report_lines(&report(vec![delta()], 0));
        let joined = lines.join("\n");
        assert!(
            joined.contains("pass 100%→100% (+0 pp), tokens +50.0%"),
            "{joined}"
        );
        assert!(
            joined.contains(
                "turns 6.0→3.0 (-50.0%), time 30.0s→15.0s (-50.0%), cost $0.2000→$0.1200 (-40.0%)"
            ),
            "{joined}"
        );
        assert!(joined.contains("low N"), "{joined}");
        assert!(
            !joined.contains("reported no cost"),
            "nothing was missing: {joined}"
        );
        // The pre-existing pass/token line stays on its own line, unchanged in shape.
        assert_eq!(lines.len(), 4, "header + 2 task lines + caveat: {lines:?}");
    }

    /// A partially-priced run says so, instead of presenting a one-trial mean as the
    /// cost of the whole run.
    #[test]
    fn partial_cost_is_disclosed_with_its_denominator() {
        let mut d = delta();
        d.control_cost_trials = 1;
        d.control_avg_cost_usd = Some(0.30);
        d.cost_delta_pct = Some(-60.0);
        let joined = report_lines(&report(vec![d], 2)).join("\n");
        assert!(
            joined.contains("cost $0.3000→$0.1200 (-60.0%)"),
            "the priced mean is what gets shown: {joined}"
        );
        assert!(
            joined.contains(
                "(cost: 2 of 6 trials reported no cost — cost above covers only the priced trials)"
            ),
            "{joined}"
        );
    }

    /// An agent that prices nothing must not render as a free run.
    #[test]
    fn an_unpriced_run_says_n_a_not_zero_dollars() {
        let mut d = delta();
        d.control_avg_cost_usd = None;
        d.treatment_avg_cost_usd = None;
        d.control_cost_trials = 0;
        d.treatment_cost_trials = 0;
        d.cost_delta_pct = None;
        let joined = report_lines(&report(vec![d], 6)).join("\n");
        assert!(joined.contains("cost n/a (agent reports none)"), "{joined}");
        assert!(!joined.contains("$0.00"), "{joined}");
        assert!(
            joined.contains("turns 6.0→3.0"),
            "effort still shown: {joined}"
        );
    }

    #[test]
    fn durations_and_costs_read_at_human_scale() {
        assert_eq!(human_ms(0.0), "0ms");
        assert_eq!(human_ms(940.0), "940ms");
        assert_eq!(human_ms(41_200.0), "41.2s");
        assert_eq!(human_ms(123_000.0), "2m03s");
        // A remainder that rounds up to 60 must carry into the minute rather
        // than printing an impossible clock reading.
        assert_eq!(human_ms(119_600.0), "2m00s");
        assert_eq!(human_ms(59_999.0), "60.0s");
        assert_eq!(human_ms(60_000.0), "1m00s");
        assert_eq!(human_ms(3_599_600.0), "60m00s");
        assert_eq!(human_usd(0.0412), "$0.0412");
        assert_eq!(human_usd(1.5), "$1.50");
    }
}
