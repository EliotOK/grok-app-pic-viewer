//! Remote mirror host — phone browser becomes a live UI of the desktop host.
//!
//! See `DESIGN.md` and `PLAN.md` Wave 2.

mod auth;
mod http;
mod lan;
mod preferences;
mod rpc;
mod tunnel;
mod ws;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

pub use auth::generate_token;

use crate::session_manager::SessionManager;

/// Default concurrent phone WebSocket clients (SEC-08c). Overridable via env / panel.
pub const DEFAULT_MAX_CLIENTS: u32 = 4;
/// Hard upper clamp for max clients (avoid unbounded fan-out).
pub const MAX_CLIENTS_CAP: u32 = 16;
/// Minimum concurrent clients setting.
pub const MIN_CLIENTS: u32 = 1;

/// Clamp user/env max-clients to a safe range.
pub fn normalize_max_clients(raw: u32) -> u32 {
    raw.clamp(MIN_CLIENTS, MAX_CLIENTS_CAP)
}

/// Environment contract (DESIGN §12).
#[derive(Debug, Clone)]
pub struct MirrorEnvConfig {
    /// `GROK_MIRROR_HEADLESS=1` → auto-start after app setup.
    pub headless: bool,
    /// Fixed token when set (`GROK_MIRROR_TOKEN`).
    pub token: Option<String>,
    /// Fixed loopback port when set (`GROK_MIRROR_PORT`).
    pub port: Option<u16>,
    /// Skip cloudflared when true (`GROK_MIRROR_NO_TUNNEL=1`).
    pub no_tunnel: bool,
    /// Bind `0.0.0.0` when true (`GROK_MIRROR_ALLOW_LAN=1`). Default loopback.
    pub allow_lan: bool,
    /// Override static SPA root (`GROK_MIRROR_DIST`).
    pub dist: Option<PathBuf>,
    /// Concurrent WS client cap (`GROK_MIRROR_MAX_CLIENTS`, default 4).
    pub max_clients: u32,
}

impl MirrorEnvConfig {
    pub fn from_env() -> Self {
        let headless = env_truthy("GROK_MIRROR_HEADLESS");
        let token = std::env::var("GROK_MIRROR_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let port = std::env::var("GROK_MIRROR_PORT")
            .ok()
            .and_then(|s| s.trim().parse::<u16>().ok())
            .filter(|&p| p > 0);
        let no_tunnel = env_truthy("GROK_MIRROR_NO_TUNNEL");
        let allow_lan = env_truthy("GROK_MIRROR_ALLOW_LAN");
        let dist = std::env::var("GROK_MIRROR_DIST")
            .ok()
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| !p.as_os_str().is_empty());
        let max_clients = std::env::var("GROK_MIRROR_MAX_CLIENTS")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map(normalize_max_clients)
            .unwrap_or(DEFAULT_MAX_CLIENTS);
        Self {
            headless,
            token,
            port,
            no_tunnel,
            allow_lan,
            dist,
            max_clients,
        }
    }
}

fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

/// Lifecycle phase for Connect panel / status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorPhase {
    Stopped,
    Starting,
    /// Local HTTP up; tunnel skipped or not yet ready.
    Local,
    WaitingTunnel,
    Live,
    /// Local host still running; cloudflared child exited (do not auto-restart).
    TunnelDead,
    Error,
}

/// Public status returned by `mirror_status` / start.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorStatus {
    pub running: bool,
    pub public_url: Option<String>,
    pub local_port: Option<u16>,
    /// Full token for QR / copy while host is running. Prefer publicUrl for display.
    /// Desktop pairing secret; never include in localStorage or support bundles.
    pub token: Option<String>,
    /// Last 6 chars of token for safe display in logs / status chips.
    pub token_tail: Option<String>,
    pub clients: u32,
    /// Concurrent WebSocket client cap (1–16, default 4).
    pub max_clients: u32,
    pub phase: MirrorPhase,
    pub error: Option<String>,
    /// When true, mirror RPC rejects write methods (send / permissions / create).
    pub read_only: bool,
    /// When true, HTTP listens on all interfaces (`0.0.0.0`) so same-LAN phones can connect.
    pub allow_lan: bool,
    /// Copy/QR URL using the detected LAN IPv4. None when LAN is off or no IPv4 found.
    pub lan_url: Option<String>,
}

struct Runtime {
    token: String,
    port: u16,
    phase: MirrorPhase,
    public_url: Option<String>,
    error: Option<String>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// cloudflared adapter handle (host process or Docker container); stopped on teardown.
    tunnel: Option<tunnel::TunnelHandle>,
    #[allow(dead_code)] // kept for future media/static re-resolve
    dist_dir: PathBuf,
    read_only: bool,
    allow_lan: bool,
    /// Cached default-route IPv4 for copy/QR (None = undetected).
    lan_ip: Option<Ipv4Addr>,
}

/// Host-side handles for RPC (set in app setup / mirror_start).
struct HostCtx {
    app: AppHandle,
    mgr: Arc<SessionManager>,
}

struct Inner {
    env: MirrorEnvConfig,
    runtime: Option<Runtime>,
    ctx: Option<HostCtx>,
    /// Default for new starts; also applied to live runtime via set_read_only.
    read_only: bool,
    /// Concurrent WS client cap (panel + env). Survives stop/start in-process.
    max_clients: u32,
    /// LAN bind preference (panel + env). Default false (loopback).
    allow_lan: bool,
}

/// Process-wide mirror host with desktop-owned pairing preferences.
pub struct MirrorHost {
    inner: Mutex<Inner>,
    hub: Arc<ws::WsHub>,
    preferences_path: Option<PathBuf>,
}

impl MirrorHost {
    pub fn from_env() -> Self {
        let env = MirrorEnvConfig::from_env();
        let max_clients = env.max_clients;
        let allow_lan = env.allow_lan;
        Self {
            inner: Mutex::new(Inner {
                env,
                runtime: None,
                ctx: None,
                // Safe default: phone can observe until the user opts into write access.
                read_only: true,
                max_clients,
                allow_lan,
            }),
            hub: Arc::new(ws::WsHub::new()),
            preferences_path: None,
        }
    }

    pub fn from_persisted_env() -> Result<Self, String> {
        Self::with_preferences(crate::paths::app_data_root().join("mirror-host.json"))
    }

    fn with_preferences(path: PathBuf) -> Result<Self, String> {
        let saved = preferences::load(&path)?;
        let mut host = Self::from_env();
        {
            let mut g = host.inner.lock();
            g.env.headless |= saved.enabled;
            if g.env.token.is_none() {
                g.env.token = saved.token;
            }
            if g.env.port.is_none() {
                g.env.port = Some(saved.port.unwrap_or(63867));
            }
            if std::env::var_os("GROK_MIRROR_ALLOW_LAN").is_none() {
                g.allow_lan = saved.allow_lan;
                g.env.allow_lan = saved.allow_lan;
            }
            if std::env::var_os("GROK_MIRROR_MAX_CLIENTS").is_none() {
                g.max_clients = saved.max_clients;
                g.env.max_clients = saved.max_clients;
            }
            g.read_only = saved.read_only;
        }
        host.preferences_path = Some(path);
        Ok(host)
    }

    fn preferences(g: &Inner) -> preferences::Preferences {
        preferences::Preferences {
            enabled: g.env.headless,
            read_only: g.read_only,
            allow_lan: g.allow_lan,
            max_clients: g.max_clients,
            token: g.env.token.clone(),
            port: g.env.port,
        }
    }

    fn save_preferences(&self, saved: &preferences::Preferences) -> Result<(), String> {
        if let Some(path) = &self.preferences_path {
            preferences::save(path, saved)?;
        }
        Ok(())
    }

    pub fn hub(&self) -> Arc<ws::WsHub> {
        self.hub.clone()
    }

    /// Attach AppHandle + SessionManager so WS RPC can call domain logic.
    pub fn attach(&self, app: AppHandle, mgr: Arc<SessionManager>) {
        let mut g = self.inner.lock();
        g.ctx = Some(HostCtx { app, mgr });
    }

    pub fn rpc_ctx(&self) -> Option<(AppHandle, Arc<SessionManager>)> {
        let g = self.inner.lock();
        g.ctx.as_ref().map(|c| (c.app.clone(), c.mgr.clone()))
    }

    /// Fan-out a session event to all mirror WS clients (payload as JSON).
    pub fn broadcast(&self, name: &str, payload: &Value) {
        self.hub.broadcast(name, payload);
    }

    pub fn env(&self) -> MirrorEnvConfig {
        self.inner.lock().env.clone()
    }

    pub fn active_token(&self) -> Option<String> {
        self.inner.lock().runtime.as_ref().map(|r| r.token.clone())
    }

    pub fn is_read_only(&self) -> bool {
        let g = self.inner.lock();
        g.runtime
            .as_ref()
            .map(|r| r.read_only)
            .unwrap_or(g.read_only)
    }

    pub fn max_clients(&self) -> u32 {
        self.inner.lock().max_clients
    }

    /// True when connected WS clients are already at the configured cap.
    pub fn is_at_client_limit(&self) -> bool {
        let max = self.max_clients();
        self.hub.client_count() as u32 >= max
    }

    pub fn set_max_clients(&self, max_clients: u32) -> Result<u32, String> {
        let next = normalize_max_clients(max_clients);
        let mut g = self.inner.lock();
        let mut saved = Self::preferences(&g);
        saved.max_clients = next;
        self.save_preferences(&saved)?;
        let prev = g.max_clients;
        g.max_clients = next;
        g.env.max_clients = next;
        if prev != next {
            // No tokens / URLs — numeric policy only.
            tracing::info!(
                max_clients = next,
                prev,
                "mirror: max concurrent clients updated"
            );
        }
        Ok(next)
    }

    pub fn set_read_only(&self, read_only: bool) -> Result<(), String> {
        let mut g = self.inner.lock();
        let mut saved = Self::preferences(&g);
        saved.read_only = read_only;
        self.save_preferences(&saved)?;
        let prev = g
            .runtime
            .as_ref()
            .map(|r| r.read_only)
            .unwrap_or(g.read_only);
        g.read_only = read_only;
        if let Some(r) = g.runtime.as_mut() {
            r.read_only = read_only;
        }
        // Audit line for support bundles / app.log — no tokens or URLs.
        if prev != read_only {
            tracing::info!(
                read_only,
                allow_write = !read_only,
                "mirror: phone write access toggled"
            );
        }
        Ok(())
    }

    /// Opt-in LAN bind. Loopback is the default. Rebinds HTTP when already running.
    pub async fn set_allow_lan(self: &Arc<Self>, allow_lan: bool) -> Result<MirrorStatus, String> {
        let need_rebind = {
            let mut g = self.inner.lock();
            let prev = g.allow_lan;
            g.allow_lan = allow_lan;
            g.env.allow_lan = allow_lan;
            if let Some(r) = g.runtime.as_mut() {
                r.allow_lan = allow_lan;
            }
            if prev != allow_lan {
                tracing::info!(allow_lan, "mirror: LAN bind toggled");
            }
            g.runtime.is_some() && prev != allow_lan
        };
        if need_rebind {
            self.rebind_http().await?;
        } else {
            self.refresh_access_urls();
        }
        self.save_preferences(&Self::preferences(&self.inner.lock()))?;
        Ok(self.status())
    }

    fn refresh_access_urls(&self) {
        let mut g = self.inner.lock();
        let Some(r) = g.runtime.as_mut() else {
            return;
        };
        if r.allow_lan {
            if r.lan_ip.is_none() {
                r.lan_ip = lan::detect_lan_ipv4();
            }
        } else {
            r.lan_ip = None;
        }
        let local = lan::local_access_url(r.allow_lan, r.port, &r.token, r.lan_ip);
        if r.phase != MirrorPhase::Live {
            r.public_url = Some(local);
        }
    }

    async fn rebind_http(self: &Arc<Self>) -> Result<(), String> {
        let (shutdown_tx, port, dist_dir, allow_lan) = {
            let mut g = self.inner.lock();
            let Some(r) = g.runtime.as_mut() else {
                return Ok(());
            };
            (
                r.shutdown_tx.take(),
                r.port,
                r.dist_dir.clone(),
                g.allow_lan,
            )
        };
        if let Some(tx) = shutdown_tx {
            let _ = tx.send(());
        }

        let (bound_port, new_shutdown) =
            bind_http_with_retry(self.clone(), port, dist_dir, allow_lan).await?;
        let lan_ip = if allow_lan {
            lan::detect_lan_ipv4()
        } else {
            None
        };

        {
            let mut g = self.inner.lock();
            if let Some(r) = g.runtime.as_mut() {
                r.port = bound_port;
                r.shutdown_tx = Some(new_shutdown);
                r.allow_lan = allow_lan;
                r.lan_ip = lan_ip;
                let local = lan::local_access_url(allow_lan, bound_port, &r.token, lan_ip);
                if r.phase != MirrorPhase::Live {
                    r.public_url = Some(local);
                }
            }
        }
        tracing::info!(port = bound_port, allow_lan, "mirror http rebound");
        Ok(())
    }

    /// Mint a new token, drop WS clients, keep the same port/tunnel when possible.
    pub fn rotate_token(&self) -> Result<MirrorStatus, String> {
        let clients_before = self.hub.client_count() as u32;
        let mut g = self.inner.lock();
        if g.runtime.is_none() {
            return Err("mirror is not running".into());
        }
        let next_token = generate_token();
        let mut saved = Self::preferences(&g);
        saved.token = Some(next_token.clone());
        self.save_preferences(&saved)?;
        g.env.token = Some(next_token.clone());
        let r = g.runtime.as_mut().expect("running host");
        r.token = next_token;
        let tail = auth::token_tail(&r.token, 6);
        if let Some(url) = r.public_url.clone() {
            if let Some(base) = url.split("/t/").next() {
                let base = base.trim_end_matches('/');
                if !base.is_empty() {
                    r.public_url = Some(format!("{base}/t/{}/", r.token));
                }
            }
        }
        drop(g);
        self.hub.disconnect_all();
        // Audit: token_tail only — never full token or public URL.
        tracing::info!(
            token_tail = %tail,
            clients_disconnected = clients_before,
            "mirror: token rotated (old QR/sessions invalidated)"
        );
        Ok(self.status())
    }

    pub fn status(&self) -> MirrorStatus {
        let mut g = self.inner.lock();
        // If cloudflared died while we still claim Live, report tunnel_dead (no auto-restart).
        if let Some(r) = g.runtime.as_mut() {
            if r.phase == MirrorPhase::Live {
                if let Some(tun) = r.tunnel.as_mut() {
                    if tun.poll_exited() {
                        r.phase = MirrorPhase::TunnelDead;
                        r.error = Some("cloudflared tunnel process exited".into());
                        tracing::warn!(
                            "mirror tunnel dead — host still up; panel should leave 已上線"
                        );
                    }
                }
            }
        }
        self.status_unlocked(&g)
    }

    /// Start loopback HTTP+WS server (idempotent if already running).
    pub async fn start(self: &Arc<Self>) -> Result<MirrorStatus, String> {
        {
            let g = self.inner.lock();
            if g.runtime.is_some() {
                // Drop lock then status() so dead cloudflared is still detected.
                drop(g);
                return Ok(self.status());
            }
        }

        let env = self.env();
        let token = env
            .token
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(generate_token);
        let prefer_port = env.port.unwrap_or(0);
        let dist_dir = resolve_dist_dir(env.dist.as_ref());

        if !dist_dir.join("index.html").is_file() {
            tracing::warn!(
                path = %dist_dir.display(),
                "mirror dist index.html missing — serving placeholder until built"
            );
        }

        // Mark starting so concurrent status is coherent.
        let allow_lan = {
            let mut g = self.inner.lock();
            let read_only = g.read_only;
            let allow_lan = g.allow_lan;
            g.runtime = Some(Runtime {
                token: token.clone(),
                port: prefer_port,
                phase: MirrorPhase::Starting,
                public_url: None,
                error: None,
                shutdown_tx: None,
                tunnel: None,
                dist_dir: dist_dir.clone(),
                read_only,
                allow_lan,
                lan_ip: None,
            });
            allow_lan
        };

        let (bound_port, shutdown_tx) = match bind_http_with_retry(
            self.clone(),
            prefer_port,
            dist_dir.clone(),
            allow_lan,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                let mut g = self.inner.lock();
                g.runtime = None;
                return Err(e);
            }
        };

        let lan_ip = if allow_lan {
            lan::detect_lan_ipv4()
        } else {
            None
        };
        let local_url = lan::local_access_url(allow_lan, bound_port, &token, lan_ip);

        {
            let mut g = self.inner.lock();
            let mut saved = Self::preferences(&g);
            saved.enabled = true;
            saved.token = Some(token.clone());
            saved.port = Some(bound_port);
            if let Err(e) = self.save_preferences(&saved) {
                drop(g);
                let _ = shutdown_tx.send(());
                self.stop_inner();
                return Err(e);
            }
            g.env.headless = true;
            g.env.token = Some(token.clone());
            g.env.port = Some(bound_port);
            if let Some(r) = g.runtime.as_mut() {
                r.port = bound_port;
                r.phase = if env.no_tunnel {
                    MirrorPhase::Local
                } else {
                    MirrorPhase::WaitingTunnel
                };
                r.public_url = Some(local_url.clone());
                r.shutdown_tx = Some(shutdown_tx);
                r.token = token.clone();
                r.allow_lan = allow_lan;
                r.lan_ip = lan_ip;
            }
        }

        let (phase, public_url, tunnel_handle, error) = if env.no_tunnel {
            tracing::info!(
                port = bound_port,
                "mirror host local-only (GROK_MIRROR_NO_TUNNEL)"
            );
            (MirrorPhase::Local, local_url.clone(), None, None)
        } else {
            match tunnel::start_quick_tunnel(bound_port).await {
                Ok(started) => {
                    let public =
                        format!("{}/t/{}/", started.public_url.trim_end_matches('/'), token);
                    (MirrorPhase::Live, public, Some(started.handle), None)
                }
                Err(e) => {
                    tracing::error!(error = %e, "mirror cloudflared tunnel failed");
                    // Keep local server up so panel can show error + still copy a debug URL.
                    (MirrorPhase::Error, local_url.clone(), None, Some(e))
                }
            }
        };

        {
            let mut g = self.inner.lock();
            if let Some(r) = g.runtime.as_mut() {
                r.phase = phase;
                r.public_url = Some(public_url.clone());
                r.tunnel = tunnel_handle;
                r.error = error;
            }
        }

        // Log phase/port/token_tail only — never the full public URL (embeds token).
        let tail = auth::token_tail(&token, 6);
        tracing::info!(
            port = bound_port,
            token_tail = %tail,
            phase = ?phase,
            no_tunnel = env.no_tunnel,
            allow_lan,
            max_clients = self.max_clients(),
            "mirror host started"
        );

        Ok(self.status())
    }

    /// Full teardown; invalidates token immediately; stops the active tunnel adapter.
    pub async fn stop(&self) -> Result<MirrorStatus, String> {
        {
            let mut g = self.inner.lock();
            let mut saved = Self::preferences(&g);
            saved.enabled = false;
            self.save_preferences(&saved)?;
            g.env.headless = false;
        }
        self.stop_inner();
        // Brief yield so server task can observe shutdown.
        tokio::task::yield_now().await;
        Ok(self.status())
    }

    /// Sync teardown for process exit (no await). Safe from `RunEvent::Exit`.
    pub fn stop_sync(&self) {
        self.stop_inner();
    }

    fn stop_inner(&self) {
        let (shutdown, tunnel) = {
            let mut g = self.inner.lock();
            let (tx, tun) = match g.runtime.take() {
                Some(mut r) => (r.shutdown_tx.take(), r.tunnel.take()),
                None => (None, None),
            };
            (tx, tun)
        };
        self.hub.disconnect_all();
        if let Some(t) = tunnel {
            t.stop();
            tracing::info!("mirror tunnel adapter stopped");
        }
        if let Some(tx) = shutdown {
            let _ = tx.send(());
            tracing::info!("mirror host stop requested (token invalidated)");
        } else {
            tracing::info!("mirror host stop (was not running)");
        }
    }

    fn status_unlocked(&self, g: &Inner) -> MirrorStatus {
        let clients = self.hub.client_count() as u32;
        let max_clients = g.max_clients;
        match &g.runtime {
            None => MirrorStatus {
                running: false,
                public_url: None,
                local_port: None,
                token: None,
                token_tail: None,
                clients: 0,
                max_clients,
                phase: MirrorPhase::Stopped,
                error: None,
                read_only: g.read_only,
                allow_lan: g.allow_lan,
                lan_url: None,
            },
            Some(r) => {
                let tail = auth::token_tail(&r.token, 6);
                let fallback = lan::local_access_url(r.allow_lan, r.port, &r.token, r.lan_ip);
                let lan_url = r
                    .lan_ip
                    .filter(|_| r.allow_lan)
                    .map(|ip| lan::mirror_path_url(&ip.to_string(), r.port, &r.token));
                MirrorStatus {
                    running: true,
                    public_url: r.public_url.clone().or(Some(fallback)),
                    local_port: Some(r.port),
                    token: Some(r.token.clone()),
                    token_tail: Some(tail),
                    clients,
                    max_clients,
                    phase: r.phase,
                    error: r.error.clone(),
                    read_only: r.read_only,
                    allow_lan: r.allow_lan,
                    lan_url,
                }
            }
        }
    }
}

async fn bind_http_with_retry(
    host: Arc<MirrorHost>,
    port: u16,
    dist_dir: PathBuf,
    allow_lan: bool,
) -> Result<(u16, oneshot::Sender<()>), String> {
    let mut last = "mirror rebind: no attempts".to_string();
    for i in 0..8u32 {
        match http::start_server(host.clone(), port, dist_dir.clone(), allow_lan).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = e;
                tokio::time::sleep(Duration::from_millis(40 * (i + 1) as u64)).await;
            }
        }
    }
    Err(last)
}

/// Tell every attached surface (desktop WebView + all mirror clients) that the
/// sessions index changed, so each can re-run `sessions.list`.
///
/// Fire on Host index writes that the desktop UI does not already refresh
/// in-process (turn persist). Mirror RPC create/rename/autoTitle also use this.
pub fn notify_sessions_changed<R: tauri::Runtime>(
    app: Option<&AppHandle<R>>,
    reason: &str,
    session_id: &str,
) {
    if let Some(app) = app {
        fanout_event(
            app,
            "sessions://changed",
            json!({ "reason": reason, "sessionId": session_id }),
        );
    }
}

/// Emit to desktop WebView **and** mirror WS clients (DESIGN §7.3).
/// Prefer this over bare `app.emit` for all `session://*` chat events.
pub fn fanout_event<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
    payload: impl Serialize + Clone,
) {
    let _ = app.emit(name, payload.clone());
    if let Some(host) = app.try_state::<Arc<MirrorHost>>() {
        match serde_json::to_value(&payload) {
            Ok(v) => host.broadcast(name, &v),
            Err(e) => {
                tracing::warn!(error = %e, event = name, "mirror fanout: serialize failed");
            }
        }
    }
}

/// Resolve SPA root: env override → compile-time ../dist → cwd candidates.
pub fn resolve_dist_dir(override_dir: Option<&PathBuf>) -> PathBuf {
    if let Some(p) = override_dir {
        return p.clone();
    }
    // Packaged / dev: Tauri frontendDist is ../dist relative to src-tauri.
    let manifest_dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist");
    if manifest_dist.join("index.html").is_file() {
        return manifest_dist.canonicalize().unwrap_or(manifest_dist);
    }
    for cand in [
        PathBuf::from("dist"),
        PathBuf::from("../dist"),
        PathBuf::from("./dist"),
    ] {
        if cand.join("index.html").is_file() {
            return cand.canonicalize().unwrap_or(cand);
        }
    }
    // Fall back to manifest-relative even if missing (placeholder HTML path).
    manifest_dist
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn mirror_status(host: State<'_, Arc<MirrorHost>>) -> Result<MirrorStatus, String> {
    Ok(host.status())
}

#[tauri::command]
pub async fn mirror_start(
    app: AppHandle,
    host: State<'_, Arc<MirrorHost>>,
    mgr: State<'_, Arc<SessionManager>>,
) -> Result<MirrorStatus, String> {
    host.attach(app, mgr.inner().clone());
    host.start().await
}

#[tauri::command]
pub async fn mirror_stop(host: State<'_, Arc<MirrorHost>>) -> Result<MirrorStatus, String> {
    host.stop().await
}

/// Restore an enabled mirror host, or explicitly start in headless mode.
pub fn maybe_autostart(host: Arc<MirrorHost>, app: AppHandle, mgr: Arc<SessionManager>) {
    if !host.env().headless {
        return;
    }
    host.attach(app, mgr);
    tracing::info!("restoring enabled mirror host");
    tauri::async_runtime::spawn(async move {
        match host.start().await {
            Ok(st) => {
                tracing::info!(
                    port = ?st.local_port,
                    phase = ?st.phase,
                    "headless mirror host ready"
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "headless mirror start failed");
            }
        }
    });
}

#[cfg(test)]
mod host_test;

#[cfg(test)]
mod lan_bind_test;

#[cfg(test)]
mod restart_test;

#[tauri::command]
pub async fn mirror_set_allow_lan(
    host: State<'_, Arc<MirrorHost>>,
    allow_lan: bool,
) -> Result<MirrorStatus, String> {
    host.set_allow_lan(allow_lan).await
}

#[tauri::command]
pub async fn mirror_rotate_token(host: State<'_, Arc<MirrorHost>>) -> Result<MirrorStatus, String> {
    host.rotate_token()
}

#[tauri::command]
pub async fn mirror_set_read_only(
    host: State<'_, Arc<MirrorHost>>,
    read_only: bool,
) -> Result<MirrorStatus, String> {
    host.set_read_only(read_only)?;
    Ok(host.status())
}

#[tauri::command]
pub async fn mirror_set_max_clients(
    host: State<'_, Arc<MirrorHost>>,
    max_clients: u32,
) -> Result<MirrorStatus, String> {
    host.set_max_clients(max_clients)?;
    Ok(host.status())
}
