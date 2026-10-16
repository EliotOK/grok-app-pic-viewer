use super::*;
use futures_util::StreamExt;

#[tokio::test]
async fn restart_restores_pairing_policy_and_explicit_stop() {
    let dir = std::env::temp_dir().join(format!("mirror-restart-{}", uuid::Uuid::new_v4()));
    let path = dir.join("mirror-host.json");
    let host = Arc::new(MirrorHost::with_preferences(path.clone()).unwrap());
    {
        let mut g = host.inner.lock();
        g.env.no_tunnel = true;
        g.env.port = Some(0);
    }
    host.set_read_only(false).unwrap();
    host.set_max_clients(7).unwrap();
    host.set_allow_lan(true).await.unwrap();
    let original = host.start().await.unwrap();
    host.stop_sync();
    drop(host);

    let restored = Arc::new(MirrorHost::with_preferences(path.clone()).unwrap());
    assert!(restored.env().headless);
    restored.inner.lock().env.no_tunnel = true;
    let status = restored.start().await.unwrap();
    assert_eq!(status.token, original.token);
    assert_eq!(status.local_port, original.local_port);
    assert_eq!(status.max_clients, 7);
    assert!(!status.read_only);
    assert!(status.allow_lan);
    let health = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!(
            "http://127.0.0.1:{}/t/{}/api/health",
            status.local_port.unwrap(),
            status.token.as_ref().unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status().as_u16(), 200);

    let rotated = restored.rotate_token().unwrap();
    assert_ne!(rotated.token, original.token);
    assert_eq!(preferences::load(&path).unwrap().token, rotated.token);
    restored.stop().await.unwrap();
    let stopped = MirrorHost::with_preferences(path).unwrap();
    assert!(!stopped.env().headless);
    assert!(!stopped.status().running);
    assert_eq!(stopped.env().token, rotated.token);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn occupied_saved_port_does_not_change_pairing() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = occupied.local_addr().unwrap().port();
    let host = Arc::new(MirrorHost::from_env());
    host.inner.lock().env.port = Some(port);
    assert!(host.start().await.is_err());
    assert_eq!(host.env().port, Some(port));
    assert!(!host.status().running);
}

#[tokio::test]
async fn stopping_host_disconnects_an_existing_phone_socket() {
    let host = Arc::new(MirrorHost::from_env());
    {
        let mut g = host.inner.lock();
        g.env.port = Some(0);
        g.env.no_tunnel = true;
    }
    let status = host.start().await.unwrap();
    let url = format!(
        "ws://127.0.0.1:{}/t/{}/ws",
        status.local_port.unwrap(),
        status.token.unwrap()
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(matches!(
        hello,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
    ));
    host.stop().await.unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
    ));
}
