# AGENTS.md: kaipr

Guidance for AI coding agents working in this repository.

## What this repo is

A working reference for an **AI platform built on agentgateway**, reconciled
onto a workload cluster from the same Git source as the cluster itself. The
GitOps scaffolding (bootstrap engine, toolbox, repo layout) is **derived from
[krops](https://github.com/polarsquad/krops)**, scoped to one environment
(`local-host`). Desired state is plain declarative YAML in Git; Flux delivers
it and controllers reconcile the infrastructure to match. No Terraform, no
state file, no cloud account.

The reference has two parts in one tree:

- A **lean, laptop-reproducible base**: a kind bootstrap cluster that
  Cluster API + CAPD pivots into a self-managed management cluster and a
  workload cluster, all on one host.
- An **AI inference platform** on the workload cluster: an agentgateway
  inference gateway in front of a Gateway API Inference Extension
  `InferencePool` and the llm-d Router Endpoint Picker, with a
  CPU-reproducible model server and LLM-aware agentgateway policies. See
  `docs/inference.md`.

This is a forkable reference, not a product. The bootstrap engine
(`bootstrap-rs/`) is deliberately **generic over environments**; this
reference scopes it to a single environment (`local-host`) via
`bootstrap.toml`. The engine keeps the multi-environment code paths (AWS,
Azure, GCP, Talos) and its tests exercise them against a fixture - the
`local-host`-only `bootstrap.toml` simply never enables them.

## Repository layout

- `bootstrap.toml`: the repository-owned configuration. The single source of
  truth for which environments exist, the imperative chart pins, and
  per-environment teardown. This reference declares one environment,
  `local-host`.
- `mgmt/local-host/`: synced by the MANAGEMENT cluster's Flux.
  - `capi-providers/`: cluster-api, capd (docker), and
    cluster-api-addon-provider.
  - `infrastructure/`: flux-operator, cert-manager, capi-operator.
  - `addons/`: `flux-apps` (installs Flux on the workload cluster) and `cni`.
  - `clusters/`: the docker workload cluster class and the management
    cluster definition.
- `workload/local-host/`: synced by the WORKLOAD cluster's Flux.
  - `podinfo/`: the reference app, a smoke test.
  - `ai-platform/`: the AI inference platform - `agentgateway/`,
    `model-server/` (the CPU vLLM simulator), `inference/` (InferencePool +
    llm-d Router EPP), and `policies/` (AgentgatewayBackend, LLM route,
    token-budget policy).
- `bootstrap-rs/`: `kaipr-bootstrap`, the Rust CLI (bootstrap, pivot,
  teardown) that reads `bootstrap.toml`. `Dockerfile` builds the toolbox
  image (mise + a podman remote client). The engine is generic over
  environments; the test fixtures in `bootstrap-rs/testdata/` carry a full
  multi-environment `bootstrap.toml` so engine-semantics tests run against
  more than the lean `local-host` environment.
- `bootstrap.sh` / `pivot.sh` / `teardown.sh` + `bootstrap-common.sh`: the
  shell lifecycle. They default to the `local-host` profile (matching
  `bootstrap.toml`).
- `scripts/toolbox-run.sh`: the Docker/Podman wrapper for the toolbox image;
  it handles the mounts, loads `.env`, and persists kubeconfigs under `.kube/`.
  `scripts/inference-demo.py` is the host-side browser demo for the AI layer
  (see `docs/inference.md`); it is stdlib-only and creates no cluster object.
- `tests/`: self-contained cross-checks run by `mise run validate`
  (`test-bootstrap-config.py` cross-checks the imperative chart pins against
  the Flux-reconciled versions; the rest cover the toolbox, the local-host
  kubeconfig endpoint, sops-keygen, and the docs helper rule).
- `docs/`: `inference.md` (the AI layer), `architecture.md`,
  `operations.md`, `bootstrap-cli.md`, `secrets.md`, `dependencies.md`,
  `extending.md`, `konflate.md`.
- `.github/workflows/`: `validate`, `bootstrap-rs`, `konflate`,
  `toolbox-release`.

## The golden rules (read before changing anything)

1. Edit YAML in Git; never mutate the clusters. Use `kubectl` to inspect
   live state, but make every persistent change here and let Flux converge.
2. Flux tracks `main`. Nothing reconciles until merged to `main`. Do not
   promise a fix is "live" until then.
3. No secrets in-tree. If a component needs a secret, encrypt it with SOPS +
   age into `*.sops.yaml` (only `data`/`stringData` fields are encrypted, per
   `.sops.yaml`) and keep `age.agekey` and `.env` gitignored. Never commit a
   plaintext credential, token, or connection string. The AI platform's
   OpenAI-compatible path needs no API key; if you point at an external
   model, inject the key as a secret referenced by the backend, never as a
   literal.
4. Run `mise run validate` before pushing. PRs are reviewed as rendered Flux
   diffs by the konflate workflow, so what you push is what gets reviewed.

## App layout convention

Workload components pair a plain kustomize root with the Flux objects that
deliver them. Register new components in the parent `kustomization.yaml`;
order with `dependsOn` / `wait: true` where it matters. AI-platform components
are delivered wholesale by the workload Flux instance (the `podinfo` model) -
there is no separate Flux Kustomization per component.

The AI layer has one load-bearing contract: the InferencePool selects the
model-server pods by the label `app: vllm-sim`, and the agentgateway
`AgentgatewayBackend` wraps the InferencePool. Keep the label and port
(8000) stable, or the EPP, the backend, and the LLM route all break together.

## Common tasks

```sh
mise install                     # host: pinned tools for validate
mise run validate                # host: cross-checks + every kustomize overlay; mirrors CI
scripts/toolbox-run.sh bootstrap # toolbox: kind + Flux handoff + pivot (local-host)
scripts/toolbox-run.sh teardown  # toolbox: full teardown
# helper tasks (sops-*, kubeconfigs, oci-push) run in the toolbox image:
docker run --rm -it -v "$PWD:/workspace" -w /workspace -e MISE_AUTO_INSTALL=0 \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run kubeconfigs
```

## Editing renovate.json5

Renovate configs fail silently in ways a syntax validator cannot see. Before
pushing a change to `renovate.json5`, prove extraction with a local dry-run
(Node >= 24):

```sh
GITHUB_COM_TOKEN=$(gh auth token) RENOVATE_TOKEN=$(gh auth token) \
  LOG_LEVEL=debug npx --yes -p renovate@44.50.1 \
  renovate --platform=local --dry-run=full > /tmp/rv.log 2>&1
```

Read the "Dependency extraction complete" stats and compare
`fileCount`/`depCount` per manager against what the change claims to cover.
The JSON5 single-backslash and `autoReplaceStringTemplate` traps are the
usual culprits; verify loaded patterns and simulate replacements rather than
trusting the diff.

## Where to look next

Load these only when the task touches their domain:

- `docs/inference.md`: the AI layer - request path, CRD prerequisites,
  bring-up, and the vLLM overlay.
- `docs/architecture.md`: how the pieces fit and the reconciliation order.
- `docs/operations.md`: toolbox runs, helper tasks, verification.
- `docs/bootstrap-cli.md`: the `kaipr-bootstrap` engine, knobs, parity.
- `docs/extending.md`: adding a workload cluster or an app.
- `docs/secrets.md`: SOPS + age setup and rotation.
- `docs/konflate.md`: rendered PR review, CI gate, tokens.
