# Security policy

kaipr is a working reference for an AI platform built on
[agentgateway](https://agentgateway.dev), reconciled onto a GitOps-managed
workload cluster through the Kubernetes API. It is not a product with a
release cadence, but its manifests, scripts, CLI, container image, and CI
workflows can be pointed at real clusters, so security reports are taken
seriously.

## Reporting a vulnerability

Report privately through GitHub:
[Report a vulnerability](https://github.com/polarsquad/kaipr/security/advisories/new).
This opens a draft security advisory visible only to the repository
maintainers.

Do not open a public issue, pull request, or discussion for anything that
could be exploited before a fix is available. That includes a committed
credential, a bypass of the SOPS chain, or a CI workflow that can be made to
run untrusted code with repository permissions.

Include:

- The affected path or component (manifest, script, `bootstrap-rs/`, the
  toolbox image, a workflow, `renovate.json5`).
- The commit on `main` you tested against.
- Steps to reproduce.
- Impact as you understand it: what an attacker gains and which part of the
  reference (management cluster, workload cluster, AI platform) is affected.

You will get an acknowledgement within 7 days. Maintainers volunteer their
time; there is no paid response team and no bug bounty. Please allow up to
90 days before public disclosure, or coordinate an earlier date once a fix is
merged.

## Supported versions

Only `main` is supported. There are no release branches. The toolbox image
`ghcr.io/polarsquad/kaipr-toolbox` is published from `v*` tags when they
exist; the newest tag is the only supported image version, and fixes land on
`main` first.

## Scope

In scope:

- Kubernetes manifests under `mgmt/` and `workload/`, including the
  AI-platform manifests (`workload/local-host/ai-platform/`) and the
  Gateway API / Inference Extension / agentgateway resources they declare.
- The bootstrap, pivot, and teardown lifecycle: `bootstrap-rs/`, the shell
  scripts, `bootstrap.toml`, and `scripts/toolbox-run.sh`.
- The toolbox container image and its signing and SBOM attestation.
- GitHub Actions workflows under `.github/workflows/`, including token
  permissions and fork handling.
- The Renovate configuration, where a malicious or mistaken rule could pull
  an unintended dependency.
- Secret handling: the SOPS and age setup, `.env` handling, and anything that
  could leak a credential into Git, logs, or CI artifacts.

Out of scope:

- Vulnerabilities in upstream projects the repository consumes (Flux,
  Cluster API and its providers, agentgateway, the llm-d Router, kind).
  Report those upstream; a report here is welcome only if kaipr configures
  the component in a way that makes the issue worse or bypasses a mitigation.
- Deployments made from forks or adapted copies of this repository. The
  README states the intended use: fork it and adapt it. Once adapted, the
  security posture is the operator's.

## What the repository already does

Knowing the existing controls helps frame whether a finding is a bypass or a
gap.

- No secrets are in-tree. Where a value must be encrypted, SOPS and age are
  used: only `data` and `stringData` fields in `*.sops.yaml` are encrypted;
  the age public recipient is committed, the private key (`age.agekey`) and
  `.env` are gitignored. Rotation procedures are in `docs/secrets.md`.
- The AI platform's OpenAI-compatible path needs no API key; the CPU
  simulator produces model-shaped completions without a backend. If an
  operator points the backend at an external model, the key is injected as a
  secret referenced by the backend, never as a literal.
- The `konflate` PR review workflow uses only the workflow `GITHUB_TOKEN`
  (`contents: read`, plus the scopes it needs to post the summary comment)
  and is skipped for pull requests from forks so untrusted sources are not
  rendered with repository permissions. No PATs or stored secrets are used.
- The toolbox image is signed with cosign keyless (GitHub OIDC, workflow
  identity) and carries an SPDX SBOM attestation. Verify with:

  ```sh
  cosign verify ghcr.io/polarsquad/kaipr-toolbox:X.Y.Z \
    --certificate-identity-regexp \
      '^https://github.com/polarsquad/kaipr/.github/workflows/toolbox-release.yml@refs/tags/vX.Y.Z$' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com
  ```

- Dependency updates are proposed by the hosted Renovate GitHub App and
  reviewed as ordinary pull requests, rendered as Flux diffs before merge.

## If you find a committed secret

Treat it as compromised even if it was removed in a later commit; Git history
and forks retain it.

1. Report it privately as described above.
2. Maintainers rotate the credential following `docs/secrets.md` (or the
   provider's own procedure), then re-encrypt and merge.
3. The exposure is disclosed in the advisory once rotation is confirmed.

## Attribution

Reporters are credited in the published advisory unless they ask not to be.
