# Secret management

In-cluster secrets are managed with [SOPS](https://github.com/getsops/sops) +
[age](https://github.com/FiloSottile/age), so encrypted manifests can live
safely in Git and Flux decrypts them at reconcile time.

- **`.sops.yaml`** declares the age *public* key (safe to commit) and a rule
  that encrypts only `data`/`stringData` fields of any `*.sops.yaml` file under
  `workload/`, leaving keys and metadata readable.
- The age *private* key lives in `age.agekey` (gitignored). The bootstrap
  (`bootstrap-common.sh`) loads it into the cluster as the `sops-age` secret
  in `flux-system`.

The tree currently carries one encrypted manifest:
`workload/local-host/ai-platform/app/secrets/anthropic-key.sops.yaml`, the
Anthropic API key consumed by the `anthropic-llm` backend (see
[Multi-provider](./inference.md#multi-provider-anthropic-hosted)). The next
candidate is the Azure OpenAI API key (issue #23). Encrypted manifests are
consumed by the `local-ai-platform` Kustomization, which declares:

```yaml
spec:
  decryption:
    provider: sops
    secretRef:
      name: sops-age
```

The `secretRef` resolves only in the Kustomization's own namespace
(`flux-system`), so the decryption key is needed exactly where the
Kustomization lives.

## The age key in one namespace

krops seeds `sops-age` in both `flux-system` and `default`, because its
ResourceSet-generated Kustomizations live in `default` and SOPS-decrypt
there. This repo has no such Kustomizations: everything reconciles from
`flux-system`, so the bootstrap seeds a single copy. That is the one
deliberate deviation from the krops pattern.

The full age key in `flux-system/sops-age` can decrypt every `*.sops.yaml` in
this repo. Do not grant Secret read in `flux-system` to principals that
should not hold the age key.

## Key lifecycle

SOPS steps are mise tasks (`mise.toml` pins `sops` and `age`), run from the
repository root. The SOPS CLI must be told where the private key lives:

```sh
mise install
export SOPS_AGE_KEY_FILE=./age.agekey   # or set it in .env (mise loads .env)
```

Generate the key (refuses to overwrite an existing `age.agekey`) and read the
public key it prints:

```sh
mise run sops-keygen        # creates ./age.agekey and prints the public key
```

Put the printed public key into the `age:` field of `.sops.yaml`. The private
key stays local (gitignored); only the public key is committed.

Encrypt a new manifest in place. Name it `*.sops.yaml` so the creation rule
applies; `data`/`stringData` are encrypted, everything else stays readable:

```sh
$EDITOR workload/local-host/.../my-secret.sops.yaml
mise run sops-encrypt workload/local-host/.../my-secret.sops.yaml
```

View a decrypted secret without changing it (stdout only):

```sh
mise run sops-decrypt <file>.sops.yaml
```

After changing the recipients in `.sops.yaml`, re-encrypt every `*.sops.yaml`
under `workload/` so they target the current recipients:

```sh
mise run sops-updatekeys
```

## Rotation

There is no automated rotation.

- **Recipient change** (add or remove a key in `.sops.yaml`): edit
  `.sops.yaml`, run `mise run sops-updatekeys`, commit. A cluster keeps
  decrypting as long as the key its `sops-age` secret holds is still a
  recipient.
- **Full key rotation** (compromise or personnel change): generate a new
  keypair (`mise run sops-keygen` after moving the old file aside, or
  `age-keygen -o age.agekey.new`), replace the recipient in `.sops.yaml` with
  the new public key, run `mise run sops-updatekeys` (every manifest now
  targets the new key only), move the new private key into place as
  `age.agekey`, and re-run the bootstrap so the cluster's `sops-age` secret
  carries the new key. A cluster still holding the old key cannot decrypt the
  re-encrypted manifests until it is re-seeded.

## Existing clusters

The `sops-age` secret is created imperatively by the bootstrap, not in Git.
A cluster that has no `sops-age` cannot reconcile the `local-ai-platform`
Kustomization (it declares `spec.decryption`) until the secret exists.
Re-running the bootstrap seeds the secret; the seed step is idempotent
(delete + create), so a re-run also picks up a rotated key.

The native path (`./bootstrap.sh`, via `bootstrap-common.sh`) seeds the
secret. The toolbox path (`scripts/toolbox-run.sh bootstrap`, the Rust
engine in `bootstrap-rs/`) does not seed it yet; until that parity lands, an
existing cluster gets `sops-age` from a native bootstrap re-run.
