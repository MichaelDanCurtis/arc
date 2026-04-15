use beacon_proto::ipc::*;
use beacon_model::SessionId;

#[test]
fn push_output_round_trip() {
    let sid: SessionId = "01KP70H1HSPDGYSKPAD42FY8BN".parse().unwrap();
    let req = IpcRequest::PushOutput {
        session_id: sid,
        bytes: vec![1, 2, 3, 4],
        seq: 42,
    };
    let bytes = rmp_serde::to_vec_named(&req).unwrap();
    let decoded: IpcRequest = rmp_serde::from_slice(&bytes).unwrap();
    match decoded {
        IpcRequest::PushOutput { session_id, bytes, seq } => {
            assert_eq!(session_id.to_string(), "01KP70H1HSPDGYSKPAD42FY8BN");
            assert_eq!(bytes, vec![1, 2, 3, 4]);
            assert_eq!(seq, 42);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn ipc_event_remote_input_round_trip() {
    let sid: SessionId = "01KP70H1HSPDGYSKPAD42FY8BN".parse().unwrap();
    let evt = IpcEvent::RemoteInput {
        session_id: sid,
        bytes: b"hello".to_vec(),
        from_user: "user-uuid".into(),
    };
    let bytes = rmp_serde::to_vec_named(&evt).unwrap();
    let decoded: IpcEvent = rmp_serde::from_slice(&bytes).unwrap();
    assert!(matches!(decoded, IpcEvent::RemoteInput { .. }));
}
