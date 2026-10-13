# Extending the reference

This reference ships one environment, `local-host`, and that environment is a
**single cluster by design**: one kind cluster runs the Flux instance and the
platform. There is no management cluster and no cluster provisioning, so
"adding a cluster" is out of scope here - that is the CAPI surface
[krops](https://github.com/polarsquad/krops) keeps and this reference dropped.
What this reference does support is adding applications to the cluster, and
adding another `local-host`-shaped environment.

## Adding apps to the cluster

`workload/local-host/` is the tree the cluster's Flux instance reconciles,
delivered wholesale from the OCI artifact. Add the app as its own
subdirectory:

1. Create `workload/local-host/<app>/` with a `kustomization.yaml` listing the
   app's manifests, and a `flux-ks.yaml` defining the Flux `Kustomization`
   (path `./workload/local-host/<app>`; add `dependsOn` and `wait: true` as
   needed).
2. Register the `flux-ks.yaml` in `workload/local-host/kustomization.yaml`.
3. Run `mise run validate`, commit, and push, and republish the artifact
   (see [Republish after a change](./operations.md#bootstrap)). The cluster
   picks it up on its next sync.

For the AI platform specifically, add the new component to
`workload/local-host/ai-platform/app/` in the right reconciliation order
(control plane, then model servers, then the `InferencePool`/EPP, then the
policies that bind them). If the component introduces a CR-consuming kind,
the CRDs belong in `workload/local-host/ai-platform/crd/`, not in the app
tree: Flux dry-runs a Kustomization's whole tree before applying any of it,
so a CR in the same tree as its own CRD installer deadlocks on first boot
(see the `local-ai-platform` / `local-ai-platform-crd` split in
`workload/local-host/flux-ks.yaml`).

## Adding another environment

An environment is a `[environments.<name>]` section in
[`bootstrap.toml`](../bootstrap.toml) plus a `mise.<name>.toml` environment
layer (the `[env]` profile default and the helper tasks, such as
`oci-push`). A second local environment - for example a second machine's
`dev-laptop` - is therefore:

1. Add `[environments.<name>]` to `bootstrap.toml` with `kind = "<name>"`,
   `sync = "oci"`, and a `sync-path` pointing at the tree to reconcile. The
   section name is authoritative: `kind` must match it.
2. Add `mise.<name>.toml` with the `oci-push` task (copy
   `mise.local-host.toml` and point `OCI_SOURCE_PATH` at the same or a
   different tree) and the `[env]` `KAIPR_PROFILE` default.
3. The cluster/registry names in `[bootstrap]` are global; if the new
   environment needs different ones, that is an engine change (the names are
   read from the global section, not per environment).
4. Run `mise run validate` (the `bootstrap.toml` cross-check requires each
   environment's sync path to exist and the mise layer to carry `oci-push`),
   commit, and push.

The bootstrap engine (`bootstrap-rs/`) stays generic over environments:
nothing in it is `local-host`-specific.

### After adding an environment

1. `mise run validate` builds every overlay and cross-checks the config.
2. Open the PR; konflate renders the blast radius into the PR comment and the
   `konflate / Rendered Flux diff` check.
3. After merge, bootstrap the new environment:
   `scripts/toolbox-run.sh bootstrap <name>`.
