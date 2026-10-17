use hypgm_authn::{AuthError, AuthMethod, Claims, JwtConfig, JwtValidator};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use uuid::Uuid;

const TEST_SECRET: &[u8] = b"super-secret-test-key-32-bytes-long!";

const RSA_PRIVATE_KEY_PEM: &[u8] = br#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCoE12Q9P+HoOjt
rQffVkLg+SyqdIUf/M7j+ArXSfh/0mE2pbOunHMawL3rMqyVQKIK3yY9J63XaP9f
P1kNUpy/GTR5Ovo+GcSTsW3fQc47OhBAPbQR6Vgv1GWDyyR+GLF2rjHXdeU7asXB
gH4YnZPrrdo4QFyiet3ArJepPMr0UZ6tgu57FlONGRzNdgcayN7d3W/GOoR07QeB
J0CTK9F1vj7KQmbGftbqwfTrBbTjA2FgI0n4pR25f0+DkIaQSgz11e2R9j6bbz1l
JdPWQvSjZ36JPQUfn47hd7pzUfJ8RFjv7X8SjZaVXWMSAEehtgWxGvDqZLswVXPa
D0qEw8r9AgMBAAECggEABv4LQfkXWmak2iJ9NdrqXF6xb4P6d9q2JsDOfqdqqbKw
bawWHhkPJtlG7W6FxI3UT/x5pEiu0Evjd+x0QXwi3kZnqvV7rLK4vWY/ym577HXd
cDbA84/QJelBsep1IbPYSjY0B6E1gLPP911noZGIsmDgt79huza+XAIyKw3G6cQc
Jd88iE9fvUv3EEJsIykXi+AVuPzqXzDl9eL0FSOr1baUxPyfguT1+nQiSH3uj+rU
TTwdoLPJaxavN+ics142kvyYGJkfU0hQnpILLI20Y7Mo9qK/jcTd2WgFlrtbLerS
NjACPGP9h3HEFLMV0KwdHzsNYX6wTPdUKXVFu91l0QKBgQDoKp2+Flt5Fo7Kc8Ym
mQ19qZXTiCayn0vrN+4d6tQwDcdcB2hedo9AjwtIWMvk50sxVZDgy/BaDTdLNxOl
ozo0u+YFnOqnMNtlI3DhCStJblPLqzTHDjB0OSA/QAq5dkMXlSgmSoX2OLfjW4PF
2P2Y6rDHncYUg+phVdkcmC8qNQKBgQC5VG3QDfs2alyn3bPIgl4fz4UZ6WtvbKGc
2Js1tY6zidIg0XZRaBGuu0lQKxxfL0bui3K8GXdMxNMjRlqM5LamxdKCfMtgK0fA
41QrIMNDmuZJNalkeAUWaCF+Ol9QkKNlmLhBgVqWsm/uLndjbq72A8Hw6XY/zISB
YKHbYQr2qQKBgQCQzePNJ8yt3a/5/fPdJyEO+7MDWKwqRMjpUN0ns8vcnPt3Hvet
02zN62oyGZAymUCA6MmQLCMhR5TxSMzsRr6LHZb+cTy6QvLURiKleT6Sckm5gnNM
pED8fJTsXllSC70U/GcqWtjrFb08BVxPDmQ9+EOCfZmKOiz5wL3F4/Wm8QKBgQCM
zKX5EQYv2zK1neu+g7z6sfBPNVUX/BH0VWPltB/MKwLnR/t58XStqUATlg0iC4r6
JBtw6HAIdOhrvkGBFSnnHhfGZwUj9P24f30snuF6dS5LpGcjG0PSOB5qBfLwBFL/
Fr21lo66Oqpq7AZsFmT6JvSrENNzuO9aU43kW5+2sQKBgCxjqW0CQXW2lgZGs660
PdUhGuAoiH+oSpV5k9ODueuWntmin5v3N+HbYmlDF6ma8CTBYYhZJJTSB9jtzuPp
aRWOGPEo87Fb8hzfKOboRuUUTe9B+22egScm7jIUYxuZIK5WD9TvwHrjeLrss/c1
eargpPfC702tQ/vIFqUD28QD
-----END PRIVATE KEY-----"#;

const RSA_PUBLIC_KEY_PEM: &[u8] = br#"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqBNdkPT/h6Do7a0H31ZC
4PksqnSFH/zO4/gK10n4f9JhNqWzrpxzGsC96zKslUCiCt8mPSet12j/Xz9ZDVKc
vxk0eTr6PhnEk7Ft30HOOzoQQD20EelYL9Rlg8skfhixdq4x13XlO2rFwYB+GJ2T
663aOEBconrdwKyXqTzK9FGerYLuexZTjRkczXYHGsje3d1vxjqEdO0HgSdAkyvR
db4+ykJmxn7W6sH06wW04wNhYCNJ+KUduX9Pg5CGkEoM9dXtkfY+m289ZSXT1kL0
o2d+iT0FH5+O4Xe6c1HyfERY7+1/Eo2WlV1jEgBHobYFsRrw6mS7MFVz2g9KhMPK
/QIDAQAB
-----END PUBLIC KEY-----"#;

fn now_epoch() -> usize {
    chrono::Utc::now().timestamp() as usize
}

#[test]
fn test_decode_valid_hs256_token() {
    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let exp = now_epoch() + 3600;

    let claims = Claims::new(auth_id, user_id, exp);
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .expect("encoding should succeed");

    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);
    let principal = validator
        .verify_token(&token)
        .expect("verification should succeed");

    assert_eq!(principal.auth_id, auth_id);
    assert_eq!(principal.user_id, user_id);
    assert_eq!(principal.method, AuthMethod::BearerJwt);

    // Also verify with "Bearer " prefix, uppercase "BEARER ", and tab whitespace
    let bearer_token = format!("Bearer {token}");
    let principal_bearer = validator
        .verify_token(&bearer_token)
        .expect("verification with Bearer prefix should succeed");
    assert_eq!(principal_bearer.auth_id, auth_id);
    assert_eq!(principal_bearer.user_id, user_id);

    let uppercase_bearer = format!("BEARER {token}");
    let principal_upper = validator
        .verify_token(&uppercase_bearer)
        .expect("verification with uppercase BEARER prefix should succeed");
    assert_eq!(principal_upper.auth_id, auth_id);

    let tab_bearer = format!("Bearer\t{token}");
    let principal_tab = validator
        .verify_token(&tab_bearer)
        .expect("verification with tab whitespace should succeed");
    assert_eq!(principal_tab.auth_id, auth_id);
}

#[test]
fn test_decode_valid_rs256_token() {
    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let exp = now_epoch() + 3600;

    let claims = Claims::new(auth_id, user_id, exp);
    let encoding_key =
        EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY_PEM).expect("valid RSA private key");
    let token = encode(&Header::new(Algorithm::RS256), &claims, &encoding_key)
        .expect("encoding should succeed");

    let validator = JwtValidator::from_rs256_pem(RSA_PUBLIC_KEY_PEM).expect("valid RSA public key");
    let principal = validator
        .verify_token(&token)
        .expect("RS256 verification should succeed");

    assert_eq!(principal.auth_id, auth_id);
    assert_eq!(principal.user_id, user_id);
    assert_eq!(principal.method, AuthMethod::BearerJwt);
}

#[test]
fn test_rs256_with_config() {
    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let exp = now_epoch() + 3600;

    let claims = Claims::with_iss_aud(
        auth_id,
        user_id,
        exp,
        Some("auth.example.com".to_string()),
        Some("api.example.com".to_string()),
    );
    let encoding_key =
        EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY_PEM).expect("valid RSA private key");
    let token = encode(&Header::new(Algorithm::RS256), &claims, &encoding_key)
        .expect("encoding should succeed");

    let config = JwtConfig {
        algorithm: Algorithm::RS256,
        issuer: Some("auth.example.com".to_string()),
        audience: Some("api.example.com".into()),
        leeway_seconds: 10,
    };
    let validator = JwtValidator::from_rs256_pem_with_config(RSA_PUBLIC_KEY_PEM, config)
        .expect("valid RSA public key with config");
    let principal = validator.verify_token(&token).unwrap();
    assert_eq!(principal.auth_id, auth_id);
    assert_eq!(principal.user_id, user_id);
}

#[test]
fn test_expired_token_rejected() {
    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let exp = now_epoch() - 3600; // Expired 1 hour ago

    let claims = Claims::new(auth_id, user_id, exp);
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap();

    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);
    let result = validator.verify_token(&token);

    match result {
        Err(AuthError::ExpiredToken) => (),
        other => panic!("expected ExpiredToken, got {other:?}"),
    }
}

#[test]
fn test_tampered_token_rejected() {
    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let exp = now_epoch() + 3600;

    let claims = Claims::new(auth_id, user_id, exp);
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"other-secret-key-32-bytes-long!!"),
    )
    .unwrap();

    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);
    let result = validator.verify_token(&token);

    match result {
        Err(AuthError::InvalidToken(_)) => (),
        other => panic!("expected InvalidToken, got {other:?}"),
    }
}

#[test]
fn test_malformed_token_rejected() {
    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);

    assert!(matches!(
        validator.verify_token("not-a-valid-token"),
        Err(AuthError::InvalidToken(_))
    ));
}

#[test]
fn test_empty_token_rejected() {
    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);

    assert!(matches!(
        validator.verify_token(""),
        Err(AuthError::MissingCredentials)
    ));
    assert!(matches!(
        validator.verify_token("Bearer   "),
        Err(AuthError::MissingCredentials)
    ));
    assert!(matches!(
        validator.verify_token("Bearer"),
        Err(AuthError::MissingCredentials)
    ));
}

#[test]
fn test_issuer_and_audience_validation() {
    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let exp = now_epoch() + 3600;

    let claims = Claims::with_iss_aud(
        auth_id,
        user_id,
        exp,
        Some("auth.example.com".to_string()),
        Some("api.example.com".to_string()),
    );

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap();

    let config = JwtConfig {
        algorithm: Algorithm::HS256,
        issuer: Some("auth.example.com".to_string()),
        audience: Some("api.example.com".into()),
        leeway_seconds: 0,
    };
    let validator = JwtValidator::new(jsonwebtoken::DecodingKey::from_secret(TEST_SECRET), config);

    let principal = validator.verify_token(&token).unwrap();
    assert_eq!(principal.auth_id, auth_id);

    // Mismatched issuer should fail
    let bad_iss_config = JwtConfig {
        algorithm: Algorithm::HS256,
        issuer: Some("wrong.example.com".to_string()),
        audience: Some("api.example.com".into()),
        leeway_seconds: 0,
    };
    let bad_iss_validator = JwtValidator::new(
        jsonwebtoken::DecodingKey::from_secret(TEST_SECRET),
        bad_iss_config,
    );
    assert!(matches!(
        bad_iss_validator.verify_token(&token),
        Err(AuthError::InvalidToken(_))
    ));

    // Mismatched audience should fail
    let bad_aud_config = JwtConfig {
        algorithm: Algorithm::HS256,
        issuer: Some("auth.example.com".to_string()),
        audience: Some("wrong.example.com".into()),
        leeway_seconds: 0,
    };
    let bad_aud_validator = JwtValidator::new(
        jsonwebtoken::DecodingKey::from_secret(TEST_SECRET),
        bad_aud_config,
    );
    assert!(matches!(
        bad_aud_validator.verify_token(&token),
        Err(AuthError::InvalidToken(_))
    ));
}

#[test]
fn test_multi_value_audience_validation() {
    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let exp = now_epoch() + 3600;

    let claims = Claims::with_iss_aud(
        auth_id,
        user_id,
        exp,
        Some("auth.example.com".to_string()),
        Some(vec![
            "api.example.com".to_string(),
            "other.example.com".to_string(),
        ]),
    );

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap();

    // Validator configured with a single audience matching one of token's audiences
    let config = JwtConfig {
        algorithm: Algorithm::HS256,
        issuer: Some("auth.example.com".to_string()),
        audience: Some("api.example.com".into()),
        leeway_seconds: 0,
    };
    let validator = JwtValidator::new(jsonwebtoken::DecodingKey::from_secret(TEST_SECRET), config);

    let principal = validator.verify_token(&token).unwrap();
    assert_eq!(principal.auth_id, auth_id);

    // Validator configured with multiple acceptable audiences
    let multi_aud_config = JwtConfig {
        algorithm: Algorithm::HS256,
        issuer: Some("auth.example.com".to_string()),
        audience: Some(
            vec![
                "other.example.com".to_string(),
                "api.example.com".to_string(),
            ]
            .into(),
        ),
        leeway_seconds: 0,
    };
    let multi_aud_validator = JwtValidator::new(
        jsonwebtoken::DecodingKey::from_secret(TEST_SECRET),
        multi_aud_config,
    );
    let principal_multi = multi_aud_validator.verify_token(&token).unwrap();
    assert_eq!(principal_multi.auth_id, auth_id);

    // Validator configured with non-matching audiences should fail
    let bad_multi_aud_config = JwtConfig {
        algorithm: Algorithm::HS256,
        issuer: Some("auth.example.com".to_string()),
        audience: Some(
            vec![
                "wrong1.example.com".to_string(),
                "wrong2.example.com".to_string(),
            ]
            .into(),
        ),
        leeway_seconds: 0,
    };
    let bad_multi_aud_validator = JwtValidator::new(
        jsonwebtoken::DecodingKey::from_secret(TEST_SECRET),
        bad_multi_aud_config,
    );
    assert!(matches!(
        bad_multi_aud_validator.verify_token(&token),
        Err(AuthError::InvalidToken(_))
    ));

    let raw_claims_json = serde_json::json!({
        "auth_id": auth_id,
        "sub": user_id,
        "exp": exp,
        "iss": "auth.example.com",
        "aud": ["api.example.com", "dashboard.example.com"]
    });
    let deserialized: Claims = serde_json::from_value(raw_claims_json).unwrap();
    match deserialized.aud {
        Some(hypgm_authn::Audience::Multiple(ref list)) => {
            assert_eq!(list.len(), 2);
            assert!(
                deserialized
                    .aud
                    .as_ref()
                    .unwrap()
                    .contains("api.example.com")
            );
        }
        _ => panic!("expected Audience::Multiple"),
    }
}
