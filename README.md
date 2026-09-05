# SkillRank

**Find, install, evaluate, and publish AI-agent skills — with real numbers.**

[![SkillRank on StartupScores](https://startupscores.com/badge/skillrank.svg?style=shield&v=combo&theme=dark)](https://startupscores.com/open-source/skillrank)

SkillRank is an open-source CLI and a public registry for agent skills
(`SKILL.md` packages used by Claude Code, Codex, Copilot, and others). Instead of
ranking skills by install counts, SkillRank runs reproducible **paired evals** on
your own agent and shows what a skill actually does to your **token spend, speed,
and success rate** — then lets you install, rate, review, and publish.

It works entirely on its own. The core — search, install, and local eval —
**needs no account**. It also integrates seamlessly with
[BuildBetter ZeroShot](https://buildbetter.app): install ZeroShot too and it
recommends skills from your real coding sessions and tracks realized savings.

```sh
curl -fsSL skillrank.dev | sh
```

The installer offers to also install ZeroShot — optional, and you can add it any
time. ZeroShot bundles SkillRank; SkillRank does not require ZeroShot.

Already installed? `skillrank update` upgrades in place, and since v0.1.5 the CLI
tells you when a newer release is out. [CHANGELOG.md](CHANGELOG.md) — also at
[skillrank.dev/changelog](https://skillrank.dev/changelog) — covers what each
release changed.

## Quick start

```sh
skillrank search playwright              # browse the registry (no account)
skillrank recommend                      # suggest skills for this repo's stack
skillrank install owner/skill            # hash-verified install into .claude/skills
skillrank eval owner/skill --suite ...   # paired eval on YOUR agent; prints deltas
skillrank publish https://github.com/... # index a public skill (needs login)
```

## Commands

| Command | Account? | What it does |
|---|---|---|
| `search <query>` | no | Search the registry (filter by `--stack`, `--agent`, `--category`). |
| `show <ref>` | no | A skill's scores, security tier, and eval results by trust tier. |
| `install <ref>` | no | Verify content hash and write into the repo's skill surface; records a lockfile entry. Refuses on hash mismatch or takedown. |
| `list` / `uninstall <slug>` | no | Manage installed skills; `list` reports drift. |
| `recommend` | no | Detect this repo's stack and suggest matching skills. |
| `eval <ref> --suite <id>` | no | Run forced-mode paired trials (skill vs no-skill) on your own agent, print per-task success, token, turn, wall-clock, and cost deltas, write a result bundle. `--publish` to contribute it. |
| `rate` / `review` / `publish` | yes | Contribute back. `login` stores a token; the core never needs one. |

## Using it inside Claude Code and Codex

You should never have to know a command. Register skillrank once and the agent
gets it as native tools — say *"find me a skill for playwright and install it"*
and it just works.

```sh
skillrank setup     # registers the skillrank MCP server with Claude Code + Codex
```

That writes an MCP server entry into `~/.claude.json` and `~/.codex/config.toml`
(both backed up, idempotent). Restart your agent; it now has tools
`skill_search`, `skill_recommend`, `skill_show`, `skill_install`, and
`skill_list` and calls them on its own when you talk about skills. Claude Code
prompts once to approve the tools — approve them (or pre-allow with
`{"permissions":{"allow":["mcp__skillrank"]}}` in `~/.claude/settings.json`).

**Why MCP:** the tools live in the agent's vocabulary directly, so it doesn't
guess unrelated tools and doesn't depend on skill-activation heuristics. It's the
one mechanism that works the same in Claude Code and Codex.

`setup` also installs a Skill (`~/.claude/skills/skillrank/SKILL.md`,
`~/.codex/skills/skillrank/SKILL.md`) and the `/skillrank` slash command.
`skillrank update` refreshes both, so an improvement to the Skill reaches an
existing install without re-running `setup`. If you have edited either file by
hand, or deleted it, skillrank leaves it that way and says so; `--force`
overrides, keeping the previous bytes in `<file>.skillrank-bak`.

*Alternative / complement — a skill file.* You can also drop a `SKILL.md` that
teaches the agent about skillrank into a repo:

```sh
skillrank skill --install     # writes .claude/skills/skillrank/SKILL.md
```

Either way, skills you `install` land in `.claude/skills/` (or `.agents/skills/`)
and the agent discovers them automatically — no restart needed.

### When the agent reaches for skillrank on its own

The Skill's trigger is situational, not only "when the user asks": the agent may
consult skillrank *before* starting work with a framework, library, or tool it
has no established approach for, and *after* a second failed attempt at the same
problem with no new information — to check whether an existing skill already
encodes the approach before it keeps improvising.

That path is deliberately bounded: read-only commands only, at most one
suggestion per session, one sentence, never blocking your task — and it will not
install anything without an explicit yes from you, whatever the skill's scan tier
says. `setup` never writes to any file inside a repository.

If you would rather it only spoke up when asked:

```sh
skillrank setup --triggers=user-only   # permanent: survives later setup + update
skillrank setup --triggers=default     # turn the agent-initiated trigger back on
skillrank setup --print                # show which variant would be written
```

Rationale and the measured baseline it is tuned against:
[docs/agent-initiated-skill-discovery-spec.md](docs/agent-initiated-skill-discovery-spec.md).

## Run a registry locally (make search work with no hosted service)

skillrank talks to a registry over HTTP (`SKILLRANK_API_URL`). Until the hosted
registry is up, run your own with one command — it serves a seed catalog of real
skills, so search / recommend / install work end to end:

```sh
skillrank serve                              # http://localhost:8899, seed catalog
export SKILLRANK_API_URL=http://localhost:8899
skillrank search "front end"                 # real results
```

To point your **agent's** MCP server at the local registry (so "find me a
front-end skill" works inside Claude Code / Codex), pass the URL to setup — it
writes it into the MCP config's env for you:

```sh
skillrank serve &                                        # keep it running
skillrank setup --api-url http://localhost:8899          # wires both agents
```

`serve --catalog <file.json>` uses your own catalog instead of the built-in seed.

## How the eval works

For each task in a suite, SkillRank runs your agent twice — once with the skill
installed (treatment) and once without (control) — against a pinned fixture repo,
then applies a **verifier that the agent never sees during the run** (verifier
isolation). It reports per-task deltas locally and, if you publish, submits a
signed-attributed result bundle. Results are shown under honest trust tiers —
**Official** (reproduced by us), **Community-reported** (≥3 independent accounts,
not yet reproduced), **Self-reported** — and are never mixed.

### What it reports

Pass rate is not the main event. Most skills do not make an agent *correct* — on a
small suite the honest outcome is usually "both arms pass, no delta". What a skill
changes is **effort**, so each task gets pass rate and tokens on one line, then
turns, wall-clock, and cost on the next:

```
Results (3 trials/arm, docker isolation):
  build-a-feature          pass 100%→100% (+0 pp), tokens +50.0%
                           turns 6.0→3.0 (-50.0%), time 30.0s→15.0s (-50.0%), cost $0.2000→$0.1200 (-40.0%)
  (low N: <5 trials/arm — treat deltas as directional, not significant)
```

That skill spends 50% more tokens and is still worth installing: half the turns,
half the wall-clock, 40% less money. Reading `tokens` alone would have called it a
regression.

Cost is the one metric an agent may not report (`codex` reports none at all; a
timed-out trial reports none either). A missing cost is **never** averaged in as
`$0`, which would make a run look cheaper than it was: cost means cover only the
trials that reported a price, the run states how many trials went unpriced, and an
arm nothing priced prints `n/a` instead of a number. `--json` carries the same
rollups per task (`control_avg_turns`, `duration_delta_pct`, `control_cost_trials`,
…), with `null` for a cost nothing reported.

Non-Docker runs and runs off the reference agent version publish as Self-reported
only. See [`docs/`](docs) for the full methodology.

## Configuration

- `SKILLRANK_API_URL` — registry base URL (default `https://api.skillrank.dev`;
  point at a self-hosted or local registry).
- `SKILLRANK_TOKEN` — registry token for writes (or `skillrank login --token`).
- `SKILLRANK_HOME` — config dir (default `~/.skillrank`).
- `SKILLRANK_NO_UPDATE_CHECK=1` — turn off the update check entirely.
- `SKILLRANK_AUTO_UPDATE=1` — apply available updates instead of just saying so.

### Update check

About once a day, skillrank prints one line to **stderr** when a newer release
exists, and leaves upgrading to you:

```
skillrank 0.2.0 available (you have 0.1.4): run `skillrank update`, or set SKILLRANK_AUTO_UPDATE=1 to auto-apply.
```

It never touches stdout (so `--json` output stays parseable), never changes the
exit code, and does no network work while your command runs — the result is
cached in `~/.skillrank/update-check.json` and refreshed afterwards. Both the
line and the lookup are rate-limited by the same daily TTL, so this is one line
a day, not one line per command. The refresh bounds connect and transfer at 2s
(DNS resolution is the exception — the OS resolver has no cancellation API), and
a failed lookup is remembered too, so an offline machine backs off for an hour
instead of retrying on every invocation. It stays quiet under `mcp`, `serve`,
and `update`, when `CI` is set, and whenever stderr is not a terminal. Notifying
is the default because this binary runs inside agent loops and scripts, where
silently swapping the executable mid-session is a worse surprise than a stale
version.

With `SKILLRANK_AUTO_UPDATE=1`, the same daily check applies the update instead
of printing. The downloaded binary is verified against the SHA-256 published
with the release before it replaces anything — the same fail-closed check
`install.sh` does, and exactly what `skillrank update` does. If applying fails
(a root-owned install directory is the usual reason) the error is printed rather
than swallowed, that version is not retried, and you get the ordinary notice
until you upgrade by hand.

## Build from source

Rust (stable, edition 2021):

```sh
cargo build --release      # target/release/skillrank
cargo test
```

## Architecture

A Cargo workspace:

- `crates/skillrank-core` — **library**: registry client, lockfile, install,
  content-hash verify, stack detection, skill-surface discovery, and the eval
  harness (`runner`: forced-mode paired trials, verifier isolation, agent-usage
  parsing, bundle construction — the same code for official baselines and
  community runs). Dependency-light and agent-agnostic, so BuildBetter ZeroShot /
  the Rust `bb` CLI can embed it as a crate to provide `bb skills` from this one
  implementation.
- `crates/skillrank` — the `skillrank` **binary**: search/show/install/list/
  uninstall/recommend/eval, plus `serve` (local registry), `setup` (MCP
  registration), `mcp` (stdio MCP server), and `skill`.

The hosted registry (search, publish, reviews, leaderboards, official baselines)
is a separate service; this repo is the client + local registry + eval harness +
agent integration.

## License

SkillRank is split by component:

- **CLI + core library** (`crates/`) — **MIT** ([LICENSE](LICENSE)). Fully open
  source: install it, embed it, fork it, ship it commercially. This is the part
  you run.
- **Hosted registry service + website** (`registry/`, `web/`) — **Elastic License
  2.0** ([registry/LICENSE](registry/LICENSE)). Source-available: read it, run it,
  self-host it — but you may not offer it to others as a hosted or managed service
  that competes with SkillRank.

In short: the tool is MIT and free for anyone (including at companies); you just
can't stand up a competing hosted SkillRank. Indexed skills remain under their own
upstream licenses.
