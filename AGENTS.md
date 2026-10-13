# AGENTS.md: kaipr

Guidance for AI coding agents working in this repository.

## What this repo is

A working reference for an **AI platform built on agentgateway**, reconciled
onto a kind cluster from the same Git source as the cluster itself. The
GitOps scaffolding (bootstrap engine, toolbox, repo layout) is **derived from
[krops](https://github.com/polarsquad/krops)**, scoped to one environment
(`local-host`). Desired state is plain declarative YAML in Git; Flux delivers
it and controllers reconcile the platform to match. No Terraform, no state
file, no cloud account, no secrets.

The reference is **single-cluster by design**: one kind cluster runs the Flux
instance and the AI platform. There is no management cluster, no Cluster API,
and no pivot.

- A **lean, laptop-reproducible base**: one kind cluster and a local OCI
  registry that Flux syncs from, all on one host.
- An **AI inference platform** on the cluster: an agentgateway inference
  gateway in front of a Gateway API Inference Extension `InferencePool` and
  the llm-d Router Endpoint Picker, with a CPU-reproducible model server and
  LLM-aware agentgateway policies. See `docs/inference.md`.

This is a forkable reference, not a product. The bootstrap engine
(`bootstrap-rs/`) is deliberately **generic over environments**; this
reference scopes it to a single environment (`local-host`) via
`bootstrap.toml`.

## Repository layout

- `bootstrap.toml`: the repository-owned configuration. The single source of
  truth for the global cluster/registry names, the imperative chart pins, and
  the declared environments. This reference declares one environment,
  `local-host`.
- `workload/local-host/`: synced by the cluster's Flux from the OCI artifact.
  - `ai-platform/`: the AI inference platform. `crd/` is the CRD layer
    (namespace, vendored Gateway API + Inference Extension CRDs, the
    agentgateway.dev CRDs HelmRelease); `app/` is the app layer
    (`agentgateway/`, `model-server/` (the CPU vLLM simulator),
    `inference/` (InferencePool + llm-d Router EPP), and `policies/`
    (AgentgatewayBackend, LLM route, token-budget policy)). `flux-ks.yaml`
    defines the two Flux Kustomizations; the app layer `dependsOn` the CRD
    layer (first-boot ordering: Flux dry-runs a tree before applying it).
- `bootstrap-rs/`: `kaipr-bootstrap`, the Rust CLI (bootstrap, teardown) that
  reads `bootstrap.toml`. `Dockerfile` builds the toolbox image (mise + a
  podman remote client). The engine is generic over environments; the test
  fixtures in `bootstrap-rs/testdata/` carry a multi-environment
  `bootstrap.toml` so engine-semantics tests run against more than the lean
  `local-host` environment.
- `bootstrap.sh` / `teardown.sh` + `bootstrap-common.sh`: the shell lifecycle
  (native reference and fallback). They default to the `local-host` profile
  (matching `bootstrap.toml`).
- `scripts/toolbox-run.sh`: the Docker/Podman wrapper for the toolbox image;
  it handles the mounts, loads `.env`, and persists kubeconfigs under `.kube/`.
- `scripts/inference-demo.py`: the host-side browser demo for the AI layer
  (see `docs/inference.md`); stdlib-only, same origin (no CORS), creates no
  cluster object.
- `tests/`: self-contained cross-checks run by `mise run validate`
  (`test-bootstrap-config.py` cross-checks the imperative chart pins and
  environment sections against the Git tree; the rest cover the toolbox
  wrapper's exec and env-precedence behavior, OCI publication from a
  worktree, and the docs helper rule).
- `docs/`: `inference.md` (the AI layer), `architecture.md`,
  `operations.md`, `bootstrap-cli.md`, `dependencies.md`, `extending.md`,
  `konflate.md`.
- `.github/workflows/`: `validate`, `bootstrap-rs`, `konflate`,
  `toolbox-release`.

## The golden rules (read before changing anything)

1. Edit YAML in Git; never mutate the cluster. Use `kubectl` to inspect
   live state, but make every persistent change here and let Flux converge.
2. Flux tracks `main`. Nothing reconciles until merged to `main`. Do not
   promise a fix is "live" until then.
3. No secrets in the stack. `local-host` uses no cloud credentials and no
   API keys, and the AI platform's OpenAI-compatible path needs no API key.
   If you point at an external model, inject the key as a Kubernetes Secret
   referenced by the backend, never as a literal in the tree.
4. Run `mise run validate` before pushing. PRs are reviewed as rendered Flux
   diffs by the konflate workflow, so what you push is what gets reviewed.

## App layout convention

Components pair a plain kustomize root with the Flux objects that deliver
them. Register new components in the parent `kustomization.yaml`; order with
`dependsOn` / `wait: true` where it matters. The AI platform is delivered by
two Flux Kustomizations (`local-ai-platform-crd`, then `local-ai-platform`
which `dependsOn` it) so the CRDs exist before the app objects are dry-run;
see `workload/local-host/flux-ks.yaml`.

The AI layer has one load-bearing contract: the InferencePool selects the
model-server pods by the label `app: vllm-sim`, and the agentgateway
`AgentgatewayBackend` wraps the InferencePool. Keep the label and port
(8000) stable, or the EPP, the backend, and the LLM route all break together.

## Common tasks

```sh
mise install                     # host: pinned tools for validate
mise run validate                # host: cross-checks + every kustomize overlay; mirrors CI
scripts/toolbox-run.sh bootstrap # toolbox: kind + registry + Flux handoff (local-host)
scripts/toolbox-run.sh teardown  # toolbox: full teardown
# helper tasks (kubeconfigs, oci-push, inference-demo) run in the toolbox image:
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
- `docs/extending.md`: adding an environment or an app.
- `docs/konflate.md`: rendered PR review and the CI gate.
