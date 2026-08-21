# Local agent instructions

## Mandatory pre-commit code-hygiene gate

Before creating or amending a commit, load `.claude/skills/code-hygiene/SKILL.md` completely and perform its full
code-hygiene pass on every Rust file included in the commit. If the agent supports repository skills, invoke
`code-hygiene`; otherwise follow the skill file manually.

Do not run `git commit`, `git commit --amend`, or an equivalent commit-producing command until the pass is complete,
its required formatting and checks have been run, and any failures have been resolved or explicitly handed back to
the user. For a commit with no Rust changes, inspect the staged file list and explicitly establish that the hygiene
scope is empty.

Treat the staged content as the reviewed unit. If a Rust file changes or is staged again after the hygiene pass,
rerun the pass for the affected file before committing. Never use `--no-verify` or another bypass to evade this gate.

## Commits always reflect the README

Every commit leaves `README.md` accurate:

- A commit that adds or changes behavior updates the README in the same commit.
- The README never claims unimplemented functionality; its status checklist must match reality at each commit.
- Pure refactors or internal work with no user-visible surface may skip the README update, but never leave it stale.

## Canonical commands

`just` is the single entry point (scripts-free equivalents of the vprogs shell wrappers):

```bash
just fmt          # nightly rustfmt + taplo, workspace + guest crate
just fmt-check    # CI-style verification only
just check        # clippy --tests with warnings denied, workspace + guest crate
just udeps        # unused-dependency scan (nightly)
just test         # workspace + guest crate tests (L1 e2e is env-gated, off by default)
```

Run the relevant recipes before every commit; `just fmt-check` and `just check` must pass.

## Commit style

- Terse messages describing what and why; no co-author lines, no AI trailers, no generated-with footers.
- Never mention `CLAUDE.md`, the code-hygiene skill, or internal docs paths in commit messages.

## Rust module style

- **Never** use `mod.rs`. Use Rust 2018 path style: a module `foo` with submodules lives in `foo.rs` (declaring
  `mod bar;`) alongside a `foo/` directory holding `bar.rs`.
- Applies to all new and edited code. If a `mod.rs` exists in code we touch, rename it to the sibling-file form.

## Docs layout

- `docs/<topic>/`: public, committed documentation; the README links into it.
- `docs/internal/`: git-excluded (agent-written specs, implementation plans, session notes). Never stage it.

## vprogs dependency

- vprogs is consumed as an external dependency through Cargo manifests only (currently a local path pin; a git or
  crates.io pin replaces it later without code changes here).
- Never vendor or commit vprogs code into this repo. The single sanctioned exception: the guest crate's
  `runtime/` modules, ported file-by-file from vprogs' runtime-processor with provenance noted, pending the vprogs
  split PR that will replace them with reusable crates.
- Never stage vprogs-side artifacts (target dirs, ELFs, lockfiles) or reference them outside Cargo manifests.

## Optional: prior-session context via ctx

If the `ctx` CLI is installed and initialized, prefer it over raw transcript digging when investigating non-trivial
work that may have prior decisions, failed attempts, commands, or test results:

```bash
export CTX_DATA_ROOT="$(git rev-parse --show-toplevel)/.ctx"
ctx status                     # initialize once with `ctx setup` if missing
ctx search "<topic>" --workspace vprog-tictactoe
ctx show event <ctx-event-id> --window 5
```

Prefer text output; use `--refresh off` for fast read-only queries. Search alternate terms before concluding
history is absent. Cite retrieved session/event IDs when they influence a decision. Never copy raw ctx output
externally without review.

ctx is optional tooling, not a requirement: skip silently when it is unavailable; other memory implementations
may substitute for it.

## Compiler-backed code navigation

When Rust crates land, prefer `cargo build-graph` (`--out target/build-graph`) over grep for cross-crate symbol
questions; rebuild only when stale. Never stage graph files, reports or caches.

## Environment notes

- `RISC0_DEV_MODE=1` is the default via `.cargo/config.toml` (stub receipts). Unset it for real proving.
- `CLAUDE.md` is a local, untracked pointer to this file; it never enters git.
