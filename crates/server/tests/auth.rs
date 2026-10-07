use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use server::auth::{Auth, Identity};
use sha2::Sha256;

fn sign(header: serde_json::Value, claims: serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let signed = format!("{header}.{claims}");
    let mut mac = Hmac::<Sha256>::new_from_slice(&[42; 32]).unwrap();
    mac.update(signed.as_bytes());
    format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

fn claims() -> serde_json::Value {
    serde_json::json!({
        "iss": "server", "aud": "settlers-v1", "sub": "player", "name": "Agent",
        "iat": 100, "nbf": 100, "exp": 200,
    })
}

#[test]
fn malformed_token_segments_and_oversized_tokens_are_rejected_without_echoing_tokens() {
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let valid = sign(serde_json::json!({"alg":"HS256","typ":"JWT"}), claims());
    for token in [
        "".to_owned(),
        "one-part".to_owned(),
        "a.b".to_owned(),
        "..".to_owned(),
        "a.b.c.d".to_owned(),
        format!("{valid}.extra"),
        "a".repeat(8193),
    ] {
        let error = auth.verify(&token, 100).unwrap_err();
        assert_eq!(error.0.code, settlers_protocol::ErrorCode::Unauthorized);
        assert_eq!(error.0.message, "invalid or expired authentication token");
    }
}

#[test]
fn valid_signatures_do_not_bypass_header_and_claim_validation() {
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let header = serde_json::json!({"alg":"HS256","typ":"JWT"});
    for bad_header in [
        serde_json::json!({"alg":"none","typ":"JWT"}),
        serde_json::json!({"alg":"RS256","typ":"JWT"}),
        serde_json::json!({"alg":"HS256","typ":"other"}),
        serde_json::json!({"alg":"HS256","typ":"JWT","unexpected":true}),
    ] {
        assert!(auth.verify(&sign(bad_header, claims()), 100).is_err());
    }
    for (field, value) in [
        ("iss", serde_json::json!("other")),
        ("aud", serde_json::json!("other")),
        ("sub", serde_json::json!("")),
        ("sub", serde_json::json!("a".repeat(201))),
        ("iat", serde_json::json!(101)),
        ("nbf", serde_json::json!(101)),
        ("exp", serde_json::json!(100)),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut claims = claims();
        claims[field] = value;
        assert!(
            auth.verify(&sign(header.clone(), claims), 100).is_err(),
            "accepted invalid {field}"
        );
    }
}

#[test]
fn tokens_are_valid_from_not_before_inclusive_until_expiry_exclusive() {
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let token = sign(serde_json::json!({"alg":"HS256","typ":"JWT"}), claims());
    assert!(auth.verify(&token, 99).is_err());
    assert!(auth.verify(&token, 100).is_ok());
    assert!(auth.verify(&token, 199).is_ok());
    assert!(auth.verify(&token, 200).is_err());
}
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
