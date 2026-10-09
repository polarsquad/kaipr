# Adding clusters and apps

This reference ships one environment, `local-host`. Everything here is the
local-host shape; the bootstrap engine (`bootstrap-rs/`) is deliberately
generic over environments, so adding a new provider or environment is the
same kind of work krops does for AWS/Azure/GCP (see
[krops](https://github.com/polarsquad/krops) for those worked examples).

## Adding a workload cluster

`local-host` already provisions one workload cluster, `local-workload`, from
`mgmt/local-host/clusters/`. To add another cluster to the same environment:

1. Create `mgmt/local-host/clusters/<name>/` with a `cluster.yaml`,
   `kustomization.yaml` (set `namePrefix`), and `capi-nameref.yaml` so CAPI
   cross-references get the prefix applied (see the existing
   `local-workload` cluster).
2. Label the `Cluster` with `fluxcd: enabled`.
3. Register it in `mgmt/local-host/clusters/kustomization.yaml` and add a
   `Kustomization` entry in `mgmt/local-host/clusters/flux-ks.yaml` with
   `dependsOn: [capd-system]`.
4. If the new cluster needs a different workload tree, add a matching
   FluxInstance entry to `mgmt/local-host/addons/flux-apps/` (sync path plus
   any `cluster-vars`).
5. Run `mise run validate`, commit, and push.

## Adding apps to the workload cluster

`workload/local-host/` is the overlay that the workload Flux instance
reconciles, delivered wholesale from the OCI artifact. Follow the `podinfo`
pattern:

1. Create `workload/local-host/<app>/` with a `kustomization.yaml` listing the
   app's manifests, and a `flux-ks.yaml` defining the Flux `Kustomization`
   (path `./workload/local-host/<app>`; add `dependsOn` and `wait: true` as
   needed).
2. Register the `flux-ks.yaml` in `workload/local-host/kustomization.yaml`.
3. Run `mise run validate`, commit, and push. The workload cluster picks it up
   on its next sync from the OCI artifact.

For the AI platform specifically, add the new component to
`workload/local-host/ai-platform/app/` in the right reconciliation order
(control plane, then model servers, then the `InferencePool`/EPP, then the
policies that bind them). If the component introduces a CR-consuming kind,
the CRDs belong in `workload/local-host/ai-platform/crd/`, not in the app
tree: Flux dry-runs a Kustomization's whole tree before applying any of it,
so a CR in the same tree as its own CRD installer deadlocks on first boot
(see the `local-ai-platform` / `local-ai-platform-crd` split in
`workload/local-host/flux-ks.yaml`).

## Using other providers

The management cluster is not CAPD-only. Providers are declared as CAPI
operator CRs (`operator.cluster.x-k8s.io/v1alpha2`) under
`mgmt/<environment>/capi-providers/`, one directory per provider namespace,
registered in that environment's `capi-providers/flux-ks.yaml`. The operator
resolves the well-known provider names (`aws`, `azure`, `gcp`, `talos`,
`k0sproject-k0smotron`) from the same built-in registry `clusterctl` uses, so a
provider is just a typed CR with a pinned version:

```
mgmt/<environment>/capi-providers/<name>-system/
  namespace.yaml
  kustomization.yaml        # lists namespace.yaml + providers.yaml (+ secrets)
  providers.yaml            # the typed provider CR(s)
  <credentials>.sops.yaml   # optional cloud credentials, SOPS-encrypted
```

The Flux registration in `flux-ks.yaml` follows the existing entries:
`dependsOn: [capi-system]`, `decryption.provider: sops` when credentials ship
in Git, and a `healthChecks` entry naming one CRD the provider installs so Flux
waits for it.

Contract note: this repo pins CAPI core v1.14.2, which speaks the v1beta2
contract and still accepts v1beta1-contract providers until the v1beta1
removal (tentatively CAPI v1.16, April 2027). Prefer providers that already
speak v1beta2.

To add a new environment (a new provider or cloud), copy the
`mgmt/local-host/` and `workload/local-host/` layout under a new environment
directory, add it to `bootstrap.toml`, and add a matching `mise.<env>.toml`
if the environment needs extra tools. The krops repository has the full
worked examples for AWS (CAPA/EKS), Azure (CAPZ/AKS), GCP (CAPG/GKE + Config
Connector), and Talos (CABPT/CACPPT + Tinkerbell); the shape of each
environment is identical, only the provider CRs and cluster definitions
differ.

### After adding a provider

1. `mise run validate` builds every overlay, including the new directory.
2. Open the PR; konflate renders the blast radius (new CRDs, provider
   deployments) into the PR comment and the `konflate / Rendered Flux diff`
   check.
3. After merge, confirm the provider came up:
   `kubectl get pods -n <name>-system` and
   `kubectl get <kind>providers.operator.cluster.x-k8s.io -A` (e.g.
   `infrastructureproviders`).
