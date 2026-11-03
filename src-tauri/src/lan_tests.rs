use super::*;
use tokio::time::{timeout, Duration};

async fn connect(server: &LanServer) -> TcpStream {
    let mut client = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    write_json(
        &mut client,
        &serde_json::json!({
            "type": "hello", "deviceId": "test-phone", "deviceName": "Test phone",
            "token": server.token(), "protocolVersion": 2
        }),
    )
    .await
    .unwrap();
    let ack = read_json(&mut client).await.unwrap();
    assert_eq!(ack["type"], "hello_ack");
    client
}

#[tokio::test]
async fn companion_roundtrips_requests_and_cleans_up_connections() {
    let server = LanServer::new(0);
    server.start().await.unwrap();
    let mut client = connect(&server).await;
    let request = server.request("test-phone", "gallery_list", serde_json::json!({}));
    let phone = async {
        let message = read_json(&mut client).await.unwrap();
        assert_eq!(message["command"], "gallery_list");
        write_json(
            &mut client,
            &serde_json::json!({
                "type": "response", "id": message["id"], "result": {"photos": []}
            }),
        )
        .await
        .unwrap();
    };
    let (result, _) = tokio::join!(request, phone);
    assert_eq!(result.unwrap()["photos"], serde_json::json!([]));
    server.stop().await.unwrap();
    assert!(timeout(Duration::from_secs(1), read_json(&mut client))
        .await
        .unwrap()
        .is_err());
    assert_eq!(server.status().await.peers, 0);
}

#[tokio::test]
async fn companion_rejects_unpaired_peers_and_local_file_requests() {
    let server = LanServer::new(0);
    server.start().await.unwrap();
    let mut client = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    write_json(
        &mut client,
        &serde_json::json!({"type":"hello", "token":"wrong", "deviceId":"attacker"}),
    )
    .await
    .unwrap();
    assert_eq!(
        read_json(&mut client).await.unwrap()["code"],
        "pairing_rejected"
    );
    assert!(read_json(&mut client).await.is_err());
    assert!(server
        .request("attacker", "gallery_list", serde_json::json!({}))
        .await
        .is_err());
    let mut client = connect(&server).await;
    write_json(
        &mut client,
        &serde_json::json!({"type":"gallery", "path":"/etc/passwd"}),
    )
    .await
    .unwrap();
    write_json(&mut client, &serde_json::json!({"type":"ping"}))
        .await
        .unwrap();
    let reply = read_json(&mut client).await.unwrap();
    assert_eq!(reply["type"], "pong");
    server.stop().await.unwrap();
}

#[tokio::test]
async fn companion_removes_failed_requests_on_disconnect() {
    let server = LanServer::new(0);
    server.start().await.unwrap();
    let mut client = connect(&server).await;
    let request = server.request("test-phone", "clipboard_get", serde_json::json!({}));
    let phone = async {
        let _ = read_json(&mut client).await.unwrap();
        drop(client);
    };
    let (result, _) = timeout(Duration::from_secs(2), async {
        tokio::join!(request, phone)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    server.stop().await.unwrap();
}

#[tokio::test]
async fn another_phone_cannot_stop_an_owned_capture() {
    let server = LanServer::new(0);
    {
        let mut media = server.inner.media.lock().await;
        media.owner = Some("phone-a".into());
        media.mode = Some("mic".into());
    }
    assert!(server
        .request("phone-b", "capture_stop", json!({}))
        .await
        .is_err());
    assert_eq!(server.inner.media.lock().await.mode.as_deref(), Some("mic"));
}

#[tokio::test]
async fn invalid_capture_mode_does_not_allocate_media() {
    let server = LanServer::new(0);
    assert!(server
        .request("test-phone", "capture_start", json!({"mode":"invalid"}))
        .await
        .is_err());
    assert!(server.inner.media.lock().await.owner.is_none());
}

#[tokio::test]
async fn camera_rejects_invalid_orientation_before_starting_decoders() {
    let server = LanServer::new(0);
    {
        let mut media = server.inner.media.lock().await;
        media.owner = Some("camera-test".into());
        media.mode = Some("studio".into());
    }
    for rotation in [
        serde_json::json!(45),
        serde_json::json!(-90),
        serde_json::json!("90"),
    ] {
        let result = process_media(
            &server.inner,
            "camera-test",
            &serde_json::json!({
                "type": "video", "codec": "h264", "width": 640, "height": 480,
                "rotation": rotation, "data": "AA=="
            }),
        )
        .await;
        assert_eq!(result.unwrap_err(), "Invalid camera rotation");
    }
}

#[test]
fn clipboard_toggle_rejects_automatic_packets_but_accepts_explicit_manual_sends() {
    let server = LanServer::new(0);
    let automatic = json!({"text":"automatic", "automatic":true});
    let legacy = json!({"text":"legacy"});
    let manual = json!({"text":"manual", "automatic":false});
    assert_eq!(server.inner.clipboard_text(&automatic), Some("automatic"));
    server.inner.clipboard_sync.store(false, Ordering::SeqCst);
    assert_eq!(server.inner.clipboard_text(&automatic), None);
    assert_eq!(server.inner.clipboard_text(&legacy), None);
    assert_eq!(
        server
            .inner
            .clipboard_text(&json!({"text":"invalid", "automatic":"false"})),
        None
    );
    assert_eq!(server.inner.clipboard_text(&manual), Some("manual"));
    server.inner.clipboard_sync.store(true, Ordering::SeqCst);
    assert_eq!(server.inner.clipboard_text(&legacy), Some("legacy"));
}
