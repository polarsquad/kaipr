//! kaipr-bootstrap – One-time imperative bootstrap for the local kind cluster.
//! Everything after this program runs is driven by GitOps (Flux).
//!
//! Rust port of `bootstrap.sh`. The CLI surface is the script's surface:
//! a positional profile, `--recreate`, the `teardown` subcommand, and the
//! environment. Reruns are safe by default: an existing healthy cluster is
//! reused and every step is idempotent, so a partially failed bootstrap can
//! be resumed by rerunning. Pass --recreate to delete and rebuild the
//! cluster instead.
//!
//! The bootstrap creates ONE kind cluster that runs the whole platform
//! (issue #14): no CAPI, no pivot, no management cluster, no secrets.
//! Everything repository-specific (names, paths, chart pins, environments)
//! comes from `bootstrap.toml` (see `config.rs`); the binary is a generic
//! bootstrap engine and kaipr is its first consumer.

mod config;
mod engine;
mod teardown;

use config::{BootstrapConfig, Environment, SyncSource};

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// curl's documented transient statuses: the set `--retry` retries.
const CURL_TRANSIENT_STATUSES: [u16; 6] = [408, 429, 500, 502, 503, 504];

// Defaults mirroring bootstrap.sh's `${VAR:-default}` values, plus the node
// Ready wait the script hardcodes (kubectl wait --timeout=120s).
const DEFAULT_REGISTRY_PORT: u16 = 5001;
const DEFAULT_REGISTRY_READY_RETRIES: u32 = 120;
const DEFAULT_LOCAL_RECONCILE_TIMEOUT: &str = "15m";
const DEFAULT_OCI_REPOSITORY: &str = "kaipr";
const DEFAULT_OCI_TAG: &str = "latest";
const NODE_READY_TIMEOUT: &str = "120s";
/// The exported host kubeconfig (`$HOME/.kube/kaipr.yaml` by default): the
/// one file the host talks to the cluster with. In the toolbox, `$HOME` is
/// the mounted `.kube/` directory, so the file lands in the checkout.
const DEFAULT_KUBECONFIG_RELATIVE: &str = ".kube/kaipr.yaml";

// ── CLI ───────────────────────────────────────────────────────────────────────

/// Resolve the active environment name the way bootstrap.sh does
/// (`PROFILE="${KAIPR_PROFILE:-${1:-local-host}}"`): a non-empty
/// KAIPR_PROFILE wins over the positional argument, then the config's
/// bootstrap.default-environment. clap resolves positional-over-env (the
/// opposite), so the env var is read manually instead of #[arg(env)].
/// The name must be an [environments.*] section of bootstrap.toml; the
/// section name is authoritative.
fn resolve_environment(
    env: Option<&str>,
    positional: Option<&str>,
    config: &BootstrapConfig,
) -> Result<String> {
    let name = if let Some(value) = env.filter(|v| !v.is_empty()) {
        value
    } else {
        positional.unwrap_or(&config.bootstrap.default_environment)
    };
    config.environment(name).map(|_| name.to_string())
}

/// One-time imperative bootstrap for the local kind cluster.
///
/// Behavioral port of bootstrap.sh: the CLI surface is the script's
/// surface — a positional profile, `--recreate`, and the environment.
/// Every `${VAR:-default}` knob the script reads is read the same way
/// (see `Config`); the expanded flag interface is deferred for separate
/// review in a follow-up.
#[derive(Parser, Debug)]
#[command(name = "kaipr-bootstrap", version, about)]
struct Cli {
    /// Deployment profile (a non-empty KAIPR_PROFILE takes precedence
    /// over this argument, matching bootstrap.sh; default: the config's
    /// bootstrap.default-environment). Valid names are the
    /// [environments.*] sections of bootstrap.toml.
    profile: Option<String>,

    /// Delete and recreate an existing cluster instead of reusing it.
    /// Required when the kind/registry configuration changed since the
    /// cluster was created (the reuse path does not detect drift).
    #[arg(long)]
    recreate: bool,

    /// Tear down everything the bootstrap created: the kind cluster, the
    /// local registry container, and the exported kubeconfig. Knobs stay
    /// env-only (CONTAINER_ENGINE, ENGINE_SOCK, KAIPR_KUBECONFIG),
    /// matching the script's interface.
    #[command(subcommand)]
    command: Option<SubCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum SubCommand {
    /// Destroy everything the bootstrap created, in reverse order.
    Teardown {
        /// Deployment profile (KAIPR_PROFILE takes precedence, as
        /// everywhere else; default: bootstrap.default-environment).
        profile: Option<String>,
    },
}

/// Resolved run configuration: the environment knobs bootstrap.sh reads
/// with `${VAR:-default}` semantics — unset and empty both fall back to
/// the default, exactly as in the script. Not clap args: this port
/// reproduces the script's behavior, and its interface is env-only.
#[derive(Debug)]
struct Config {
    /// The parsed bootstrap.toml (repository-owned values).
    repo: BootstrapConfig,
    /// The resolved [environments.*] section name.
    profile: String,
    /// The resolved environment section.
    environment: Environment,
    recreate: bool,
    registry_port: u16,
    registry_ready_retries: u32,
    local_reconcile_timeout: String,
    container_engine: Option<String>,
    engine_sock: Option<String>,
    toolbox: bool,
    oci_repository: String,
    oci_tag: String,
    /// The exported host kubeconfig written at the end of the bootstrap
    /// and removed by teardown.
    kubeconfig: PathBuf,
}

impl Config {
    /// Address the local registry from the process running this binary.
    /// Host runs use the published port; toolbox runs share the kind network.
    fn registry_endpoint(&self) -> (String, u16) {
        if self.toolbox {
            (self.repo.bootstrap.registry_name.clone(), 5000)
        } else {
            ("localhost".to_string(), self.registry_port)
        }
    }

    /// Resolve the run configuration from the process environment.
    fn load(cli: &Cli, repo: BootstrapConfig) -> Result<Self> {
        Self::from_env(cli, repo, |name| std::env::var(name).ok(), true)
    }

    /// Resolve the teardown subset of the run configuration: profile,
    /// environment, engine, and kubeconfig knobs only. The bootstrap-only
    /// knobs (registry, reconciliation timeout) keep their defaults, so a
    /// stray stale value cannot fail a teardown run.
    fn load_teardown(cli: &Cli, repo: BootstrapConfig) -> Result<Self> {
        Self::from_env(cli, repo, |name| std::env::var(name).ok(), false)
    }

    /// Build a Config from a lookup over the script's env knobs
    /// (injectable so the resolution logic is unit-testable). An empty
    /// value behaves like unset, matching `${VAR:-default}`.
    /// `validate_bootstrap_knobs` gates the bootstrap-only knob parsing.
    fn from_env(
        cli: &Cli,
        repo: BootstrapConfig,
        get: impl Fn(&str) -> Option<String>,
        validate_bootstrap_knobs: bool,
    ) -> Result<Self> {
        let value = |name: &str| get(name).filter(|v| !v.is_empty());
        let with_default =
            |name: &str, default: &str| value(name).unwrap_or_else(|| default.to_string());
        let profile = resolve_environment(
            value("KAIPR_PROFILE").as_deref(),
            cli.profile.as_deref(),
            &repo,
        )?;
        let environment = repo
            .environment(&profile)
            .with_context(|| "internal: environment vanished between resolution and lookup")?
            .clone();
        let registry_port = if validate_bootstrap_knobs {
            with_default("REGISTRY_PORT", &DEFAULT_REGISTRY_PORT.to_string())
                .parse::<u16>()
                .context("REGISTRY_PORT must be a port number (1-65535)")?
        } else {
            DEFAULT_REGISTRY_PORT
        };
        let registry_ready_retries = if validate_bootstrap_knobs {
            with_default(
                "REGISTRY_READY_RETRIES",
                &DEFAULT_REGISTRY_READY_RETRIES.to_string(),
            )
            .parse::<u32>()
            .context("REGISTRY_READY_RETRIES must be a non-negative integer")?
        } else {
            DEFAULT_REGISTRY_READY_RETRIES
        };
        let local_reconcile_timeout = if validate_bootstrap_knobs {
            let timeout = with_default("LOCAL_RECONCILE_TIMEOUT", DEFAULT_LOCAL_RECONCILE_TIMEOUT);
            parse_duration_seconds(&timeout)
                .with_context(|| format!("LOCAL_RECONCILE_TIMEOUT '{timeout}' is invalid"))?;
            timeout
        } else {
            DEFAULT_LOCAL_RECONCILE_TIMEOUT.to_string()
        };
        let kubeconfig = match value("KAIPR_KUBECONFIG").map(PathBuf::from) {
            Some(path) => path,
            None => default_kubeconfig()?,
        };
        Ok(Config {
            recreate: cli.recreate,
            registry_port,
            registry_ready_retries,
            local_reconcile_timeout,
            container_engine: value("CONTAINER_ENGINE"),
            engine_sock: value("ENGINE_SOCK"),
            toolbox: with_default("KAIPR_TOOLBOX", "0") == "1",
            oci_repository: with_default("OCI_REPOSITORY", DEFAULT_OCI_REPOSITORY),
            oci_tag: with_default("OCI_TAG", DEFAULT_OCI_TAG),
            kubeconfig,
            repo,
            profile,
            environment: environment.clone(),
        })
    }
}

/// Default host kubeconfig path: `$HOME/.kube/kaipr.yaml`.
fn default_kubeconfig() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set; cannot locate the kubeconfig path")?;
    Ok(home.join(DEFAULT_KUBECONFIG_RELATIVE))
}

// ── Pure helpers (unit-tested) ────────────────────────────────────────────────

/// Extract the numeric host port from `<engine> port` output ("0.0.0.0:32771").
fn extract_published_port(port_output: &str) -> Option<String> {
    port_output
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .map(str::trim)
        .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

fn toolbox_kubeconfig_path(toolbox: bool, kubeconfig: Option<&str>) -> Result<Option<PathBuf>> {
    if !toolbox {
        return Ok(None);
    }
    let path = kubeconfig
        .filter(|value| !value.is_empty())
        .context("KUBECONFIG must name one writable file when KAIPR_TOOLBOX=1")?;
    ensure!(
        !path.contains(':'),
        "KUBECONFIG must name a single file when KAIPR_TOOLBOX=1"
    );
    Ok(Some(PathBuf::from(path)))
}

fn internal_kind_kubeconfig_args(name: &str) -> [&str; 5] {
    ["get", "kubeconfig", "--internal", "--name", name]
}

/// Render the kind cluster config, mirroring the script's heredoc.
/// The cluster always speaks to the local registry through its kind-network
/// name (the containerd mirror), and mounts the host engine socket so
/// in-cluster components can reach a Docker-compatible API whether the
/// backend is Docker or Podman. `registry_name` comes from bootstrap.toml
/// ([bootstrap] registry-name).
fn render_kind_config(registry_port: u16, engine_sock: &str, registry_name: &str) -> String {
    let registry_patch = format!(
        "containerdConfigPatches:\n  - |-\n    [plugins.\"io.containerd.grpc.v1.cri\".registry.mirrors.\"localhost:{registry_port}\"]\n      endpoint = [\"http://{registry_name}:5000\"]\n"
    );
    format!(
        "kind: Cluster\napiVersion: kind.x-k8s.io/v1alpha4\n{registry_patch}nodes:\n  - role: control-plane\n    extraMounts:\n      - hostPath: {engine_sock}\n        containerPath: /var/run/docker.sock\n"
    )
}

/// Parse a duration the way the script's timeout arithmetic does: `40m`,
/// `2h`, `90s`, or a bare second count.
fn parse_duration_seconds(input: &str) -> Result<u64> {
    let s = input.trim();
    let (digits, unit): (&str, u64) = if let Some(d) = s.strip_suffix('h') {
        (d, 3600)
    } else if let Some(d) = s.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, 1)
    } else {
        (s, 1)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("invalid duration '{input}' (expected forms like 40m, 2h, 90s, or bare seconds)");
    }
    digits
        .parse::<u64>()
        .map(|n| n * unit)
        .with_context(|| format!("duration '{input}' overflows"))
}

/// Tools required on PATH for the given environment's sync surface. The
/// base set serves every environment; the OCI sync surface adds the
/// reconciliation-watch tools: flux (the error stream) and curl (the mise
/// oci-push task's registry probe).
fn required_tools(env: &Environment) -> Vec<&'static str> {
    let mut tools = vec!["kind", "helm", "kubectl", "mise"];
    if env.sync == SyncSource::Oci {
        tools.extend(["flux", "curl"]);
    }
    tools
}

// ── Process helpers ───────────────────────────────────────────────────────────

/// Is `cmd` an executable file somewhere on PATH?
fn command_exists(cmd: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable_file(&dir.join(cmd)))
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file()
        && p.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Run a command with inherited stdio; error if it exits nonzero.
pub(crate) async fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .await
        .with_context(|| format!("failed to spawn '{cmd}'"))?;
    if !status.success() {
        bail!("'{cmd} {}' failed with {status}", args.join(" "));
    }
    Ok(())
}

/// Run a command silently; report only whether it succeeded.
pub(crate) async fn run_quiet(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command and capture stdout (stderr inherited); error on nonzero exit.
pub(crate) async fn capture(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .await
        .with_context(|| format!("failed to spawn '{cmd}'"))?;
    if !out.status.success() {
        bail!("'{cmd} {}' failed with {}", args.join(" "), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a command and capture stdout, ignoring the exit status (like `cmd || true`).
pub(crate) async fn capture_lossy(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Run a command with `input` piped to stdin and stdio otherwise inherited.
pub(crate) async fn run_with_stdin(cmd: &str, args: &[&str], input: &str) -> Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn '{cmd}'"))?;
    child
        .stdin
        .take()
        .context("child stdin unavailable")?
        .write_all(input.as_bytes())
        .await?;
    let status = child.wait().await?;
    if !status.success() {
        bail!("'{cmd} {}' failed with {status}", args.join(" "));
    }
    Ok(())
}

/// kubectl args with an optional target kubeconfig (None = the current
/// context, i.e. the kind cluster).
pub(crate) fn kubectl_cmd<'a>(kubeconfig: Option<&'a str>, args: &[&'a str]) -> Vec<&'a str> {
    let mut full = Vec::with_capacity(args.len() + 2);
    if let Some(kc) = kubeconfig {
        full.push("--kubeconfig");
        full.push(kc);
    }
    full.extend_from_slice(args);
    full
}

/// Kills the wrapped child process when dropped (best-effort).
struct ChildGuard(tokio::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

// ── Preflight ─────────────────────────────────────────────────────────────────

struct Preflight {
    engine: String,
    engine_sock: String,
}

async fn preflight_checks(cfg: &Config) -> Result<Preflight> {
    // Report every missing tool at once instead of failing on the first.
    let missing: Vec<&str> = required_tools(&cfg.environment)
        .into_iter()
        .filter(|t| !command_exists(t))
        .collect();
    if !missing.is_empty() {
        bail!("missing required tools in PATH: {}", missing.join(", "));
    }

    let resolved = engine::resolve(
        cfg.container_engine.clone(),
        cfg.engine_sock.clone(),
        cfg.toolbox,
    )
    .await?;

    Ok(Preflight {
        engine: resolved.engine,
        engine_sock: resolved.engine_sock,
    })
}

// ── Steps ─────────────────────────────────────────────────────────────────────

pub(crate) async fn select_toolbox_kind_kubeconfig(cfg: &Config) -> Result<()> {
    let kubeconfig = std::env::var("KUBECONFIG").ok();
    let Some(path) = toolbox_kubeconfig_path(cfg.toolbox, kubeconfig.as_deref())? else {
        return Ok(());
    };
    let args = internal_kind_kubeconfig_args(&cfg.repo.bootstrap.kind_cluster);
    let contents = capture("kind", &args).await?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to chmod 600 {}", path.display()))?;
    }
    Ok(())
}

/// Ensure the kind cluster exists and is healthy.
///
/// Default: reuse an existing cluster after validating that its context is
/// reachable and all nodes go Ready; this makes reruns non-destructive and
/// lets a partially failed bootstrap resume. With --recreate (or when no
/// cluster exists) the cluster is (re)built from the rendered config.
async fn ensure_kind_cluster(cfg: &Config, engine: &str, engine_sock: &str) -> Result<()> {
    let kind_cluster = &cfg.repo.bootstrap.kind_cluster;
    let kind_context = &cfg.repo.bootstrap.kind_context;
    let clusters = capture_lossy("kind", &["get", "clusters"]).await;
    let exists = clusters.lines().any(|l| l.trim() == kind_cluster);
    let node_ready_timeout = format!("--timeout={NODE_READY_TIMEOUT}");

    if exists && !cfg.recreate {
        println!(
            ">>> Reusing existing kind cluster '{kind_cluster}' (pass --recreate to replace it)..."
        );
        println!(">>> Validating existing cluster health...");
        {
            // Must precede the kubeconfig selection: the internal endpoint
            // only resolves once the toolbox is on the kind network.
            engine::toolbox_join_kind_network(cfg, engine).await?;
            select_toolbox_kind_kubeconfig(cfg).await?;
        }
        if !run_quiet("kubectl", &["config", "use-context", kind_context]).await {
            // kind get clusters can report the cluster while the kubeconfig
            // lacks the context (interrupted first create, pruned
            // kubeconfig). Recover it instead of demanding a destructive
            // --recreate.
            println!(">>> Context '{kind_context}' missing from kubeconfig; exporting it...");
            if !run_quiet("kind", &["export", "kubeconfig", "--name", kind_cluster]).await {
                bail!(
                    "failed to export kubeconfig for existing cluster '{kind_cluster}'; rerun with --recreate to replace it"
                );
            }
        }
        if run_quiet("kubectl", &["config", "use-context", kind_context]).await
            && run_quiet(
                "kubectl",
                &[
                    "wait",
                    "--for=condition=Ready",
                    "node",
                    "--all",
                    &node_ready_timeout,
                ],
            )
            .await
        {
            println!(">>> Existing cluster '{kind_cluster}' is healthy; continuing.");
            return Ok(());
        }
        bail!(
            "existing kind cluster '{kind_cluster}' is not healthy (context unreachable or nodes not Ready within {NODE_READY_TIMEOUT}); rerun with --recreate to replace it"
        );
    }

    if exists {
        println!(">>> Cluster '{kind_cluster}' exists and --recreate was given – recreating...");
        engine::toolbox_leave_kind_network(cfg, engine).await;
        run("kind", &["delete", "cluster", "--name", kind_cluster]).await?;
    }

    println!(">>> Creating kind cluster '{kind_cluster}'...");
    let kind_config = render_kind_config(
        cfg.registry_port,
        engine_sock,
        &cfg.repo.bootstrap.registry_name,
    );
    run_with_stdin(
        "kind",
        &["create", "cluster", "--name", kind_cluster, "--config", "-"],
        &kind_config,
    )
    .await?;
    engine::toolbox_join_kind_network(cfg, engine).await?;
    select_toolbox_kind_kubeconfig(cfg).await?;

    println!(">>> Waiting for cluster node to be ready...");
    // Explicitly switch kubectl to use the kind cluster context.
    run("kubectl", &["config", "use-context", kind_context]).await?;
    run(
        "kubectl",
        &[
            "wait",
            "--for=condition=Ready",
            "node",
            "--all",
            &node_ready_timeout,
        ],
    )
    .await?;
    Ok(())
}

async fn bootstrap_local_registry(
    cfg: &Config,
    engine: &str,
    http: &reqwest::Client,
) -> Result<()> {
    println!(">>> Bootstrapping local container registry...");
    let port = cfg.registry_port;
    let registry_name = &cfg.repo.bootstrap.registry_name;
    let name_filter = format!("name=^{registry_name}$");

    let exists = capture_lossy(
        engine,
        &[
            "ps",
            "-a",
            "--filter",
            &name_filter,
            "--format",
            "{{.Names}}",
        ],
    )
    .await
    .lines()
    .any(|l| l.trim() == registry_name.as_str());

    if !exists {
        println!("    Creating registry container '{registry_name}'...");
        create_registry_container(engine, registry_name, port).await?;
        println!("    Registry created and running: {registry_name}:5000");
    } else {
        // Name match alone proves nothing about configuration. Verify the
        // host-port binding and the kind-network attachment; a stale
        // container bound elsewhere or detached from the network answers
        // the host readiness check and then fails in-cluster.
        let port_matches = extract_published_port(
            &capture_lossy(engine, &["port", registry_name, "5000/tcp"]).await,
        )
        .is_some_and(|p| p.parse::<u16>().is_ok_and(|p| p == port));
        let in_kind_network = capture_lossy(
            engine,
            &[
                "inspect",
                registry_name,
                "--format",
                "{{json .NetworkSettings.Networks}}",
            ],
        )
        .await
        .contains("\"kind\"");
        if !port_matches || !in_kind_network {
            println!("    Existing registry misconfigured (port or network); recreating...");
            run(engine, &["rm", "-f", registry_name]).await?;
            create_registry_container(engine, registry_name, port).await?;
            println!("    Registry recreated: {registry_name}:5000");
        } else {
            let running = capture_lossy(
                engine,
                &["ps", "--filter", &name_filter, "--format", "{{.Names}}"],
            )
            .await
            .lines()
            .any(|l| l.trim() == registry_name.as_str());
            if !running {
                println!("    Restarting stopped registry...");
                capture(engine, &["start", registry_name]).await?;
                println!("    Registry restarted: {registry_name}:5000");
            } else {
                println!("    Registry already running: {registry_name}:5000");
            }
        }
    }

    let (registry_host, probe_port) = cfg.registry_endpoint();
    let registry_url = format!("http://{registry_host}:{probe_port}/v2/");
    println!(">>> Waiting for local registry API at {registry_host}:{probe_port}...");
    // Parity with the script's `curl --fail --retry N --retry-connrefused
    // --retry-delay 1`: one initial attempt plus N retries 1s apart; any
    // response below 400 succeeds (curl --fail's threshold); connection
    // errors (--retry-connrefused) and curl's documented transient
    // statuses (408, 429, 500, 502, 503, 504) are retried; any other
    // answer fails immediately instead of burning the retry budget.
    // The probe address is where THIS process reaches the registry: the
    // kind-network name inside the toolbox, the published port on the host.
    let mut ready = false;
    for attempt in 0..=cfg.registry_ready_retries {
        if let Ok(resp) = http.get(&registry_url).send().await {
            let status = resp.status().as_u16();
            if status < 400 {
                ready = true;
                break;
            }
            if !CURL_TRANSIENT_STATUSES.contains(&status) {
                bail!("local registry at {registry_url} returned {status}; not retrying");
            }
        }
        // Connection errors fall through and are retried (--retry-connrefused).
        if attempt < cfg.registry_ready_retries {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    if !ready {
        bail!("local registry did not become ready at {registry_url}");
    }

    println!(">>> Publishing initial OCI artifact from the local Git checkout...");
    // Forward the resolved values: the mise oci-push task reads them from
    // its own environment, so they must not stop at this boundary.
    // REGISTRY_HOST reroutes the task's own registry probe and push from
    // the published localhost port to the kind-network name (toolbox runs).
    let (registry_host, oci_port) = cfg.registry_endpoint();
    let status = Command::new("mise")
        .args(["-E", &cfg.profile, "run", "oci-push"])
        .env("REGISTRY_PORT", oci_port.to_string())
        .env("REGISTRY_HOST", &registry_host)
        .env("OCI_REPOSITORY", &cfg.oci_repository)
        .env("OCI_TAG", &cfg.oci_tag)
        .status()
        .await
        .with_context(|| "failed to spawn 'mise'")?;
    if !status.success() {
        bail!(
            "'mise -E {} run oci-push' failed with {status}",
            cfg.profile
        );
    }
    println!(
        ">>> Initial OCI artifact is available at oci://localhost:{port}/{repo}:{tag}",
        repo = cfg.oci_repository,
        tag = cfg.oci_tag
    );
    Ok(())
}

/// Create the local registry container on the kind network, publishing
/// 5000 on the host loopback at the configured port.
async fn create_registry_container(engine: &str, registry_name: &str, port: u16) -> Result<()> {
    let publish = format!("127.0.0.1:{port}:5000");
    capture(
        engine,
        &[
            "run",
            "-d",
            "--name",
            registry_name,
            "--network",
            "kind",
            "-p",
            &publish,
            "registry:2",
        ],
    )
    .await?;
    Ok(())
}

async fn install_flux_operator(repo: &BootstrapConfig, registry_config: &Path) -> Result<()> {
    println!(">>> Installing Flux Operator...");
    let cfg = registry_config.to_string_lossy().into_owned();
    let flux_ns = &repo.bootstrap.flux_namespace;
    let chart_version = repo
        .charts
        .get("flux-operator")
        .context("charts.flux-operator missing from bootstrap.toml")?;
    // upgrade --install (script used install): rerun-safe after a partial failure.
    // --version pins the chart so reruns cannot resolve a different release
    // (the chart pin lives in bootstrap.toml [charts]; the Python
    // cross-check keeps it from drifting into the Git manifests).
    let args: Vec<String> = vec![
        "upgrade".into(),
        "--install".into(),
        "flux-operator".into(),
        "oci://ghcr.io/controlplaneio-fluxcd/charts/flux-operator".into(),
        "--version".into(),
        chart_version.into(),
        "--namespace".into(),
        flux_ns.into(),
        "--create-namespace".into(),
        "--wait".into(),
        "--timeout".into(),
        "10m".into(),
        "--registry-config".into(),
        cfg,
    ];
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run("helm", &arg_refs).await
}

async fn install_flux_instance(cfg: &Config, registry_config: &Path) -> Result<bool> {
    println!(">>> Installing FluxInstance via Helm...");
    let mut controllers_ready = true;
    let registry_cfg = registry_config.to_string_lossy().into_owned();
    let flux_ns = &cfg.repo.bootstrap.flux_namespace;
    let chart_version = cfg
        .repo
        .charts
        .get("flux-operator")
        .context("charts.flux-operator missing from bootstrap.toml")?;
    let args: Vec<String> = vec![
        "upgrade".into(),
        "--install".into(),
        "flux".into(),
        "oci://ghcr.io/controlplaneio-fluxcd/charts/flux-instance".into(),
        "--version".into(),
        chart_version.into(),
        "--namespace".into(),
        flux_ns.into(),
        // Helm 4's watcher strategy treats the FluxInstance's transient
        // InProgress condition as a terminal failure. Use the legacy
        // chart-resource wait here, then wait explicitly for the
        // operator-owned Ready condition below.
        "--wait=legacy".into(),
        "--timeout".into(),
        "10m".into(),
        "--registry-config".into(),
        registry_cfg,
        "--set".into(),
        "instance.cluster.type=kubernetes".into(),
        "--set".into(),
        "instance.cluster.size=small".into(),
        "--set".into(),
        "instance.cluster.multitenant=false".into(),
        "--set".into(),
        "instance.cluster.networkPolicy=true".into(),
        "--set".into(),
        "instance.cluster.domain=cluster.local".into(),
        // The local registry is insecure: the artifact is published to it
        // from this host and never leaves it.
        "--set".into(),
        "instance.sync.kind=OCIRepository".into(),
        "--set".into(),
        format!(
            "instance.sync.url=oci://{}:5000/{}",
            cfg.repo.bootstrap.registry_name,
            cfg.oci_repository
        ),
        "--set".into(),
        format!("instance.sync.ref={}", cfg.oci_tag),
        "--set".into(),
        format!("instance.sync.path={}", cfg.environment.sync_path),
        "--set-json".into(),
        r#"instance.kustomize.patches=[{"patch":"- op: add\n  path: /spec/insecure\n  value: true","target":{"kind":"OCIRepository"}}]"#.into(),
    ];

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run("helm", &arg_refs).await?;

    println!(">>> Waiting for FluxInstance reconciliation to complete...");
    run(
        "kubectl",
        &kubectl_cmd(
            None,
            &[
                "wait",
                "fluxinstance/flux",
                "--namespace",
                flux_ns,
                "--for=condition=Ready",
                "--timeout=10m",
            ],
        ),
    )
    .await?;

    // Verify the Flux controllers are running before declaring success.
    // The script tolerated this wait failing (`|| true`); keep the
    // tolerance but make the failure visible and stop short of the plain
    // completion message, which would otherwise report success over
    // ImagePullBackOff'd controllers.
    println!(">>> Waiting for Flux controllers to be ready...");
    if run(
        "kubectl",
        &kubectl_cmd(
            None,
            &[
                "wait",
                "--namespace",
                flux_ns,
                "--for=condition=ready",
                "pod",
                "--selector=app.kubernetes.io/part-of=flux",
                "--timeout=90s",
            ],
        ),
    )
    .await
    .is_err()
    {
        eprintln!("WARNING: not all Flux controllers became ready within 90s:");
        let _ = run(
            "kubectl",
            &kubectl_cmd(None, &["get", "pods", "--namespace", flux_ns]),
        )
        .await;
        controllers_ready = false;
    }
    Ok(controllers_ready)
}

/// Wait for the whole reconciled tree to go Ready.
///
/// The FluxInstance operator generates a root Kustomization from the
/// FluxInstance spec; that root reconciles `sync.path` and creates the
/// tree's Flux Kustomizations (workload/local-host/flux-ks.yaml:
/// local-ai-platform-crd -> local-ai-platform). Waiting for EVERY
/// Kustomization in the Flux namespace to be Ready is the terminal state:
/// it covers the root and every child, so the watch stays generic over
/// what the tree declares.
async fn watch_local_reconciliation(cfg: &Config) -> Result<()> {
    println!();
    println!(">>> Flux reconciliation progress");

    let flux_ns = &cfg.repo.bootstrap.flux_namespace;

    // The root Kustomization appears once the operator has reconciled the
    // FluxInstance. Poll a bounded `kubectl wait` (it exits 0 only when at
    // least one Kustomization in the namespace is Ready); this is a fast,
    // actionable failure for a broken sync source before the long readiness
    // wait below.
    let mut ks_ready = false;
    for _ in 0..60 {
        if run_quiet(
            "kubectl",
            &[
                "wait",
                "kustomization",
                "--all",
                "--namespace",
                flux_ns,
                "--for=condition=Ready",
                "--timeout=2s",
            ],
        )
        .await
        {
            ks_ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    if !ks_ready {
        eprintln!("ERROR: no Flux Kustomization was Ready within 2 minutes");
        let _ = run("flux", &["get", "kustomizations"]).await;
        bail!("Flux Kustomization missing");
    }

    // Stream Flux errors in the bootstrap terminal while we wait.
    let flux_logs = Command::new("flux")
        .args([
            "logs",
            "--all-namespaces",
            "--follow",
            "--level=error",
            "--since=10m",
        ])
        .spawn()
        .context("failed to spawn 'flux logs'")?;
    let _log_guard = ChildGuard(flux_logs);

    println!(">>> Waiting for the AI platform to be ready...");
    let timeout_arg = format!("--timeout={}", cfg.local_reconcile_timeout);
    if run(
        "kubectl",
        &kubectl_cmd(
            None,
            &[
                "wait",
                "kustomization",
                "--all",
                "--namespace",
                flux_ns,
                "--for=condition=Ready",
                &timeout_arg,
            ],
        ),
    )
    .await
    .is_err()
    {
        eprintln!(
            "ERROR: reconciliation did not complete within {}",
            cfg.local_reconcile_timeout
        );
        let _ = run("flux", &["get", "kustomizations"]).await;
        bail!("reconciliation timed out");
    }
    // _log_guard drops here: the flux logs follower is killed.
    Ok(())
}

/// Write the exported host kubeconfig: `kind get kubeconfig` (the published
/// localhost endpoint), chmod 600. kind publishes the API port on localhost,
/// so the file works from the host with no endpoint rewrite; in the toolbox
/// the write lands in the mounted `.kube/` directory and persists into the
/// checkout.
async fn write_kubeconfig(cfg: &Config) -> Result<()> {
    let kind_cluster = &cfg.repo.bootstrap.kind_cluster;
    let out = &cfg.kubeconfig;
    let contents = capture("kind", &["get", "kubeconfig", "--name", kind_cluster]).await?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(out, &contents).with_context(|| format!("failed to write {}", out.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to chmod 600 {}", out.display()))?;
    }
    println!(">>> Kubeconfig written to {}", out.display());
    Ok(())
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Must run before any engine probe below, in either subcommand branch.
    engine::ensure_toolbox_container_host(
        std::env::var("KAIPR_TOOLBOX").is_ok_and(|value| value == "1"),
    );

    // Load the repository config first: profile names, defaults, and chart
    // pins all come from it (BOOTSTRAP_CONFIG overrides the location).
    let repo = BootstrapConfig::locate_and_load()?;
    let cli = Cli::parse();

    // The teardown subcommand resolves only its own knob subset.
    if let Some(SubCommand::Teardown { .. }) = &cli.command {
        let cfg = Config::load_teardown(&cli, repo)?;
        return teardown::run_teardown(&cfg).await;
    }

    let cfg = Config::load(&cli, repo)?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("kaipr-bootstrap/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    // Parity with the script: cleanup runs on the normal exit and error
    // paths (temp files drop with their owners, the flux logs follower is
    // killed by its guard). Complete signal-driven cancellation is deferred
    // to a focused follow-up rather than shipped half-implemented here.
    run_bootstrap(&cfg, &http).await
}

async fn run_bootstrap(cfg: &Config, http: &reqwest::Client) -> Result<()> {
    let preflight = preflight_checks(cfg).await?;
    println!(
        ">>> Using container engine: {} (socket: {})",
        preflight.engine, preflight.engine_sock
    );

    // Step 1: ensure the kind cluster (reuse by default; --recreate replaces).
    ensure_kind_cluster(cfg, &preflight.engine, &preflight.engine_sock).await?;

    // Step 2: bootstrap the local container registry and publish the artifact.
    if cfg.environment.sync == SyncSource::Oci {
        bootstrap_local_registry(cfg, &preflight.engine, http).await?;
    }

    // Anonymous registry config shared by both helm installs; the temp file is
    // removed automatically when it drops at the end of main.
    let mut registry_config =
        tempfile::NamedTempFile::new().context("failed to create temp registry config")?;
    {
        use std::io::Write;
        writeln!(registry_config, "{{}}")?;
        registry_config.flush()?;
    }

    // Step 3: install the Flux Operator.
    install_flux_operator(&cfg.repo, registry_config.path()).await?;

    // Step 4: install the FluxInstance via Helm.
    let controllers_ready = install_flux_instance(cfg, registry_config.path()).await?;

    // Step 5: watch reconciliation.
    if cfg.environment.sync == SyncSource::Oci {
        watch_local_reconciliation(cfg).await?;
    }

    // Step 6: export the host kubeconfig.
    write_kubeconfig(cfg).await?;

    // Done. Everything else is driven by GitOps.
    println!();
    if !controllers_ready {
        println!(
            ">>> Bootstrap finished WITH WARNINGS: Flux controllers were not all ready; \
             check 'kubectl -n flux-system get pods' before relying on the cluster"
        );
    }
    let registry_name = &cfg.repo.bootstrap.registry_name;
    println!(">>> Bootstrap complete: Flux is reconciling from the local OCI artifact");
    println!(
        ">>> Local registry: localhost:{port} (cluster endpoint: {registry_name}:5000)",
        port = cfg.registry_port
    );
    println!(
        ">>> OCI source: oci://{registry_name}:5000/{repo}:{tag} (path: {})",
        cfg.environment.sync_path,
        repo = cfg.oci_repository,
        tag = cfg.oci_tag
    );
    println!(">>> Kubeconfig: {}", cfg.kubeconfig.display());
    println!(">>> Watch progress with: flux get kustomizations --watch");
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// The repository's checked-in bootstrap.toml, as tests' base config.
    fn repo_config() -> BootstrapConfig {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        BootstrapConfig::load_from(&root.join("../bootstrap.toml")).unwrap()
    }

    /// The multi-environment fixture: engine-semantics tests (profile
    /// precedence, file-order error messages) run against it.
    fn full_repo_config() -> BootstrapConfig {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        BootstrapConfig::load_from(&root.join("testdata/bootstrap-full.toml")).unwrap()
    }

    fn config_from(
        repo: &BootstrapConfig,
        cli: &Cli,
        get: impl Fn(&str) -> Option<String>,
    ) -> Config {
        Config::from_env(cli, (*repo).clone(), get, true).unwrap()
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_rejects_unknown_profile() {
        // Positional profile names are validated against bootstrap.toml at
        // resolution time (they name [environments.*] sections), not by clap.
        let cli = Cli::try_parse_from(["kaipr-bootstrap", "bogus"]).unwrap();
        assert!(Config::from_env(&cli, repo_config(), |_| None, true).is_err());
    }

    #[test]
    fn cli_rejects_deferred_flags() {
        // The expanded flag interface is deferred (review follow-up): the
        // CLI accepts only the positional profile and --recreate.
        for rejected in [
            ["kaipr-bootstrap", "--registry-port", "5500"],
            ["kaipr-bootstrap", "--oci-tag", "dev"],
            ["kaipr-bootstrap", "--container-engine", "docker"],
            ["kaipr-bootstrap", "--kubeconfig", "/tmp/x.yaml"],
            // Teardown knobs are env-only too (CONTAINER_ENGINE,
            // KAIPR_KUBECONFIG), matching teardown.sh's interface.
            ["kaipr-bootstrap", "teardown", "--force-kind-delete"],
            ["kaipr-bootstrap", "teardown", "--aws-only"],
        ] {
            assert!(
                Cli::try_parse_from(rejected).is_err(),
                "{rejected:?} should not parse"
            );
        }
    }

    #[test]
    fn config_defaults_match_script() {
        let repo = repo_config();

        let cfg = config_from(
            &repo,
            &Cli::try_parse_from(["kaipr-bootstrap", "local-host"]).unwrap(),
            |_| None,
        );
        assert_eq!(cfg.profile, "local-host");
        assert!(!cfg.recreate);
        assert_eq!(cfg.registry_port, 5001);
        assert_eq!(cfg.registry_ready_retries, 120);
        assert_eq!(cfg.local_reconcile_timeout, "15m");
        assert_eq!(cfg.oci_repository, "kaipr");
        assert_eq!(cfg.oci_tag, "latest");
        assert!(cfg.container_engine.is_none());
        assert_eq!(cfg.kubeconfig, default_kubeconfig().unwrap());
        assert_eq!(cfg.registry_endpoint(), ("localhost".into(), 5001));
        // Values now sourced from the repository config.
        assert_eq!(cfg.environment.sync, SyncSource::Oci);
        assert_eq!(cfg.environment.sync_path, "workload/local-host");
        assert_eq!(cfg.repo.bootstrap.kind_cluster, "kaipr");
        assert_eq!(cfg.repo.bootstrap.kind_context, "kind-kaipr");
        assert_eq!(cfg.repo.bootstrap.registry_name, "kaipr-registry");
        assert_eq!(cfg.repo.bootstrap.flux_namespace, "flux-system");
        assert_eq!(cfg.repo.charts["flux-operator"], "0.61.0");
    }

    #[test]
    fn config_reads_env_knobs_with_script_semantics() {
        let cli = Cli::try_parse_from(["kaipr-bootstrap"]).unwrap();
        let get = |name: &str| -> Option<String> {
            match name {
                "REGISTRY_PORT" => Some("5500".into()),
                "LOCAL_RECONCILE_TIMEOUT" => Some("30m".into()),
                "OCI_TAG" => Some("dev".into()),
                "KAIPR_KUBECONFIG" => Some("/tmp/kc.yaml".into()),
                _ => None,
            }
        };
        let cfg = Config::from_env(&cli, full_repo_config(), get, true).unwrap();
        assert_eq!(cfg.profile, "local-host"); // no env, no positional
        assert_eq!(cfg.registry_port, 5500);
        assert_eq!(cfg.local_reconcile_timeout, "30m");
        assert_eq!(cfg.oci_tag, "dev");
        assert_eq!(cfg.kubeconfig, PathBuf::from("/tmp/kc.yaml"));
    }

    #[test]
    fn config_reads_toolbox_runtime_knobs() {
        let repo = repo_config();

        let cli = Cli::try_parse_from(["kaipr-bootstrap", "local-host"]).unwrap();
        let cfg = config_from(&repo, &cli, |name| match name {
            "KAIPR_TOOLBOX" => Some("1".into()),
            "ENGINE_SOCK" => Some("/run/user/501/podman/podman.sock".into()),
            _ => None,
        });

        assert!(cfg.toolbox);
        assert_eq!(
            cfg.engine_sock.as_deref(),
            Some("/run/user/501/podman/podman.sock")
        );
        assert_eq!(cfg.registry_endpoint(), ("kaipr-registry".into(), 5000));
    }

    #[test]
    fn host_runtime_uses_published_port() {
        let repo = repo_config();

        let cfg = config_from(
            &repo,
            &Cli::try_parse_from(["kaipr-bootstrap", "local-host"]).unwrap(),
            |_| None,
        );

        assert!(!cfg.toolbox);
        assert!(cfg.engine_sock.is_none());
        assert_eq!(cfg.registry_endpoint(), ("localhost".into(), 5001));
    }

    #[test]
    fn sync_source_is_config_driven() {
        // The FluxInstance sync source comes from bootstrap.toml
        // [environments.*], not from the profile name. The repository
        // ships the OCI source only.
        for name in ["local-host", "dev-laptop"] {
            assert_eq!(
                full_repo_config().environment(name).unwrap().sync,
                SyncSource::Oci,
                "{name} must declare sync = \"oci\""
            );
        }
    }

    #[test]
    fn required_tools_match_the_sync_surface() {
        let local = required_tools(full_repo_config().environment("local-host").unwrap());
        assert_eq!(
            local,
            vec!["kind", "helm", "kubectl", "mise", "flux", "curl"]
        );
    }

    #[test]
    fn toolbox_kind_kubeconfig_uses_one_explicit_file() {
        assert_eq!(
            toolbox_kubeconfig_path(true, Some("/state/kind.yaml")).unwrap(),
            Some(PathBuf::from("/state/kind.yaml"))
        );
        assert!(toolbox_kubeconfig_path(true, None)
            .unwrap_err()
            .to_string()
            .contains("KUBECONFIG"));
        assert!(toolbox_kubeconfig_path(true, Some("/a:/b"))
            .unwrap_err()
            .to_string()
            .contains("single file"));
        assert_eq!(toolbox_kubeconfig_path(false, None).unwrap(), None);
    }

    #[test]
    fn internal_kind_kubeconfig_command_targets_the_named_cluster() {
        assert_eq!(
            internal_kind_kubeconfig_args("kaipr"),
            ["get", "kubeconfig", "--internal", "--name", "kaipr"]
        );
    }

    #[test]
    fn config_rejects_invalid_ready_timeout() {
        let cli = Cli::try_parse_from(["kaipr-bootstrap", "local-host"]).unwrap();
        let err = Config::from_env(
            &cli,
            repo_config(),
            |name| match name {
                "LOCAL_RECONCILE_TIMEOUT" => Some("abc".into()),
                _ => None,
            },
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("LOCAL_RECONCILE_TIMEOUT"));
    }

    #[test]
    fn config_rejects_non_numeric_registry_port() {
        let cli = Cli::try_parse_from(["kaipr-bootstrap", "local-host"]).unwrap();
        let err = Config::from_env(
            &cli,
            repo_config(),
            |name| match name {
                "REGISTRY_PORT" => Some("not-a-port".into()),
                _ => None,
            },
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("REGISTRY_PORT"));
    }

    #[test]
    fn duration_parsing_matches_script_arithmetic() {
        assert_eq!(parse_duration_seconds("40m").unwrap(), 2400);
        assert_eq!(parse_duration_seconds("15m").unwrap(), 900);
        assert_eq!(parse_duration_seconds("90s").unwrap(), 90);
        assert_eq!(parse_duration_seconds("2h").unwrap(), 7200);
        assert_eq!(parse_duration_seconds("1200").unwrap(), 1200); // bare seconds
        assert_eq!(parse_duration_seconds(" 30m ").unwrap(), 1800); // trimmed
        for bad in ["", "m", "40x", "4-0m", "forty"] {
            assert!(parse_duration_seconds(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn environment_resolution_matches_script_precedence() {
        let repo = full_repo_config();
        // Non-empty env wins over the positional, like ${KAIPR_PROFILE:-${1:-local-host}}.
        assert_eq!(
            resolve_environment(Some("dev-laptop"), Some("local-host"), &repo).unwrap(),
            "dev-laptop"
        );
        assert_eq!(
            resolve_environment(Some("local-host"), Some("dev-laptop"), &repo).unwrap(),
            "local-host"
        );
        // Empty env falls through to the positional, then the config's
        // default-environment (local-host).
        assert_eq!(
            resolve_environment(Some(""), Some("dev-laptop"), &repo).unwrap(),
            "dev-laptop"
        );
        assert_eq!(
            resolve_environment(Some(""), None, &repo).unwrap(),
            "local-host"
        );
        assert_eq!(
            resolve_environment(None, Some("dev-laptop"), &repo).unwrap(),
            "dev-laptop"
        );
        assert_eq!(
            resolve_environment(None, None, &repo).unwrap(),
            "local-host"
        );
        // Unknown env values fail with the script's error message.
        assert_eq!(
            resolve_environment(Some("bogus"), Some("local-host"), &repo)
                .unwrap_err()
                .to_string(),
            "unsupported profile 'bogus' (expected 'local-host' or 'dev-laptop')"
        );
    }

    #[test]
    fn cli_accepts_recreate_flag() {
        let cli = Cli::try_parse_from(["kaipr-bootstrap", "local-host", "--recreate"]).unwrap();
        assert!(cli.recreate);
    }

    #[test]
    fn extract_published_port_parses_engine_output() {
        assert_eq!(
            extract_published_port("0.0.0.0:32771\n[::]:32771\n").as_deref(),
            Some("32771")
        );
        assert_eq!(
            extract_published_port("127.0.0.1:6443\n").as_deref(),
            Some("6443")
        );
        assert_eq!(extract_published_port(""), None);
        assert_eq!(extract_published_port("garbage\n"), None);
        assert_eq!(extract_published_port("0.0.0.0:\n"), None);
    }

    #[test]
    fn kind_config_includes_registry_patch_and_socket_mount() {
        let local = render_kind_config(5001, "/var/run/docker.sock", "kaipr-registry");
        assert!(local.contains("containerdConfigPatches"));
        assert!(local.contains("localhost:5001"));
        assert!(local.contains("http://kaipr-registry:5000"));
        assert!(local.contains("hostPath: /var/run/docker.sock"));
        assert!(local.contains("kind: Cluster"));
        assert!(local.contains("role: control-plane"));
    }
}
