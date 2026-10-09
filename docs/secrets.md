# Secret management

This repository currently carries **no SOPS-encrypted secrets in-tree**. The
`local-host` environment syncs from a local OCI registry (not GitHub), needs no
cloud credentials, and the AI platform's OpenAI-compatible path needs no API
key. There is nothing to encrypt today.

The SOPS + age machinery is still wired in because the scaffolding is derived
from [krops](https://github.com/polarsquad/krops), where in-tree secrets are
the norm. If you add an environment or a component that needs a secret, use the
pattern below so Flux can decrypt it at reconcile time.

## The pattern

In-cluster secrets are managed with [SOPS](https://github.com/getsops/sops) +
[age](https://github.com/FiloSottile/age), so encrypted manifests can live
safely in Git and Flux decrypts them at reconcile time.

- **`.sops.yaml`** declares the age *public* key (safe to commit) and a rule
  that encrypts only `data`/`stringData` fields of any `*.sops.yaml` file. The
  checked-in rule currently targets `mgmt/aws/`, `mgmt/azure/`, or
  `mgmt/gcp/`; extend the `path_regex` to whatever path you add.
- The age *private* key lives in `age.agekey` (gitignored). Bootstrap loads it
  into the cluster as the `sops-age` secret in `flux-system`.
- A Flux `Kustomization` that consumes an encrypted file sets
  `spec.decryption.provider: sops`.

> The AI platform's OpenAI-compatible path needs no API key; the CPU simulator
> produces model-shaped completions without a backend. If you point the backend
> at an external model, inject the key as a Secret referenced by the backend,
> never as a literal. See [Inference platform](./inference.md).

## First-time setup

Every SOPS step is a mise task run in the toolbox as your own user, so the key
file it writes is owned by you (the run shape is defined in
[Helper tasks in the toolbox](./operations.md#helper-tasks-in-the-toolbox)).
The commands below use this shell function for brevity:

```sh
export TOOLBOX_IMAGE=ghcr.io/polarsquad/kaipr-toolbox:latest   # or kaipr-toolbox:dev
kaipr_mise() {
  docker run --rm -it --user "$(id -u):$(id -g)" -e HOME=/tmp \
    -v "$PWD:/workspace" -w /workspace \
    -e MISE_AUTO_INSTALL=0 -e SOPS_AGE_KEY_FILE=/workspace/age.agekey \
    --entrypoint mise "$TOOLBOX_IMAGE" "$@"
}
```

Generate the key (refuses to overwrite an existing `age.agekey`) and read the
public key it prints:

```sh
kaipr_mise run sops-keygen        # creates ./age.agekey and prints the public key
```

`AGE_KEY_FILE` (in `.env`, default `age.agekey`) is the bootstrap input;
`SOPS_AGE_KEY_FILE` (set by `kaipr_mise` above) tells the SOPS CLI which private
key to use while editing encrypted files.

Put the printed public key into the `age:` field of `.sops.yaml`, then
re-encrypt every `*.sops.yaml` under `mgmt/` so they target your key:

```sh
kaipr_mise run sops-updatekeys
```

## Encrypting and decrypting a secret

```sh
# Encrypt a manifest in place (the file must be named *.sops.yaml and match
# the .sops.yaml rule):
kaipr_mise run sops-encrypt path/to/secret.sops.yaml

# Decrypt to stdout without changing the file:
kaipr_mise run sops-decrypt path/to/secret.sops.yaml
```

Verify before committing that only the `data`/`stringData` fields are encrypted
and the rest of the manifest is still readable, and run `mise run validate`.
A Git update is not live adoption: after merge to `main`, confirm Flux reconciled
the decryption on the target.
