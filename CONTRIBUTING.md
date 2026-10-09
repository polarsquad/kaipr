# Contributing to kaipr

kaipr is a working reference implementation of an AI platform built on
agentgateway: it shows how to stand up an inference gateway with LLM routing
and token-budget controls, reconciled onto a GitOps-managed cluster on a single
host from the same Git source as the cluster. The GitOps scaffolding (bootstrap
engine, toolbox, repo layout) is derived from krops. Contributions that sharpen
the AI-platform demonstration, make it more reproducible, or make it safer to
operate are welcome. Contributions that turn it into a framework are not.

`AGENTS.md` holds the same rules in the form AI coding agents consume; the two
files must agree, so update both when you change a workflow.

## Where the project stands

The first pass is a lean, public, laptop-reproducible reference:

- The local-host base (kind + CAPD) and the Rust bootstrap engine are in
  place, with the `mise run validate` gate and the konflate rendered-review
  workflow.
- The AI inference platform (agentgateway + InferencePool + llm-d Router EPP +
  CPU model server + LLM policies) is declared in `workload/local-host/ai-platform/`
  (CRD layer in `crd/`, app layer in `app/`, ordered by `dependsOn`) and
  passes the kustomize + cross-check gate.
- A live local-host e2e run (bootstrap through the toolbox, then a real
  inference request against the gateway) is the next step and is tracked as a
  follow-up issue.

## Ways to contribute

- Run the `local-host` lifecycle on your machine and report what breaks. This
  is the most useful contribution with the lowest cost. It needs only Docker
  or Podman.
- Exercise the AI path end to end: bring up the platform, send a
  `/v1/chat/completions` request through the gateway, and confirm the token
  budget trips. `docs/inference.md` has the verification commands.
- Improve the documentation. Every `docs/` page is fair game.
- Review open pull requests. Rendered Flux diffs make review approachable
  without cluster access.

Ideas that need discussion before code: adding a new environment (cloud or
otherwise), a change to the bootstrap sequence, anything that touches
`renovate.json5`, and anything that changes the shape of `bootstrap.toml`.
Open an issue first.

## Setting up

The toolbox container is the primary lifecycle interface; host tools are for
development and validation.

```sh
git clone https://github.com/polarsquad/kaipr.git
cd kaipr
mise trust
mise install                 # kubectl, kind, flux2, sops, age, helm, clusterctl, ...
docker build -f bootstrap-rs/Dockerfile -t kaipr-toolbox:dev .
export TOOLBOX_IMAGE=kaipr-toolbox:dev
mise run validate            # must pass on a clean checkout before you change anything
```

Requirements:

- Mise 2026.10.3 or newer (see `min_version` in `mise.toml`).
- Docker, or Podman 5.5+. kind creates clusters through the mounted engine
  socket.
- Rust via rustup only when building `bootstrap-rs/` outside the image; the
  pin is `bootstrap-rs/rust-toolchain.toml`.
- Node 24 or newer only for a Renovate dry-run.
- Python 3 for the test scripts under `tests/`.

`mise.local-host.toml` adds the local-host tool layer (flux2, kubectl, and the
`oci-push` / `kubeconfigs` helper tasks). The helper tasks run in the toolbox
image (`--entrypoint mise`, see `docs/operations.md`).

## Rules that apply to every change

These come from `AGENTS.md` and are not negotiable.

1. Edit YAML in Git; never mutate a cluster by hand. Use `kubectl` to
   inspect, then make the persistent change in the repository and let Flux
   converge.
2. Flux tracks `main`. Nothing reconciles until merged. Do not describe a fix
   as live before then.
3. No secrets in-tree. Secrets go through SOPS and age: encrypted files are
   named `*.sops.yaml` and only `data`/`stringData` fields are encrypted.
   `age.agekey` and `.env` are gitignored and must stay that way. Never commit
   a plaintext credential, token, or connection string.
4. Run `mise run validate` before pushing.

Additional conventions:

- Each component pairs a plain kustomize root (`kustomization.yaml`) with the
  Flux objects that deliver it. Register new components in the parent
  `kustomization.yaml`.
- AI-platform components are delivered wholesale by the workload Flux
  instance; there is no per-component Flux Kustomization. Keep the model
  server's label (`app: vllm-sim`) and port (8000) stable, or the InferencePool,
  the EPP, the backend, and the LLM route all break together.
- Dependency versions live in the files that consume them and are updated by
  Renovate. Do not reintroduce a central version list. If you add a new pinned
  dependency, add Renovate coverage for it in the same PR and prove it with the
  dry-run described in `AGENTS.md` ("Editing renovate.json5").
- When a change affects repository structure, workflows, or how to navigate
  the repository, update `AGENTS.md` and the relevant `docs/` page in the same
  PR.

## Making a change

### Branch and commits

- Branch from `main`. The usual branch shape is `<type>/<issue>-<slug>`,
  for example `feat/ai-token-budget` or `fix/local-host-preflight`. Drop the
  issue number when there is none. `renovate/*` is reserved for Renovate.
- Commit messages follow Conventional Commits: `feat`, `fix`, `docs`, `chore`,
  `refactor`, `test`, `ci`, with an optional scope such as `feat(ai-platform):`
  or `docs(agents):`. Reference the issue in the body (`Refs #N`, `Closes #N`).
- Write the body for a reader who was not in the room: what was wrong, what
  changed, how it was verified.
- Do not force-push a branch that someone else has commented on unless a
  maintainer asks for it.

### Scope

- One logical change per PR.
- File each distinct problem you find as its own issue, cross-referenced to
  the issue or PR where you found it. Do not fold unrelated fixes into a
  feature PR.

### Validate locally

Run the checks that match what you touched. CI runs all of them.

| You changed | Run |
|---|---|
| Anything | `mise run validate` (shell syntax, `bootstrap.toml` cross-check, every kustomize overlay, toolbox/kubeconfig/docs cross-checks, ruff) |
| `README.md` or `docs/` | `python3 tests/test-docs-toolbox-helpers.py` (no bare host mise helper lines) |
| `mgmt/` or `workload/` YAML | `yamllint` with the CI settings (line length and document start disabled, `*.sops.yaml` ignored) |
| `bootstrap-rs/` | `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo build --locked`, `cargo test --locked` |
| `renovate.json5` or a pinned version | The pinned dry-run from `AGENTS.md` |
| `bootstrap.toml` or anything it references | `python3 tests/test-bootstrap-config.py` |
| Lifecycle scripts or the CLI | A full `local-host` bootstrap and teardown through the toolbox |

### Open the pull request

- Fill in the PR description with the problem, the change, and the
  verification you ran, including command output where it proves a claim.
- Link the issue. Use `Closes #N` only when the PR completes the whole issue.
- Expect these automated checks:
  - `validate`: `bootstrap.toml` cross-check, every kustomize overlay, the
    toolbox/kubeconfig/docs cross-checks, and a Python lint gate.
  - `bootstrap-rs`: fmt, clippy, build, test for the Rust CLI.
  - `konflate`: the PR rendered as a Flux diff (blast radius, image changes,
    render failures), posted as a PR comment and required for merge. This job
    is skipped for pull requests from forks because it would run untrusted
    sources through konflate. A maintainer may recreate your branch inside the
    repository to obtain the render before merging.
- Review is done against the rendered diff and the evidence in the
  description, not against intent. If a reviewer asks for evidence, add it to
  the PR rather than replying with a description.
- `main` accepts no direct pushes, deletions, or force-pushes. Maintainers
  merge once checks are green and the review is resolved.

## Testing against real infrastructure

`local-host` is free and covers the full lifecycle, including the AI path.
Use it by default. The AI platform runs a CPU simulator, so no GPU or model
weights are needed; `docs/inference.md` documents the vLLM overlay for a real
model.

## Reporting problems

- Bugs and proposals: open a GitHub issue. State the commit on `main`, the
  command, and the observed output.
- Security-sensitive findings (a leaked credential, a bypass of the SOPS
  chain): do not open a public issue. Report it privately through the
  repository security advisory form (`SECURITY.md`).

## License

kaipr is licensed under the Apache License 2.0 (`LICENSE`). By submitting a
contribution you agree that it is licensed under the same terms. The project
does not currently require a Developer Certificate of Origin sign-off.

OCI publication from linked worktrees uses host Git metadata forwarded by
`scripts/toolbox-run.sh`. Keep `KAIPR_OCI_GIT_SHA`, `KAIPR_OCI_GIT_REF`, and
`KAIPR_OCI_SOURCE_URL` out of `.env`. Run `python3 tests/test-oci-push-worktree.py`
when changing the wrapper or `oci-push`; validate and CI also run this gate.
