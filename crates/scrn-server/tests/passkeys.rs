//! Integration tests for passkeys. A small software authenticator in this
//! file produces real WebAuthn registration and authentication responses
//! (CBOR attestation with "none" format, COSE ES256 key, ECDSA signatures),
//! which webauthn-rs verifies exactly as it would verify a browser's.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, PASSWORD, TestResponse, TestServer};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use scrn_server::crypto::sha256;
use scrn_server::passkey::CEREMONY_TTL;
use scrn_server::repo::passkeys;
use scrn_server::repo::users::{self, Role, User};
use serde_json::{Value, json};

const ORIGIN: &str = "https://scrn.test";
const RP_ID: &str = "scrn.test";

// --- A minimal software authenticator --------------------------------------

fn b64url(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

fn cbor_head(major: u8, n: u64) -> Vec<u8> {
    let m = major << 5;
    match n {
        0..=23 => vec![m | n as u8],
        24..=0xff => vec![m | 24, n as u8],
        0x100..=0xffff => {
            let mut v = vec![m | 25];
            v.extend_from_slice(&(n as u16).to_be_bytes());
            v
        }
        _ => {
            let mut v = vec![m | 26];
            v.extend_from_slice(&(n as u32).to_be_bytes());
            v
        }
    }
}

fn cbor_int(i: i64) -> Vec<u8> {
    if i >= 0 {
        cbor_head(0, i as u64)
    } else {
        cbor_head(1, (-1 - i) as u64)
    }
}

fn cbor_bytes(b: &[u8]) -> Vec<u8> {
    let mut v = cbor_head(2, b.len() as u64);
    v.extend_from_slice(b);
    v
}

fn cbor_text(s: &str) -> Vec<u8> {
    let mut v = cbor_head(3, s.len() as u64);
    v.extend_from_slice(s.as_bytes());
    v
}

fn cbor_map(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut v = cbor_head(5, entries.len() as u64);
    for (k, val) in entries {
        v.extend_from_slice(k);
        v.extend_from_slice(val);
    }
    v
}

struct SoftKey {
    key: SigningKey,
    credential_id: Vec<u8>,
    counter: u32,
}

impl SoftKey {
    fn new(seed: u8) -> Self {
        let mut secret = [7u8; 32];
        secret[0] = seed;
        Self {
            key: SigningKey::from_slice(&secret).unwrap(),
            credential_id: vec![seed; 16],
            counter: 0,
        }
    }

    fn cose_key(&self) -> Vec<u8> {
        let point = self.key.verifying_key().to_sec1_point(false);
        let bytes = point.as_bytes();
        let (x, y) = (&bytes[1..33], &bytes[33..65]);
        cbor_map(&[
            (cbor_int(1), cbor_int(2)),
            (cbor_int(3), cbor_int(-7)),
            (cbor_int(-1), cbor_int(1)),
            (cbor_int(-2), cbor_bytes(x)),
            (cbor_int(-3), cbor_bytes(y)),
        ])
    }

    fn client_data(kind: &str, challenge: &str) -> Vec<u8> {
        json!({"type": kind, "challenge": challenge, "origin": ORIGIN, "crossOrigin": false})
            .to_string()
            .into_bytes()
    }

    /// Answers `navigator.credentials.create()`.
    fn register(&self, options: &Value) -> Value {
        let challenge = options["publicKey"]["challenge"].as_str().unwrap();
        let client_data = Self::client_data("webauthn.create", challenge);
        let mut auth_data = sha256(RP_ID.as_bytes()).to_vec();
        auth_data.push(0x45); // user present, user verified, attested data
        auth_data.extend_from_slice(&0u32.to_be_bytes());
        auth_data.extend_from_slice(&[0; 16]); // AAGUID
        auth_data.extend_from_slice(&(self.credential_id.len() as u16).to_be_bytes());
        auth_data.extend_from_slice(&self.credential_id);
        auth_data.extend_from_slice(&self.cose_key());
        let attestation = cbor_map(&[
            (cbor_text("fmt"), cbor_text("none")),
            (cbor_text("attStmt"), cbor_map(&[])),
            (cbor_text("authData"), cbor_bytes(&auth_data)),
        ]);
        json!({
            "id": b64url(&self.credential_id),
            "rawId": b64url(&self.credential_id),
            "type": "public-key",
            "response": {
                "attestationObject": b64url(&attestation),
                "clientDataJSON": b64url(&client_data),
            },
            "clientExtensionResults": {},
        })
    }

    /// Answers `navigator.credentials.get()`.
    fn sign(&mut self, options: &Value) -> Value {
        self.counter += 1;
        let challenge = options["publicKey"]["challenge"].as_str().unwrap();
        let client_data = Self::client_data("webauthn.get", challenge);
        let mut auth_data = sha256(RP_ID.as_bytes()).to_vec();
        auth_data.push(0x05); // user present, user verified
        auth_data.extend_from_slice(&self.counter.to_be_bytes());
        let mut signed = auth_data.clone();
        signed.extend_from_slice(&sha256(&client_data));
        let signature: Signature = self.key.sign(&signed);
        json!({
            "id": b64url(&self.credential_id),
            "rawId": b64url(&self.credential_id),
            "type": "public-key",
            "response": {
                "authenticatorData": b64url(&auth_data),
                "clientDataJSON": b64url(&client_data),
                "signature": b64url(signature.to_der().as_bytes()),
                "userHandle": null,
            },
            "clientExtensionResults": {},
        })
    }
}

// --- Helpers ---------------------------------------------------------------

async fn post_json(client: &mut Client, path: &str, csrf: &str, body: &Value) -> TestResponse {
    client.headers.retain(|(name, _)| name != "x-csrf-token");
    client.headers.push(("x-csrf-token".into(), csrf.into()));
    let response = client
        .send_raw(
            "POST",
            path,
            Some("application/json"),
            body.to_string().into_bytes(),
        )
        .await;
    client.headers.retain(|(name, _)| name != "x-csrf-token");
    response
}

fn json_body(response: &TestResponse) -> Value {
    serde_json::from_str(&response.body).unwrap_or(Value::Null)
}

async fn logged_in(server: &TestServer, name: &str) -> (User, Client, String) {
    let user = server.create_user(name, Role::User).await;
    let mut browser = server.client("192.0.2.1");
    browser.login(name, PASSWORD).await.assert_redirect("/app");
    let csrf = browser.get("/app/settings").await.csrf();
    (user, browser, csrf)
}

async fn register(browser: &mut Client, csrf: &str, key: &SoftKey, name: &str) -> TestResponse {
    let start = post_json(browser, "/app/passkeys/register/start", csrf, &json!({})).await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.body);
    let start = json_body(&start);
    let credential = key.register(&start["options"]);
    post_json(
        browser,
        "/app/passkeys/register/finish",
        csrf,
        &json!({"ceremony": start["ceremony"], "name": name, "credential": credential}),
    )
    .await
}

/// A logged-out browser with its login-page CSRF token.
async fn login_page(server: &TestServer, ip: &str) -> (Client, String) {
    let mut browser = server.client(ip);
    let csrf = browser.get("/login").await.csrf();
    (browser, csrf)
}

async fn login_start(browser: &mut Client, csrf: &str, username: &str) -> TestResponse {
    post_json(
        browser,
        "/login/passkey/start",
        csrf,
        &json!({"username": username}),
    )
    .await
}

// --- Tests -----------------------------------------------------------------

#[tokio::test]
async fn register_then_sign_in_with_the_passkey() {
    let server = TestServer::new().await;
    let (user, mut browser, csrf) = logged_in(&server, "alice").await;
    let mut key = SoftKey::new(1);

    let page = browser.get("/app/settings").await;
    assert!(page.body.contains("data-passkey-register"));

    let done = register(&mut browser, &csrf, &key, "Laptop").await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(
        json_body(&done)["redirect"],
        "/app/settings?passkey=added#passkeys"
    );
    let stored = passkeys::list_for_user(&server.state.db, user.id)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "Laptop");
    assert_eq!(stored[0].credential_id, key.credential_id);
    let page = browser.get("/app/settings?passkey=added").await;
    assert!(page.body.contains("Passkey hinzugefügt."));
    assert!(page.body.contains(r#"value="Laptop""#));

    // Sign in from a fresh browser.
    let (mut other, login_csrf) = login_page(&server, "192.0.2.2").await;
    assert!(
        other
            .get("/login")
            .await
            .body
            .contains("data-passkey-login-button")
    );
    let start = login_start(&mut other, &login_csrf, "ALICE").await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.body);
    let start = json_body(&start);
    let allowed = &start["options"]["publicKey"]["allowCredentials"];
    assert_eq!(allowed[0]["id"], b64url(&key.credential_id));
    let credential = key.sign(&start["options"]);
    let done = post_json(
        &mut other,
        "/login/passkey/finish",
        &login_csrf,
        &json!({"ceremony": start["ceremony"], "credential": credential}),
    )
    .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(json_body(&done)["redirect"], "/app");
    assert!(
        done.set_cookies
            .iter()
            .any(|c| c.starts_with("scrn_session="))
    );
    assert_eq!(other.get("/app").await.status, StatusCode::OK);

    let stored = passkeys::list_for_user(&server.state.db, user.id)
        .await
        .unwrap();
    assert_eq!(stored[0].last_used_at, Some(common::START));
    assert!(stored[0].credential.contains("\"counter\":1"));
}

#[tokio::test]
async fn ceremonies_are_single_use_and_expire() {
    let server = TestServer::new().await;
    let (_, mut browser, csrf) = logged_in(&server, "alice").await;
    let mut key = SoftKey::new(1);
    register(&mut browser, &csrf, &key, "Laptop").await;

    let (mut other, login_csrf) = login_page(&server, "192.0.2.2").await;
    let start = json_body(&login_start(&mut other, &login_csrf, "alice").await);
    let credential = key.sign(&start["options"]);
    let body = json!({"ceremony": start["ceremony"], "credential": credential});
    let first = post_json(&mut other, "/login/passkey/finish", &login_csrf, &body).await;
    assert_eq!(first.status, StatusCode::OK);
    // Replaying the same answer fails: the ceremony is gone.
    let replay = post_json(&mut other, "/login/passkey/finish", &login_csrf, &body).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert!(
        json_body(&replay)["message"]
            .as_str()
            .unwrap()
            .contains("abgelaufen")
    );

    // A ceremony older than the TTL is refused.
    let (mut late, late_csrf) = login_page(&server, "192.0.2.3").await;
    let start = json_body(&login_start(&mut late, &late_csrf, "alice").await);
    server.clock.advance(CEREMONY_TTL);
    let credential = key.sign(&start["options"]);
    let response = post_json(
        &mut late,
        "/login/passkey/finish",
        &late_csrf,
        &json!({"ceremony": start["ceremony"], "credential": credential}),
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(late.cookies.keys().all(|k| k != "scrn_session"));
}

#[tokio::test]
async fn wrong_keys_and_foreign_credentials_are_rejected() {
    let server = TestServer::new().await;
    let (_, mut alice, alice_csrf) = logged_in(&server, "alice").await;
    let alice_key = SoftKey::new(1);
    register(&mut alice, &alice_csrf, &alice_key, "Laptop").await;

    let mut bob_browser = server.client("192.0.2.5");
    server.create_user("bob", Role::User).await;
    bob_browser.login("bob", PASSWORD).await;
    let bob_csrf = bob_browser.get("/app/settings").await.csrf();
    let mut bob_key = SoftKey::new(2);
    register(&mut bob_browser, &bob_csrf, &bob_key, "Phone").await;

    // A different private key with Alice's credential ID.
    let (mut attacker, csrf) = login_page(&server, "192.0.2.66").await;
    let start = json_body(&login_start(&mut attacker, &csrf, "alice").await);
    let mut forged = SoftKey::new(9);
    forged.credential_id = alice_key.credential_id.clone();
    let response = post_json(
        &mut attacker,
        "/login/passkey/finish",
        &csrf,
        &json!({"ceremony": start["ceremony"], "credential": forged.sign(&start["options"])}),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    // Bob's valid passkey cannot answer a challenge for Alice.
    let start = json_body(&login_start(&mut attacker, &csrf, "alice").await);
    let response = post_json(
        &mut attacker,
        "/login/passkey/finish",
        &csrf,
        &json!({"ceremony": start["ceremony"], "credential": bob_key.sign(&start["options"])}),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(attacker.cookies.keys().all(|k| k != "scrn_session"));

    // The same credential cannot be registered twice: webauthn-rs refuses
    // it because it is in the exclusion list of the ceremony.
    let response = register(&mut bob_browser, &bob_csrf, &bob_key, "Again").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let bob = users::get_by_username(&server.state.db, "bob")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        passkeys::count_for_user(&server.state.db, bob.id)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn login_start_does_not_tell_users_apart() {
    let server = TestServer::new().await;
    server.create_user("nopasskey", Role::User).await;
    let (disabled, mut browser, csrf) = logged_in(&server, "disabled").await;
    register(&mut browser, &csrf, &SoftKey::new(1), "Laptop").await;
    users::set_disabled(&server.state.db, disabled.id, Some(common::START))
        .await
        .unwrap();

    let (mut visitor, csrf) = login_page(&server, "192.0.2.9").await;
    let mut bodies = Vec::new();
    for name in ["ghost", "nopasskey", "disabled", ""] {
        let response = login_start(&mut visitor, &csrf, name).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{name}");
        bodies.push(response.body);
    }
    assert!(bodies.windows(2).all(|w| w[0] == w[1]), "{bodies:?}");
}

#[tokio::test]
async fn json_endpoints_need_csrf_and_login() {
    let server = TestServer::new().await;
    let (_, mut browser, csrf) = logged_in(&server, "alice").await;
    for path in [
        "/app/passkeys/register/start",
        "/app/passkeys/register/finish",
    ] {
        let response = post_json(&mut browser, path, "wrong", &json!({})).await;
        assert!(
            matches!(
                response.status,
                StatusCode::FORBIDDEN | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "{path}: {}",
            response.status
        );
    }
    let response = post_json(
        &mut browser,
        "/app/passkeys/register/start",
        "wrong",
        &json!({}),
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    let (mut visitor, login_csrf) = login_page(&server, "192.0.2.9").await;
    let response = login_start(&mut visitor, "wrong", "alice").await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let response = login_start(&mut visitor, &login_csrf, "alice").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);

    // Registration needs a session.
    let response = post_json(
        &mut visitor,
        "/app/passkeys/register/start",
        &csrf,
        &json!({}),
    )
    .await;
    response.assert_redirect("/login");

    // A registration ceremony belongs to the user who started it.
    let start = json_body(
        &post_json(
            &mut browser,
            "/app/passkeys/register/start",
            &csrf,
            &json!({}),
        )
        .await,
    );
    let (_, mut bob, bob_csrf) = logged_in(&server, "bob").await;
    let response = post_json(
        &mut bob,
        "/app/passkeys/register/finish",
        &bob_csrf,
        &json!({"ceremony": start["ceremony"], "name": "x", "credential": SoftKey::new(3).register(&start["options"])}),
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rename_and_delete_but_never_the_last_login_method() {
    let server = TestServer::new().await;
    let (user, mut browser, csrf) = logged_in(&server, "alice").await;
    register(&mut browser, &csrf, &SoftKey::new(1), "One").await;
    register(&mut browser, &csrf, &SoftKey::new(2), "Two").await;
    let rows = passkeys::list_for_user(&server.state.db, user.id)
        .await
        .unwrap();
    let (first, second) = (rows[0].id, rows[1].id);

    browser
        .submit(
            "/app/settings",
            &format!("/app/passkeys/{first}/rename"),
            &[("name", "Work laptop")],
        )
        .await
        .assert_redirect("/app/settings#passkeys");
    let response = browser
        .submit(
            "/app/settings",
            &format!("/app/passkeys/{first}/rename"),
            &[("name", " ")],
        )
        .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        passkeys::list_for_user(&server.state.db, user.id)
            .await
            .unwrap()[0]
            .name,
        "Work laptop"
    );

    // Without a password, the last passkey is the last way to sign in.
    sqlx::query("UPDATE users SET password_hash = NULL WHERE id = ?")
        .bind(user.id)
        .execute(&server.state.db)
        .await
        .unwrap();
    browser
        .submit(
            "/app/settings",
            &format!("/app/passkeys/{first}/delete"),
            &[],
        )
        .await
        .assert_redirect("/app/settings#passkeys");
    let response = browser
        .submit(
            "/app/settings",
            &format!("/app/passkeys/{second}/delete"),
            &[],
        )
        .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.body.contains("letzte Anmeldemöglichkeit"));
    assert_eq!(
        passkeys::count_for_user(&server.state.db, user.id)
            .await
            .unwrap(),
        1
    );

    // With a password it can go.
    sqlx::query("UPDATE users SET password_hash = 'x' WHERE id = ?")
        .bind(user.id)
        .execute(&server.state.db)
        .await
        .unwrap();
    browser
        .submit(
            "/app/settings",
            &format!("/app/passkeys/{second}/delete"),
            &[],
        )
        .await
        .assert_redirect("/app/settings#passkeys");
    assert_eq!(
        passkeys::count_for_user(&server.state.db, user.id)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn other_users_passkeys_cannot_be_changed() {
    let server = TestServer::new().await;
    let (alice, mut alice_browser, csrf) = logged_in(&server, "alice").await;
    register(&mut alice_browser, &csrf, &SoftKey::new(1), "Laptop").await;
    let id = passkeys::list_for_user(&server.state.db, alice.id)
        .await
        .unwrap()[0]
        .id;

    let (_, mut bob, _) = logged_in(&server, "bob").await;
    let response = bob
        .submit(
            "/app/settings",
            &format!("/app/passkeys/{id}/rename"),
            &[("name", "mine")],
        )
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let response = bob
        .submit("/app/settings", &format!("/app/passkeys/{id}/delete"), &[])
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let rows = passkeys::list_for_user(&server.state.db, alice.id)
        .await
        .unwrap();
    assert_eq!(rows[0].name, "Laptop");
}

#[tokio::test]
async fn passkeys_are_off_for_ip_base_urls() {
    let server = TestServer::with_env(&[("SCRN_BASE_URL", "http://127.0.0.1:8080")]).await;
    assert!(server.state.passkeys.is_none());
    let (_, mut browser, csrf) = logged_in(&server, "alice").await;
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains("keine IP-Adresse"));
    assert!(!page.body.contains("data-passkey-register"));
    let response = post_json(
        &mut browser,
        "/app/passkeys/register/start",
        &csrf,
        &json!({}),
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let (mut visitor, _) = login_page(&server, "192.0.2.9").await;
    assert!(
        !visitor
            .get("/login")
            .await
            .body
            .contains("data-passkey-login-button")
    );
}

// --- Passkey instead of the current password -------------------------------

const NEW_PASSWORD: &str = "a brand new passphrase";

async fn set_password(browser: &mut Client, current: &str) -> TestResponse {
    browser
        .submit(
            "/app/settings",
            "/app/settings/password",
            &[
                ("current_password", current),
                ("password", NEW_PASSWORD),
                ("password_confirm", NEW_PASSWORD),
            ],
        )
        .await
}

/// Signs in with the passkey from a fresh browser.
async fn passkey_login(server: &TestServer, key: &mut SoftKey, ip: &str) -> Client {
    let (mut browser, csrf) = login_page(server, ip).await;
    let start = json_body(&login_start(&mut browser, &csrf, "alice").await);
    let credential = key.sign(&start["options"]);
    let done = post_json(
        &mut browser,
        "/login/passkey/finish",
        &csrf,
        &json!({"ceremony": start["ceremony"], "credential": credential}),
    )
    .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    browser
}

async fn confirm(browser: &mut Client, csrf: &str, key: &mut SoftKey) -> TestResponse {
    let start = post_json(browser, "/app/passkeys/confirm/start", csrf, &json!({})).await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.body);
    let start = json_body(&start);
    let credential = key.sign(&start["options"]);
    post_json(
        browser,
        "/app/passkeys/confirm/finish",
        csrf,
        &json!({"ceremony": start["ceremony"], "credential": credential}),
    )
    .await
}

#[tokio::test]
async fn a_fresh_passkey_login_replaces_the_current_password() {
    let server = TestServer::new().await;
    let (_, mut browser, csrf) = logged_in(&server, "alice").await;
    let mut key = SoftKey::new(1);
    register(&mut browser, &csrf, &key, "Laptop").await;

    let mut passkey_browser = passkey_login(&server, &mut key, "192.0.2.2").await;
    let page = passkey_browser.get("/app/settings").await;
    assert!(!page.body.contains(r#"name="current_password""#));
    set_password(&mut passkey_browser, "")
        .await
        .assert_redirect("/app/settings?password=changed#password");
    // The password session from before is signed out.
    browser.get("/app").await.assert_redirect("/login");
    let mut fresh = server.client("192.0.2.3");
    fresh
        .login("alice", NEW_PASSWORD)
        .await
        .assert_redirect("/app");
}

#[tokio::test]
async fn passkey_confirmation_expires() {
    let server = TestServer::new().await;
    let (_, mut browser, csrf) = logged_in(&server, "alice").await;
    let mut key = SoftKey::new(1);
    register(&mut browser, &csrf, &key, "Laptop").await;
    let mut passkey_browser = passkey_login(&server, &mut key, "192.0.2.2").await;

    server
        .clock
        .advance(scrn_server::auth::PASSKEY_CONFIRM_WINDOW);
    let page = passkey_browser.get("/app/settings").await;
    assert!(page.body.contains(r#"name="current_password""#));
    let refused = set_password(&mut passkey_browser, "").await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn confirm_with_passkey_on_the_settings_page() {
    let server = TestServer::new().await;
    let (user, mut browser, csrf) = logged_in(&server, "alice").await;

    // Without a passkey there is nothing to confirm with.
    let page = browser.get("/app/settings").await;
    assert!(!page.body.contains("data-passkey-confirm-button"));
    let none = post_json(
        &mut browser,
        "/app/passkeys/confirm/start",
        &csrf,
        &json!({}),
    )
    .await;
    assert_eq!(none.status, StatusCode::BAD_REQUEST);

    let mut key = SoftKey::new(1);
    register(&mut browser, &csrf, &key, "Laptop").await;
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains("data-passkey-confirm-button"));
    assert!(page.body.contains(r#"name="current_password""#));

    let done = confirm(&mut browser, &csrf, &mut key).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(
        json_body(&done)["redirect"],
        "/app/settings?confirmed=1#password"
    );
    let page = browser.get("/app/settings?confirmed=1").await;
    assert!(page.body.contains("Mit Passkey bestätigt."));
    assert!(!page.body.contains(r#"name="current_password""#));
    set_password(&mut browser, "forgotten")
        .await
        .assert_redirect("/app/settings?password=changed#password");
    let stored = users::get_by_id(&server.state.db, user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(scrn_server::crypto::verify_password(
        NEW_PASSWORD,
        stored.password_hash.as_deref().unwrap()
    ));
}

#[tokio::test]
async fn confirmation_needs_the_users_own_passkey() {
    let server = TestServer::new().await;
    let (_, mut alice, alice_csrf) = logged_in(&server, "alice").await;
    let mut alice_key = SoftKey::new(1);
    register(&mut alice, &alice_csrf, &alice_key, "Laptop").await;

    server.create_user("bob", Role::User).await;
    let mut bob_browser = server.client("192.0.2.9");
    bob_browser
        .login("bob", PASSWORD)
        .await
        .assert_redirect("/app");
    let bob_csrf = bob_browser.get("/app/settings").await.csrf();
    let bob_key = SoftKey::new(2);
    register(&mut bob_browser, &bob_csrf, &bob_key, "Phone").await;

    // Alice's ceremony cannot be finished in Bob's session.
    let start = json_body(
        &post_json(
            &mut alice,
            "/app/passkeys/confirm/start",
            &alice_csrf,
            &json!({}),
        )
        .await,
    );
    let credential = alice_key.sign(&start["options"]);
    let stolen = post_json(
        &mut bob_browser,
        "/app/passkeys/confirm/finish",
        &bob_csrf,
        &json!({"ceremony": start["ceremony"], "credential": credential}),
    )
    .await;
    assert_eq!(stolen.status, StatusCode::BAD_REQUEST);

    // A different key than the registered one is rejected.
    let start = json_body(
        &post_json(
            &mut bob_browser,
            "/app/passkeys/confirm/start",
            &bob_csrf,
            &json!({}),
        )
        .await,
    );
    let mut stranger = SoftKey::new(3);
    let credential = stranger.sign(&start["options"]);
    let wrong = post_json(
        &mut bob_browser,
        "/app/passkeys/confirm/finish",
        &bob_csrf,
        &json!({"ceremony": start["ceremony"], "credential": credential}),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    let page = bob_browser.get("/app/settings").await;
    assert!(page.body.contains(r#"name="current_password""#));

    // Wrong CSRF token.
    let forged = post_json(
        &mut alice,
        "/app/passkeys/confirm/start",
        "forged",
        &json!({}),
    )
    .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn new_admin_is_asked_for_a_passkey() {
    let server = TestServer::new().await;
    scrn_server::announce_setup_token(&server.state)
        .await
        .unwrap();
    let token = server.state.issue_setup_token().unwrap();
    let mut browser = server.client("192.0.2.1");
    browser
        .submit(
            "/setup",
            "/setup",
            &[
                ("setup_token", &token),
                ("username", "admin"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await
        .assert_redirect("/app/settings?welcome=1#passkeys");
    let page = browser.get("/app/settings?welcome=1").await;
    assert!(page.body.contains("Lege jetzt einen Passkey an."));
    let key = SoftKey::new(1);
    let csrf = page.csrf();
    let done = register(&mut browser, &csrf, &key, "Laptop").await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    let page = browser.get("/app/settings?welcome=1").await;
    assert!(!page.body.contains("Lege jetzt einen Passkey an."));
}
