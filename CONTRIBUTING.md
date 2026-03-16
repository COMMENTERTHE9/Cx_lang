# Contributing to Cx

## Branch Rules

### Rule 1 — One backend branch per task
Backend branches are short-lived. Create a branch for one task, merge or abandon it, then delete it. Never keep a long-lived backend branch open while the frontend is evolving.

### Rule 2 — Always start from fresh main
Every backend session starts with:
```bash
git checkout main
git pull
git checkout -b backend/<task-name>
```
Never resume an old backend branch from a previous session.

### Rule 3 — Stale base gate
Any PR more than 20 commits behind main is blocked from merging until rebased. CI will label it `stale-base` automatically.

### Rule 4 — Backend recovery is file-scoped
If backend work must be recovered from an old branch, carry over only the backend files needed. Never revive a stale branch as the working base. Never cherry-pick broad frontend commits into a backend branch.

## CI Checks

Every PR must pass:
- `cargo build` — frontend/runtime build
- `cargo build --features jit` — backend/IR build
- Verification matrix — all `.cx` test files in `src/tests/verification_matrix/`
- `cargo test --features jit` — backend IR tests

## Merge Policy

- Merges to `main` are manual — a human must approve
- Automated PRs from `submain` to `main` open when the matrix is green
- No auto-merge under any circumstance

## Agent Workflow

Before every commit, agents must:
1. Run `git branch --show-current` and confirm the branch is correct
2. Never commit directly to `main` or `submain` without a PR
3. Never cherry-pick frontend commits into backend branches
