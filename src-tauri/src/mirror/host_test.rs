use super::*;

#[test]
fn env_config_defaults() {
    // Smoke: parsing does not panic; values depend on process env.
    let c = MirrorEnvConfig::from_env();
    let _ = c.headless;
    let _ = c.no_tunnel;
    assert!(c.max_clients >= MIN_CLIENTS && c.max_clients <= MAX_CLIENTS_CAP);
}

#[test]
fn normalize_max_clients_clamps() {
    assert_eq!(normalize_max_clients(0), MIN_CLIENTS);
    assert_eq!(normalize_max_clients(4), 4);
    assert_eq!(normalize_max_clients(99), MAX_CLIENTS_CAP);
}

#[test]
fn resolve_dist_prefers_existing() {
    let p = resolve_dist_dir(None);
    // Path is absolute-ish or relative; just ensure function returns something.
    assert!(!p.as_os_str().is_empty());
}

#[test]
fn set_max_clients_clamps_and_status() {
    let host = MirrorHost::from_env();
    assert_eq!(host.set_max_clients(0).unwrap(), MIN_CLIENTS);
    assert_eq!(host.max_clients(), MIN_CLIENTS);
    assert_eq!(host.set_max_clients(100).unwrap(), MAX_CLIENTS_CAP);
    let st = host.status();
    assert_eq!(st.max_clients, MAX_CLIENTS_CAP);
    assert!(!st.running);
}

#[tokio::test]
async fn token_gate_health_ac3() {
    let host = Arc::new(MirrorHost {
        inner: Mutex::new(Inner {
            env: MirrorEnvConfig {
                headless: false,
                token: Some("test-token-please-use-long-random-value-0123456789".into()),
                port: Some(0), // ephemeral
                no_tunnel: true,
                allow_lan: false,
                dist: Some(resolve_dist_dir(None)),
                max_clients: DEFAULT_MAX_CLIENTS,
            },
            runtime: None,
            ctx: None,
            read_only: true,
            max_clients: DEFAULT_MAX_CLIENTS,
            allow_lan: false,
        }),
        hub: Arc::new(ws::WsHub::new()),
        preferences_path: None,
    });
    let st = host.start().await.expect("start");
    assert!(st.running);
    let port = st.local_port.expect("port");
    let token = st.token.expect("token");

    let client = reqwest::Client::new();
    let good = client
        .get(format!("http://127.0.0.1:{port}/t/{token}/api/health"))
        .send()
        .await
        .expect("good req");
    assert_eq!(good.status().as_u16(), 200);

    let bad = client
        .get(format!("http://127.0.0.1:{port}/t/wrong-token/api/health"))
        .send()
        .await
        .expect("bad req");
    assert_eq!(bad.status().as_u16(), 401);

    let index = client
        .get(format!("http://127.0.0.1:{port}/t/{token}/"))
        .header(
            "user-agent",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)",
        )
        .send()
        .await
        .expect("index");
    assert_eq!(index.status().as_u16(), 200);
    let body = index.text().await.unwrap_or_default();
    assert!(
        body.contains("__MIRROR__") || body.contains("root"),
        "expected SPA/placeholder body, got len {}",
        body.len()
    );

    host.stop().await.expect("stop");
    let after = client
        .get(format!("http://127.0.0.1:{port}/t/{token}/api/health"))
        .send()
        .await;
    // Server may be gone (connection error) or still draining with non-OK.
    if let Ok(r) = after {
        let code = r.status().as_u16();
        assert!(
            code == 401 || code == 404 || code == 502 || code >= 500,
            "unexpected status after stop: {code}"
        );
    }
    // connection refused after shutdown is fine
}

/// Placeholder HTML (missing dist/index.html) must carry no-store Cache-Control
/// so phones never permanently cache "Mirror host online (dist not found)".
#[tokio::test]
async fn placeholder_index_has_no_store_cache_control() {
    let missing =
        std::env::temp_dir().join(format!("grok-mirror-missing-dist-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    std::fs::create_dir_all(&missing).expect("temp dist dir");
    // intentionally no index.html

    let host = Arc::new(MirrorHost {
        inner: Mutex::new(Inner {
            env: MirrorEnvConfig {
                headless: false,
                token: Some("placeholder-cache-token-0123456789abcdef".into()),
                port: Some(0),
                no_tunnel: true,
                allow_lan: false,
                dist: Some(missing.clone()),
                max_clients: DEFAULT_MAX_CLIENTS,
            },
            runtime: None,
            ctx: None,
            read_only: true,
            max_clients: DEFAULT_MAX_CLIENTS,
            allow_lan: false,
        }),
        hub: Arc::new(ws::WsHub::new()),
        preferences_path: None,
    });
    let st = host.start().await.expect("start");
    let port = st.local_port.expect("port");
    let token = st.token.expect("token");

    let client = reqwest::Client::new();
    let res = client
        .get(format!("http://127.0.0.1:{port}/t/{token}/"))
        .send()
        .await
        .expect("index");
    assert_eq!(res.status().as_u16(), 200);
    let cc = res
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        cc, "no-cache, no-store, must-revalidate",
        "placeholder must not be cacheable"
    );
    let body = res.text().await.unwrap_or_default();
    assert!(
        body.contains("dist not found") || body.contains("__MIRROR__"),
        "expected placeholder body, got len {}",
        body.len()
    );

    host.stop().await.expect("stop");
    let _ = std::fs::remove_dir_all(&missing);
}
