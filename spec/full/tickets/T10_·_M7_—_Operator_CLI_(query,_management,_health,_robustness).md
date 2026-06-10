# T10 · M7 — Operator CLI (query, management, health, robustness)

**Milestone:** M7 · **Backlog:** T16.2 (#79), T16.3 (#80), T17 (#57), T18 (#58)

## Scope

**In:**

- Advanced query: IPC client to agent, single-shot `--sql` + `explain`/`validate`, streaming/pagination, parameter binding, NO_COLOR/TERM handling; degraded results return distinct non-zero exit code.
- Management commands: `rules` (incl. file-based + minimal interactive create), `alerts`, `health`, `data`, `config`, `service`; YAML config hierarchy; destructive actions confirm by default with `--yes`/`--force`.
- Health/diagnostics: color-coded overview surfacing collection rate, rule eval latency, alert delivery health, redb write latency, and degraded-detection summary.
- CLI robustness: insta snapshot tests, actionable errors, shell completions (bash/zsh/fish/PowerShell).

**Out:** Interactive REPL shell (post-v1.0).

## Spec references

- spec:.../811199cd-23dc-4ded-9932-735252ec2438 (Flows 2, 3, 4, 6, 7, 9)
- requirements R8.1–8.5, R10.2–10.3, R10.5

## Key touchpoints

- file:daemoneye-cli/src/main.rs — expand clap subcommands: `query`, `rules`, `alerts`, `health`, `data`, `config`, `service`, `audit`; CLI talks to agent over IPC only (never opens DB directly).
- file:daemoneye-lib/src/ipc/client.rs — CLI→agent client; query request/response protobuf (extend file:daemoneye-lib/proto/ipc.proto).
- file:daemoneye-lib/src/config.rs — YAML config hierarchy + precedence (flag > env > user > system > default).
- Query path reuses the same SELECT-only allowlist as rules (T5); degraded results return distinct non-zero exit code; honor `NO_COLOR`/`TERM=dumb`.
- New dep: `clap_complete` (shell completions: bash/zsh/fish/PowerShell). Tests: file:daemoneye-cli/tests/cli.rs (insta snapshots).

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean.
- insta snapshot tests stable under `NO_COLOR=1 TERM=dumb`; test short/long flags; destructive actions confirm-by-default with `--yes`/`--force`; verify exit codes for complete vs degraded.

## Dependencies

T6 (query/detection), T8 (audit commands).

## Acceptance criteria

- All command groups operate over agent IPC (CLI never touches DB directly); completeness markers + exit codes behave per Core Flows; snapshot tests stable under `NO_COLOR=1 TERM=dumb`.
