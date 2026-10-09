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

- `mise.toml` and `mise.local-host.toml`: tool pins (kubectl, flux, age,
  kind, clusterctl, helm, python, uv, kustomize) and the task layers.
- `bootstrap.toml`: the Flux Operator, cert-manager, and CAPI Operator chart
  pins consumed by `kaipr-bootstrap`. One annotation-driven custom manager
  reads the adjacent `# renovate:` metadata. `mise run validate` cross-checks
  these pins against their declarative Helm releases.
- `bootstrap-rs/Cargo.toml` and `bootstrap-rs/Cargo.lock`: Rust crate
  dependencies through Renovate's Cargo manager.
- `bootstrap-rs/Dockerfile`: digest-pinned build and runtime base images, and
  the mise CLI and Podman remote-client build arguments used by the toolbox.
  The `mise install` layer names tools without versions (`python`, `uv`,
  etc.), so every pin resolves from the copied `mise.toml` at build time;
  there is no inline version to keep in lockstep.
- `mgmt/**` and `workload/**` YAML: Flux, Helm, and Kubernetes manifests,
  chart values, and the CAPI provider CRs under
  `mgmt/local-host/capi-providers/`.
- `kindest/node` image tags in the local-host cluster classes (the management
  cluster, the workload cluster, and the kind bootstrap node all run the same
  version).
- `pyproject.toml` and `uv.lock`: repository test dependencies (pytest)
  through the native Python managers.
- `pivot.sh`: imperative cert-manager and CAPI Operator chart pins, grouped
  with `bootstrap.toml` and the Git manifests so a bump lands in one PR.
- `.github/workflows/`: GitHub Actions references.

Grouping keeps related bumps in a single PR: the `kubernetes-version` group
covers `kindest/node` and the `kubectl` pin (both track a literal Kubernetes
release version), and the `platform-charts` group keeps cert-manager's chart
pin together with its image tags. `Renovate` proposes one PR at the newest
available version for each dependency. Base images in `bootstrap-rs/Dockerfile`
are digest-pinned while retaining readable tags. Nothing automerges. A
`minimumReleaseAge` of 3 days keeps a freshly published, possibly compromised
release from landing on `main` unreviewed.

### Inherited, inert rules

`renovate.json5` was inherited from krops and still contains rules for surfaces
this repository does not ship: the `airgap/` image inventory, the
`virtualized-e2e/` WireMock harness, the Zarf packaging, the
`mise.aws.toml` / `mise.azure.toml` / `mise.gcp.toml` /
`mise.local-talos.toml` environment layers, and the `mgmt/local-talos/`
manifests. Those files are not present (or are present only as unused task
layers), so the matching rules never find a dependency and never open a PR.
They are kept so the file stays diffable against the krops source; pruning
them is a separate cleanup, not a correctness issue.

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
   kustomize overlay under `mgmt/` and `workload/`, lints YAML, checks script
   syntax, and runs the cross-checks (`bootstrap.toml` against the
   declarative Helm releases, the toolbox-run behavior tests, the docs
   helper-run test).
3. For toolbox inputs, also require the `bootstrap-rs` workflow's Rust checks
   and container build/smoke job.
4. For a `kubernetes-version` PR, confirm the `kindest/node` tag and the
   `kubectl` pin both moved to the same release.
5. Merge manually.

## Intentional differences

- Unversioned local tags such as `kaipr-registry:5000/kaipr:latest` have no
  comparable release version and remain untracked.
- `scripts/toolbox-run.sh` defaults `TOOLBOX_IMAGE` to the mutable
  `ghcr.io/polarsquad/kaipr-toolbox:latest`; Renovate does not manage this
  runtime default.
- The toolbox Dockerfile installs `docker-ce-cli` from Docker's apt repository
  without a package-version pin. Its client version intentionally follows that
  repository, while the base image, mise installer, and Podman client remain
  Renovate-managed.
- `*.sops.yaml` `version:` fields, Kubernetes `apiVersion` strings, Helm chart
  `appVersion` values, and `bootstrap-rs/Cargo.toml`'s package version are not
  dependency pins.
