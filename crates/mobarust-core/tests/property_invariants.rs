use mobarust_core::{AuthMethod, Protocol, SessionId, SessionRecord};
use proptest::prelude::*;

fn ascii_token(max_length: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::range('a', 'z'), 1..=max_length)
        .prop_map(|characters| characters.into_iter().collect())
}

fn ssh_session(
    name: String,
    hostname: String,
    username: String,
    port: u16,
    favorite: bool,
    server_alive_interval: Option<u64>,
) -> SessionRecord {
    SessionRecord {
        id: SessionId::new(),
        name,
        protocol: Protocol::Ssh,
        hostname,
        port,
        username: Some(username),
        auth: AuthMethod::None,
        last_used_at: None,
        known_hosts_path: None,
        pinned_fingerprint: None,
        x11_display: None,
        x11_single_connection: false,
        server_alive_interval,
        folder: None,
        tags: vec!["fixture".into()],
        favorite,
        startup_directory: None,
        startup_command: None,
        environment: Vec::new(),
        jump_hosts: Vec::new(),
        jump_host_profiles: Vec::new(),
        notes: None,
        serial_profile: None,
        telnet_profile: None,
        remote_desktop_profile: None,
    }
}

proptest! {
    #[test]
    fn session_json_round_trip_preserves_secret_free_profiles(
        name in ascii_token(40),
        hostname in ascii_token(64),
        username in ascii_token(32),
        port in 1u16..=65535,
        favorite in any::<bool>(),
        server_alive_interval in prop::option::of(0u64..=86_400),
    ) {
        let session = ssh_session(
            name,
            hostname,
            username,
            port,
            favorite,
            server_alive_interval,
        );
        let encoded = serde_json::to_vec(&session).expect("fixture session should serialize");
        let decoded: SessionRecord =
            serde_json::from_slice(&encoded).expect("serialized session should decode");

        prop_assert_eq!(decoded, session);
        prop_assert!(!encoded
            .windows("credentialRef".len())
            .any(|window| window == b"credentialRef"));
    }

    #[test]
    fn validated_server_alive_intervals_remain_within_the_transport_bound(
        interval in 0u64..=86_400,
    ) {
        let session = ssh_session(
            "fixture".into(),
            "fixture.local".into(),
            "operator".into(),
            22,
            false,
            Some(interval),
        );

        prop_assert!(session.validate().is_ok());
    }
}
