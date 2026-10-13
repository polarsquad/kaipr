# Dependencies

Dependency versions live in the files that consume them: mise configuration,
`bootstrap.toml`, Cargo manifests and locks, the toolbox Dockerfile,
Kubernetes and Flux manifests, and GitHub Actions workflows. There is no
central version catalog.

Renovate discovers the pins through
[`renovate.json5`](../renovate.json5) and opens weekly update PRs through the
hosted Renovate GitHub App. Pending and proposed updates appear in the Renovate
dependency dashboard issue.

## Managed surfaces

What this repository actually ships, Renovate manages:

- `mise.toml`: tool pins (go, kubectl, python, kind, helm, flux2, uv) plus the
  mise CLI self-pin (`min_version`). The per-tool custom managers resolve each
  pin against the exact upstream repository it installs from.
- `bootstrap.toml`: the imperative chart pins consumed by `kaipr-bootstrap`.
  `local-host` has one, `flux-operator`. An annotation-driven custom manager
  reads the adjacent `# renovate:` metadata. `mise run validate` cross-checks
  these pins against the Git tree.
- `bootstrap-rs/Cargo.toml` and `bootstrap-rs/Cargo.lock`: Rust crate
  dependencies through Renovate's Cargo manager.
- `bootstrap-rs/Dockerfile`: digest-pinned build and runtime base images, and
  the mise CLI and Podman remote-client build arguments used by the toolbox.
  The `mise install` layer names tools without versions (`python`, `uv`,
  etc.), so every pin resolves from the copied `mise.toml` at build time;
  there is no inline version to keep in lockstep.
- `workload/**` YAML: Flux, Helm, and Kubernetes manifests and chart values
  for the `ai-platform` (agentgateway, model server, InferencePool + llm-d
  Router EPP, LLM policies).
- `pyproject.toml` and `uv.lock`: the repository Python lint tooling (ruff).
- `.github/workflows/`: GitHub Actions references.

Grouping keeps related bumps in a single PR: the `github-actions` group
covers the workflow action pins (excluding workflow images and runners).
`Renovate` proposes one PR at the newest available version for each dependency.
Base images in `bootstrap-rs/Dockerfile` are digest-pinned while retaining
readable tags. Nothing automerges.

### Pruned krops surface

`renovate.json5` was inherited from krops. The rules for surfaces this
repository does not ship - the cloud environment mise layers, the
management-cluster manifests, the CAPI provider CRs, the SOPS/age tooling, the
`airgap/` packaging, and the `virtualized-e2e/` harness - were pruned in the
cleanup tracked by issue #12 and again with the single-cluster change; only
rules targeting files this repo ships remain.

## Toolbox release version

The `kaipr-bootstrap` package version lives in `bootstrap-rs/Cargo.toml`. It is
a release version, not a dependency pin, so Renovate does not increment it.
When a `v*` tag is pushed, `.github/workflows/toolbox-release.yml` fails unless
the tag matches that package version, then publishes the multi-architecture
toolbox image, signs it, and attaches an SPDX SBOM attestation.

Lifecycle tool versions installed in the image come from `mise.toml`. The
Dockerfile separately pins its Rust builder and Debian runtime base images,
plus the mise installer and Podman remote-client build arguments; Renovate
manages those base references and build arguments.

## Update procedure

1. Wait for a Renovate PR. For configuration troubleshooting, use the pinned
   local dry-run procedure in [AGENTS.md](../AGENTS.md) under "Editing
   renovate.json5".
2. Review the raw and rendered diffs. The `validate` workflow builds every
   kustomize overlay under `workload/`, lints YAML, checks script syntax, and
   runs the cross-checks (`bootstrap.toml` against the Git tree, the
   toolbox-run behavior tests, the docs helper-run test).
3. For toolbox inputs, also require the `bootstrap-rs` workflow's Rust checks
   and container build/smoke job.
4. Merge manually.

## Intentional differences

- The kind cluster uses the kind 0.33.0 default node image (Kubernetes
  v1.37.0); the cluster has no in-repo node-image pin, so Renovate tracks the
  `kind` tool pin in `mise.toml` but not a `kindest/node` tag.
- Unversioned local tags such as `kaipr-registry:5000/kaipr:latest` have no
  comparable release version and remain untracked.
- `scripts/toolbox-run.sh` defaults `TOOLBOX_IMAGE` to the mutable
  `ghcr.io/polarsquad/kaipr-toolbox:latest`; Renovate does not manage this
  runtime default.
- The toolbox Dockerfile installs `docker-ce-cli` from Docker's apt repository
  without a package-version pin. Its client version intentionally follows that
  repository, while the base image, mise installer, and Podman client remain
  Renovate-managed.
- Kubernetes `apiVersion` strings, Helm chart `appVersion` values, and
  `bootstrap-rs/Cargo.toml`'s package version are not dependency pins.
