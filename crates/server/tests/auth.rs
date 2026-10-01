use server::auth::{Auth, Identity};
#[test]
fn signed_tokens_expire_and_survive_authenticator_restart() {
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let identity = Identity {
        subject: "stable-player".into(),
        name: "Bot".into(),
    };
    let token = auth.issue(&identity, 100).unwrap();
    assert_eq!(token.expires_at, 100 + 30 * 86400);
    let restarted = Auth::new(vec![42; 32], true).unwrap();
    assert_eq!(
        restarted.verify(&token.token, 101).unwrap().subject,
        identity.subject
    );
    assert!(restarted.verify(&token.token, token.expires_at).is_err());
    assert_eq!(
        restarted
            .verify(&token.token, token.expires_at - 1)
            .unwrap(),
        identity
    );
    assert!(restarted.verify(&token.token, 99).is_err());
    let renewed = restarted.issue(&identity, 200).unwrap();
    assert_eq!(renewed.subject, token.subject);
}
#[test]
fn tampering_wrong_keys_and_unsigned_tokens_are_rejected() {
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let token = auth
        .issue(
            &Identity {
                subject: "a".into(),
                name: "A".into(),
            },
            0,
        )
        .unwrap();
    assert!(
        Auth::new(vec![41; 32], true)
            .unwrap()
            .verify(&token.token, 1)
            .is_err()
    );
    assert!(auth.verify("eyJhbGciOiJub25lIn0.e30.", 1).is_err());
    assert!(auth.verify(&(token.token + "a"), 1).is_err());
    assert!(Auth::new(vec![0; 16], true).is_err());
}
#[tokio::test]
async fn production_disables_guest_bootstrap() {
    let auth = Auth::new(vec![42; 32], false).unwrap();
    assert!(
        auth.login("guest", &serde_json::json!({"name":"A"}), 0)
            .await
            .is_err()
    );
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let a = auth
        .login("guest", &serde_json::json!({"name":"A"}), 0)
        .await
        .unwrap();
    let b = auth
        .login("guest", &serde_json::json!({"name":"A"}), 0)
        .await
        .unwrap();
    assert_ne!(a.subject, b.subject);
    assert!(
        auth.login("guest", &serde_json::json!({"name":""}), 0)
            .await
            .is_err()
    );
}
