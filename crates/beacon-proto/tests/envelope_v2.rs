use beacon_proto::envelope::*;
use beacon_proto::ipc::DriverInfo;
use beacon_model::SessionId;

#[test]
fn subscribe_session_round_trip() {
    let sid: SessionId = "01KP70H1HSPDGYSKPAD42FY8BN".parse().unwrap();
    let msg = ClientToArc::SubscribeSession { session_id: sid, since_seq: Some(100) };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"kind\":\"SubscribeSession\""));
    let back: ClientToArc = serde_json::from_str(&json).unwrap();
    match back {
        ClientToArc::SubscribeSession { session_id, since_seq } => {
            assert_eq!(session_id.to_string(), "01KP70H1HSPDGYSKPAD42FY8BN");
            assert_eq!(since_seq, Some(100));
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn session_output_round_trip() {
    let sid: SessionId = "01KP70H1HSPDGYSKPAD42FY8BN".parse().unwrap();
    let msg = ArcToClient::SessionOutput {
        session_id: sid,
        bytes: vec![1, 2, 3],
        seq: 7,
        replay: false,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: ArcToClient = serde_json::from_str(&json).unwrap();
    match back {
        ArcToClient::SessionOutput { session_id, bytes, seq, replay } => {
            assert_eq!(session_id.to_string(), "01KP70H1HSPDGYSKPAD42FY8BN");
            assert_eq!(bytes, vec![1, 2, 3]);
            assert_eq!(seq, 7);
            assert_eq!(replay, false);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn lease_changed_none_round_trip() {
    let sid: SessionId = "01KP70H1HSPDGYSKPAD42FY8BN".parse().unwrap();
    let msg = ArcToClient::LeaseChanged { session_id: sid, driver: None };
    let json = serde_json::to_string(&msg).unwrap();
    let back: ArcToClient = serde_json::from_str(&json).unwrap();
    match back {
        ArcToClient::LeaseChanged { session_id, driver: None } => {
            assert_eq!(session_id.to_string(), "01KP70H1HSPDGYSKPAD42FY8BN");
        }
        other => panic!("wrong variant or driver not None: {other:?}"),
    }
}

#[test]
fn lease_changed_some_round_trip() {
    let sid: SessionId = "01KP70H1HSPDGYSKPAD42FY8BN".parse().unwrap();
    let driver = DriverInfo {
        user_id: "u1".into(),
        device_label: "mac".into(),
        since: 1_700_000_000,
    };
    let msg = ArcToClient::LeaseChanged { session_id: sid, driver: Some(driver.clone()) };
    let json = serde_json::to_string(&msg).unwrap();
    let back: ArcToClient = serde_json::from_str(&json).unwrap();
    match back {
        ArcToClient::LeaseChanged { driver: Some(d), .. } => {
            assert_eq!(d, driver);
        }
        other => panic!("wrong variant or driver not Some: {other:?}"),
    }
}

#[test]
fn host_input_round_trip() {
    let sid: SessionId = "01KP70H1HSPDGYSKPAD42FY8BN".parse().unwrap();
    let msg = ArcToClient::HostInput {
        session_id: sid,
        bytes: b"echo hi\n".to_vec(),
        from_user: "u1".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: ArcToClient = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, ArcToClient::HostInput { .. }));
}
