# Stacked PR CI and merge queue

This repository keeps every required check honest while making stacked work
safe to merge in one operation. The CI workflow is path-aware for ordinary
pull requests and also listens for GitHub's `merge_group` event, so a future
merge queue receives the same `frontend`, `backend`, `mobile`, and `installer`
verdicts as a pull request.

## What the current workflow guarantees

- A newer run cancels an older run for the same pull request or ref. Runs for
  different Stack layers remain independent; one layer must not claim another
  layer's result.
- Frontend, Rust, Mobile, installer, and image validation are selected by the
  `changes` classifier. A skipped surface still reports through its stable
  aggregate check, rather than leaving a required context pending.
- Rust pull-request compiler caches are read-only. This prevents untrusted
  pull-request code from writing to a shared compiler cache. Merge-queue refs
  use the same read-only mode because they execute pull-request code too.
- Rust validation is Stack-aware. For a same-repository PR, `changes` queries
  GitHub for an open child whose `base.ref` is the current PR head. A matching
  child marks this layer as intermediate; a narrow workspace-member source
  diff then runs formatting, a `cargo check` closure containing its reverse
  dependents, and tests for the changed crates. Workspace manifests,
  lockfiles, migrations, toolchain, deployment, and other broad boundaries
  fall back to the full Rust jobs. Top-level PRs, merge-group refs, `main`,
  tags, and releases always use the full workspace suite.
- The required contexts remain `frontend`, `backend`, `mobile`, and
  `installer`. Do not mark a check successful from a script, remove a required
  context, or use a bypass to compensate for a missing run.

The Rust classifier fails closed: if the GitHub child lookup, `cargo metadata`,
diff, or package mapping cannot be completed, `changes` fails and the required
`backend` check cannot pass. The lightweight job is accepted by `backend` only
when the classifier explicitly returns `lightweight`; a skipped lightweight
job can never make a full-scope PR green.

## Stack workflow

Use the installed `gh-stack` extension and keep the layers in dependency
order:

```bash
gh stack checkout <top-pr>
gh stack rebase
gh stack push --remote origin
gh stack view --json
gh stack merge <top-pr> --yes --merge
```

The final command is all-or-nothing and merges from the bottom of the Stack
upward. Use `gh stack merge`, not `gh pr merge`, for dependent PRs. Before
merging, inspect every layer's current head SHA, required checks, review
threads, and merge state. A successful run for an old SHA is not evidence for
the rebased head.

## Enabling a merge queue

The workflow's `merge_group` trigger is source-controlled, but GitHub's queue
itself is repository policy and cannot be enabled by a workflow commit. An
administrator must configure a ruleset or branch-protection rule for `main`
that:

1. requires pull requests and the same four required status checks;
2. enables the merge queue and keeps strict/up-to-date requirements enabled;
3. does not allow bypass actors to skip those checks; and
4. verifies that the queue requests checks through the `merge_group` event.

Until that external setting exists, Stack layers still run their own CI in
parallel and are not cross-PR deduplicated. Once it is enabled,
`gh stack merge <top-pr> --yes --merge` can submit the Stack to the queue;
GitHub controls queue grouping and may process the layers in more than one
group. The queue does not make real provider credentials, callbacks, or
macOS acceptance evidence appear, so those runtime gates must remain explicit.

The Stack-aware Rust optimization is intentionally source-controlled and does
not change branch protection. It reduces repeated workspace compilation for
intermediate source-only layers; it does not replace the complete Rust suite
on the top layer or after landing on `main`/release.
