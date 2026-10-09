# The bootstrap CLI (`kaipr-bootstrap`)

The imperative part of kaipr lives in one Rust binary under
[`bootstrap-rs/`](../bootstrap-rs/). It implements the initial bootstrap, the
default CAPI pivot into the self-managed management cluster, and teardown.
After bootstrap and pivot finish, Flux owns the declared state until teardown.

The binary is the generic bootstrap engine carried over from
[krops](https://github.com/polarsquad/krops). It is deliberately environment
agnostic: it reads everything repo-specific from
[`bootstrap.toml`](../bootstrap.toml) and understands several environment
`kind`s (`local-host`, `aws`, `azure`, `gcp`, `local-talos`). This reference
enables exactly one of them, `local-host`, so the whole thing runs on a laptop
with no cloud account. The other `kind`s are part of the engine's contract
(they are what krops uses for its cloud environments) and are documented below
because they share the same interface, but no manifests for them are checked
in here.

The binary is a behavioral port of `bootstrap.sh`, `pivot.sh`, and
`teardown.sh`. It preserves their step order, progress messages, environment
interface, and safety guards, with two deliberate upgrades:

- **Reruns are safe by default.** An existing healthy `mgmt` kind cluster is
  reused and each bootstrap or pivot step is idempotent. Pass `--recreate` to
  delete and rebuild the kind cluster instead.
- **Typed process execution.** Tool arguments are passed as argv entries,
  secrets travel through stdin or the environment, and the GitHub and registry
  HTTP checks use reqwest with explicit timeouts.

## Distribution and build

The primary distribution is the toolbox image,
`ghcr.io/polarsquad/kaipr-toolbox`. It contains `kaipr-bootstrap` and the
pinned tools required by the lifecycle. See [Operations](./operations.md) for
the container invocation and host runtime contract.

The `toolbox-release` workflow runs on `v*` tags. It requires the tag to match
`bootstrap-rs/Cargo.toml`, builds Linux amd64 and arm64 images each on a
runner of that architecture, publishes `X.Y.Z`, `X.Y`, and stable `latest`
tags, signs the image with GitHub OIDC, and attaches a Syft SPDX JSON SBOM
attestation. The `bootstrap-rs` CI workflow also builds and smokes the arm64
image when its inputs change.

Each architecture builds natively rather than under QEMU: emulating the
amd64 rustc binary on the arm64 runner segfaulted deterministically and
blocked the v0.2.0 release. The two single-arch images are merged into the
multi-arch manifest with `docker buildx imagetools create` in plain bash
rather than `docker/metadata-action`, after that action's `tags` output
collapsed to a single tag on the v0.2.1 run and silently dropped the `X.Y` and
`latest` aliases.

Verify a published image against the workflow's OIDC identity:

```sh
cosign verify ghcr.io/polarsquad/kaipr-toolbox:X.Y.Z \
  --certificate-identity-regexp \
    '^https://github.com/polarsquad/kaipr/.github/workflows/toolbox-release.yml@refs/tags/vX.Y.Z$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

Published releases carry the stable tags described above; build the current
checkout as shown in [Operations](./operations.md) only for unreleased
changes.

Build the CLI directly for native development:

```sh
cd bootstrap-rs
cargo build --locked
cd ..
./bootstrap-rs/target/debug/kaipr-bootstrap --help
./bootstrap-rs/target/debug/kaipr-bootstrap teardown --help
```

Run the binary from the repository root so its default `./bootstrap.toml` path
resolves. Set `BOOTSTRAP_CONFIG` when running from another directory.

CI runs `cargo fmt --check`, clippy with warnings denied, a locked build, and
the test suite. The toolchain is pinned in `bootstrap-rs/rust-toolchain.toml`;
crate dependencies are locked in `Cargo.lock`.

## Interface

```text
kaipr-bootstrap [OPTIONS] [PROFILE] [COMMAND]
kaipr-bootstrap teardown [PROFILE]
```

Common examples:

```sh
kaipr-bootstrap                         # local-host bootstrap, then pivot
kaipr-bootstrap --recreate local-host   # rebuild the bootstrap kind cluster
kaipr-bootstrap teardown                # local-host teardown
kaipr-bootstrap teardown local-host     # local-host teardown (explicit)
```

- `PROFILE` is the CLI's positional name. Its value names a section under
  `[environments.*]` in [`bootstrap.toml`](../bootstrap.toml). This reference
  checks in a single environment, `local-host`. The engine understands more
  (`aws`, `azure`, `gcp`, `local-talos`) for the environments krops ships;
  adding one is described in [extending.md](./extending.md).
- A non-empty `KAIPR_PROFILE` overrides the positional profile. If neither is
  set, `bootstrap.default-environment` from `bootstrap.toml` is used.
- `--recreate` applies to bootstrap only. Teardown is a subcommand and keeps
  its script-compatible controls in environment variables.
- There is no pivot subcommand. Pivot is the default exit from bootstrap, and
  rerunning the normal command resumes an interrupted bootstrap or pivot.

## Repository configuration

Repository-owned cluster names, paths, chart versions, provider manifests, and
teardown targets live in [`bootstrap.toml`](../bootstrap.toml). The binary
retains generic fallback defaults and sequence-level contracts. It reads
`./bootstrap.toml` by default; `BOOTSTRAP_CONFIG` selects another path.

Runtime environment variables take precedence where an override exists.
`mise run validate` parses the file and cross-checks its chart pins and
teardown names against the Git manifests. Renovate updates the annotated chart
pins together with their declarative counterparts. See
[Dependencies](./dependencies.md).

The `local-host` entry declares `sync = "oci"` (the management Flux instance
pulls the OCI artifact the local registry serves), the management cluster
name, the infra provider (`docker` / `capd-system`), and the provider
manifests. The engine also recognizes fields used only by the non-local
environment kinds, so they are not needed here:

- `pivot-sops-secrets` (optional, list): SOPS-encrypted manifests the pivot
  decrypts with `SOPS_AGE_KEY_FILE` (defaults to `AGE_KEY_FILE`) and applies to
  the target before `clusterctl move`. No environment here uses it.
- `pivot-manifests` (optional, list): plain (unencrypted) manifests applied to
  the target before `clusterctl move`, after the provider CRs, with `${VAR}`
  placeholders substituted from the bootstrap cluster's Flux namespace
  ConfigMaps.
- `post-kind-create-task` (optional, string): a mise task in the active
  profile run once the kind bootstrap cluster exists; a non-zero exit aborts
  the bootstrap.
- `teardown.manual` (optional, string): when set, `kaipr-bootstrap teardown`
  refuses to run for that environment and prints the text.

## Bootstrap and pivot controls

| Variable | Default | Used by |
|---|---|---|
| `BOOTSTRAP_CONFIG` | `./bootstrap.toml` | Repository configuration path |
| `KAIPR_PROFILE` | positional profile, then `bootstrap.default-environment` (`local-host` checked in) | Environment selection |
| `REGISTRY_PORT` | `5001` | Local-host registry host port |
| `REGISTRY_READY_RETRIES` | `120` | Local-host registry readiness attempts |
| `LOCAL_RECONCILE_TIMEOUT` | `15m` | Local-host management and workload reconciliation waits |
| `CONTAINER_ENGINE` | auto-detect Docker, then Podman | kind and registry engine |
| `GIT_REPO_URL` | required only for GitHub-synced environment kinds | Management Flux Git source |
| `GITHUB_TOKEN` | required only for GitHub-synced environment kinds | PAT with read access to the repository |
| `GITHUB_USER` | `git` | Basic-auth username paired with the PAT |
| `AGE_KEY_FILE` | `age.agekey` | SOPS age private key loaded into `sops-age` |
| `AGE_PUBLIC_KEY` | derived from `AGE_KEY_FILE` | Public key override during secret creation; must match the key file's public key when both are known (preflight fails fast on a mismatch) |
| `OCI_REPOSITORY` / `OCI_TAG` | `kaipr` / `latest` | Local-host OCI artifact name |
| `BOOTSTRAP_PIVOT` | `1` | Any value other than literal `1` skips pivot |
| `MGMT_KUBECONFIG` | `~/.kube/kaipr-mgmt.yaml` | Exported management kubeconfig for native fallback runs |
| `MGMT_READY_TIMEOUT` | `15m` for local-host | Management cluster definition and provisioning waits |
| `MGMT_POLL_INTERVAL` | `10` seconds | Management cluster definition, provisioning, and node-readiness polls |
| `BOOTSTRAP_KUBECONTEXT` | config value `kind-mgmt` | Source context required by pivot |
| `PIVOT_SKIP_DELETE` | `0` | Literal `1` keeps kind after a successful pivot |
| `KAIPR_RUN_ID` | `{profile}-{timestamp}` | Bootstrap/pivot run identifier for resource tagging; set to override auto-generation |
| `KAIPR_RUN_TTL` | `86400s` (24h) | Resource time-to-live duration (e.g. `2h`, `30m`, `3600s`), or literal `none` for persistent tags |
| `KAIPR_REVISION` | Git branch HEAD SHA | Git revision tag for resource tagging; auto-extracted during bootstrap |
| `KAIPR_RUN_KIND` | `manual` | Bootstrap/pivot invocation kind for resource tagging (e.g. `manual`, `scheduled`, `emergency`) |

`GIT_REPO_URL` and `GITHUB_TOKEN` do not apply to `local-host`, which syncs
from the local OCI registry; they are listed because the engine shares the
interface with the GitHub-synced environment kinds. The pivot's target
node-readiness wait uses a fixed 15m budget (`MGMT_NODE_READY_TIMEOUT` in
`pivot.sh` and `bootstrap-rs/src/main.rs`). It is not an environment knob and
is separate from `MGMT_READY_TIMEOUT`.

### Toolbox runtime contracts

The toolbox runtime adds five contracts:

- `KAIPR_TOOLBOX=1` enables internal kind networking and disables host-only CAPD
  endpoint rewrites.
- `ENGINE_SOCK` names the engine socket path as seen by the daemon. The wrapper
  resolves it for Docker Desktop, Docker contexts, rootful or rootless Podman,
  and `podman machine`.
- `KUBECONFIG` must name one writable file, not a colon-separated list. The
  wrapper uses `/workspace/.kube/kind.yaml`.
- `CONTAINER_HOST` points the Podman remote client at the mounted engine
  socket (`unix:///var/run/docker.sock`) when `KAIPR_TOOLBOX=1`. The CLI sets
  it only if unset, before any Podman probe, so an operator-supplied value is
  kept regardless of the eventual `CONTAINER_ENGINE`.
- `AWS_PAGER` is baked into the image as an empty string so the AWS CLI never
  pages its output through `less` (the toolbox is non-interactive and ships no
  pager). It is set for the engine's AWS path; it has no effect on
  `local-host`. An operator-supplied value via `-e AWS_PAGER=...` overrides the
  default at runtime.

The container reaches the local registry at `kaipr-registry:5000`. Its
`/root/.kube` mount makes the management kubeconfig persist on the host as
`./.kube/kaipr-mgmt.yaml`. The wrapper's environment allowlist and override
limitations are documented in [Operations](./operations.md#toolbox-container-primary-interface).

**Design notes (`bootstrap-rs/src/engine.rs`):** engine detection, socket
resolution, `CONTAINER_HOST` defaulting, and kind-network attach/detach used to
be implemented separately in `preflight_checks`, a `detect_engine_for_network`
helper that read `Config` defaults instead of the engine `preflight_checks`
actually resolved, `teardown::detect_engine`, and the toolbox shell entrypoint
(which also detected and validated the engine before Rust ever ran). All of
that now lives in one module: `preflight_checks` resolves the engine once and
threads it through bootstrap, pivot, and teardown instead of re-detecting it,
and `toolbox-entrypoint.sh` only sets `KAIPR_TOOLBOX=1` and execs. Inside the
toolbox, `podman info` reports the mounted *client* socket (`CONTAINER_HOST`),
not the daemon-side path kind's `extraMounts` need, so the module skips
querying it there and uses the static fallback instead. kind-network
attach/detach checks actual network membership rather than matching Docker's
and Podman's differently worded "already connected" errors, so it doesn't
depend on engine- or version-specific error text.

## What bootstrap and pivot do (local-host)

1. **Preflight:** validate the environment and required tools, and select a
   running container engine. A fallback native run requires `kind`, `helm`,
   `kubectl`, `clusterctl`, and `mise`; OCI-synced environments like
   `local-host` also require `flux` and `curl`. The engine's GitHub-token and
   age-key preflights run only for the GitHub-synced environment kinds.
2. **Bootstrap kind:** create or reuse `mgmt`, start the local registry,
   install the Flux Operator, publish the local OCI artifact, install the
   `FluxInstance`, and watch reconciliation.
3. **Pivot by default:** wait for the Flux-created management `Cluster`
   definition (a clean first run polls through the reconciliation chain and
   surfaces failed Kustomizations on timeout), wait for the CAPI-managed
   management cluster, export its kubeconfig, wait for the target nodes,
   install cert-manager, the CAPI operator, and provider CRs at the versions
   declared in `bootstrap.toml`, suspend Flux in kind, run `clusterctl move`,
   unpause the moved clusters, seed Flux on the target, and delete kind after
   the safety checks pass.

The engine's non-local paths add environment-specific steps (for example an
EKS EIP-quota preflight and a per-region sweep on the AWS kind, or a
PXE/Tinkerbell boot on the `local-talos` kind). Those do not run for
`local-host`.

If a phase fails, fix the cause and rerun the same command. `clusterctl move`
is re-runnable, and kind remains authoritative until the final deletion.
Recovery details and the warning against deleting moved CAPI objects are in
[Pivot recovery](./operations.md#pivot-recovery).

## Teardown controls and behavior

```sh
kaipr-bootstrap teardown [PROFILE]
```

| Variable | Default | Effect |
|---|---|---|
| `AWS_ONLY` | `0` | Engine recovery knob for the AWS kind only (skips Kubernetes steps and runs only the AWS orphan sweep). Rejected for `local-host` |
| `FORCE_KIND_DELETE` | `0` | Literal `1` removes the controller host even when CAPI cluster deletion was not confirmed |
| `CLUSTER_DELETE_TIMEOUT` | `1200` seconds | CAPI cluster deletion wait |
| `PROVIDER_DELETE_TIMEOUT` | `300` seconds | CAPI provider deletion wait |
| `MGMT_KUBECONFIG` | `~/.kube/kaipr-mgmt.yaml` | Post-pivot controller-host kubeconfig |

Teardown checks required tools before mutation. For `local-host` it requires
`kind` and `kubectl`; `AWS_ONLY=1` is rejected.

Teardown discovers where the CAPI controllers run: the pre-pivot kind cluster,
the post-pivot self-managed management cluster, or no reachable cluster. In
the toolbox the run first joins the kind network so the pre-pivot kind
endpoint resolves before discovery. Teardown binds every kubectl call to that
discovered target (an explicit kind context or the management kubeconfig; never
the operator's current context) and treats a *failed* kubectl query as an
unknown state, never as "nothing left to delete." A failed listing or lookup
(auth, forbidden, API outage) therefore aborts the teardown with a nonzero exit
and leaves the management cluster and its controllers intact, so in-flight
deprovisioning can continue; re-run it once the query works. A successful empty
listing, a named lookup reporting `NotFound`, or the API server reporting the
resource type as not installed is the only evidence accepted as "confirmed
gone" by the deletion guard, which is what allows the management cluster to be
removed.

For `local-host` the sequence is: suspend the workload Kustomization, delete
the CAPD workload cluster and wait for its containers to disappear, remove
either the pre-pivot kind cluster or the post-pivot self-managed management
containers, and remove the local registry last. (The AWS kind additionally
runs a cloud orphan sweep; the `local-talos` kind deletes every CAPI Cluster as
the release. Neither applies here.)

## Entry-point and parity status

`scripts/toolbox-run.sh bootstrap`, `scripts/toolbox-run.sh pivot`, and
`scripts/toolbox-run.sh teardown` invoke this CLI in the toolbox container. The
`pivot` wrapper verb is a named resume path for the rerun-safe default
lifecycle; it does not select a separate CLI subcommand.

The three shell scripts remain as native reference and fallback paths.
Local-host bootstrap, pivot, and post-pivot teardown have completed parity
runs against them.
