# PR review: konflate

[Konflate](https://github.com/home-operations/konflate) reviews this repo's
open PRs as **rendered** Flux diffs instead of raw file diffs. For each PR it
renders the full Flux output at the merge-base and at the head, then diffs the
two, so a review shows:

- **Blast radius**: which clusters/Kustomizations a change actually touches
  (a one-line kustomize edit can fan out to many rendered resources).
- **Image changes**: container image bumps extracted from the rendered output.
- **Render failures**: a PR that breaks the Flux render is caught before
  merge, not at reconcile time.
- **Danger lint**: cautions on risky changes.

In this repository konflate runs as a one-shot service container inside a
GitHub Actions job. There is no in-cluster konflate instance: the
`local-host` management cluster is a local kind cluster that GitHub-hosted
runners cannot reach, so CI renders on its own. The review is a merge gate.

## The workflow

`.github/workflows/konflate.yml` runs konflate on every pull request to
`main`:

- konflate runs as a **service container** inside the job
  (`ghcr.io/home-operations/konflate:0.6.4`). `KONFLATE_PR_FILTER_EXPR`
  scopes it to render only the triggering PR, and `KONFLATE_REFRESH_INTERVAL=0`
  makes it a one-shot render: the job ends when the render does. `KONFLATE_REPO`
  is `github://polarsquad/kaipr`, rendered from the repo root.
- The job waits on `GET /readyz`, then pulls the summary Markdown from
  `GET /api/prs/{n}/summary?forge=github`. The endpoint answers
  `503 + Retry-After` until the render reaches a terminal state, so
  `curl --retry` waits it out with no polling loop.
- The summary is upserted as a single PR comment keyed by konflate's hidden
  `<!-- konflate:pr-N -->` marker, so a re-render edits the same comment in
  place instead of piling up duplicates.
- The job gates on the `X-Konflate-Render-Status` header: only `ok` passes.
  `failures` (some resources did not render) and `error` (no diff produced)
  fail the job. To gate merges on the render, require the
  `konflate / Rendered Flux diff` check in branch protection.

## Authentication

The workflow uses only its own `GITHUB_TOKEN`: konflate clones the repo and
lists PRs with it, and the job posts the comment with it
(`contents: read`, `pull-requests: write`, `issues: write`, plus `checks` /
`statuses: read` for konflate's CI-status reads). There are no PATs or stored
secrets to rotate.

Fork pull requests are skipped: rendering one would run untrusted sources
through konflate with repository permissions. A maintainer can recreate a
fork's branch inside the repository to obtain the render before merging.

## What the render covers

The render reads the repository's Flux Kustomizations from the repo root
(`./mgmt/local-host/...`, `./workload/local-host/...`). Because the
`local-host` environment syncs from an OCI artifact rather than a live
`GitRepository`, the render reflects the declared Git state at the PR head -
the same state a bootstrap would publish. A change to any kustomization under
`mgmt/` or `workload/` is what the blast-radius and image-change analysis sees.
