//! The application half of this guest: implement `gen::AuthImpl` here.
//! crabgen scaffolds this file ONCE and never overwrites it; `crabgen regen`
//! prints any missing method signatures instead of editing it.
//!
//! Card-backed accounts: a ComputerCraft floppy is the "card" — its disk id is
//! `card_id`, a random key file on it is `card_secret`. We argon2id-hash the
//! secret (never store it) and keep users in a sqlite workload reached over the
//! mesh. Every method takes `store`, the name of that sqlite workload, so
//! placement stays the gateway's job (same convention as guest/caller).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use crab_sdk::{decode, encode_to_vec, mesh_call, Type, Value};

use crate::gen::AuthImpl;

/// Function address of sqlite's `exec` on the target (store) workload.
const EXEC_FN: &str = "crab:sqlite/db@0.1.0#exec";

pub struct App;

// ---- argon2 ---------------------------------------------------------------

/// DEMO cost parameters. Tiny memory (256 KiB, 1 pass) so the pure-Lua
/// wasmcraft engine can actually run the hash in-game; this is safe ONLY
/// because a card secret is a high-entropy random key. For human-typed
/// passwords, raise these toward OWASP guidance (m=19 MiB, t=2) and expect
/// much slower logins on the interpreter.
fn hasher() -> Argon2<'static> {
    let params = Params::new(256, 1, 1, None).expect("valid argon2 params");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// argon2id-hash `secret` with a fresh 16-byte random salt; returns a PHC
/// string (encodes algorithm + params + salt + hash, so it is self-describing
/// for verification).
fn hash_secret(secret: &str) -> Result<String, String> {
    let mut salt_bytes = [0u8; 16];
    getrandom::getrandom(&mut salt_bytes).map_err(|e| format!("rng unavailable: {e}"))?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|e| format!("salt encode: {e}"))?;
    let phc = hasher()
        .hash_password(secret.as_bytes(), &salt)
        .map_err(|e| format!("hash: {e}"))?;
    Ok(phc.to_string())
}

/// Constant-time verify of `secret` against a stored PHC string. Params come
/// from the PHC itself, so a default Argon2 verifies any of our records.
fn verify_secret(secret: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(secret.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

// ---- sqlite over the mesh -------------------------------------------------

/// Single-quote a value for inline SQL (sqlite `exec` takes a raw statement,
/// not bound params, so we escape `'` by doubling it).
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Run one SQL statement on the `store` workload and return its JSON reply.
/// A SQL-level error arrives as the sqlite result err case and surfaces here
/// as `Err`; transport failures do too.
fn exec(store: &str, sql: &str) -> Result<String, String> {
    let params = encode_to_vec(&Value::String(sql.to_string()));
    let reply = mesh_call(store, EXEC_FN, &params)?;
    let ty = Type::Result {
        ok: Some(Box::new(Type::String)),
        err: Some(Box::new(Type::String)),
    };
    match decode(&ty, &reply)? {
        Value::Result(Ok(Some(v))) => String::try_from(*v),
        Value::Result(Err(Some(v))) => Err(format!("sqlite: {}", String::try_from(*v)?)),
        other => Err(format!("sqlite: unexpected reply: {other:?}")),
    }
}

// ---- the exported interface ----------------------------------------------

impl AuthImpl for App {
    /// crab:auth/accounts@0.1.0#init — create the users table if absent.
    fn init(&self, store: String) -> Result<(), String> {
        exec(
            &store,
            "CREATE TABLE IF NOT EXISTS users(\
                card_id TEXT PRIMARY KEY, \
                username TEXT NOT NULL, \
                phc TEXT NOT NULL, \
                meta TEXT NOT NULL DEFAULT '{}')",
        )?;
        Ok(())
    }

    /// crab:auth/accounts@0.1.0#enroll-card — hash the secret and insert.
    fn enroll_card(
        &self,
        store: String,
        username: String,
        card_id: String,
        card_secret: String,
        meta: String,
    ) -> Result<(), String> {
        if username.is_empty() || card_id.is_empty() || card_secret.is_empty() {
            return Err("username, card-id and card-secret are required".into());
        }
        let phc = hash_secret(&card_secret)?;
        let meta = if meta.is_empty() { "{}" } else { &meta };
        let sql = format!(
            "INSERT INTO users(card_id, username, phc, meta) VALUES({}, {}, {}, {})",
            q(&card_id),
            q(&username),
            q(&phc),
            q(meta),
        );
        exec(&store, &sql).map_err(|e| {
            if e.contains("UNIQUE") || e.contains("constraint") {
                "card already enrolled".into()
            } else {
                e
            }
        })?;
        Ok(())
    }

    /// crab:auth/accounts@0.1.0#login-card — look up the card, verify the
    /// secret, and return the account as JSON. Unknown card and wrong secret
    /// return the SAME error so the client can't probe which cards exist.
    fn login_card(
        &self,
        store: String,
        card_id: String,
        card_secret: String,
    ) -> Result<String, String> {
        let sql = format!(
            "SELECT username, phc, meta FROM users WHERE card_id = {}",
            q(&card_id),
        );
        let json = exec(&store, &sql)?;
        account_from_reply(&json, &card_secret)
    }
}

/// Pure half of `login_card`: given sqlite's JSON reply for the card lookup and
/// the presented secret, verify it and produce the account JSON. Factored out
/// so it is unit-testable without the mesh. Unknown card and wrong secret both
/// return "invalid card" so the client can't probe which cards exist.
fn account_from_reply(json: &str, card_secret: &str) -> Result<String, String> {
    let reply: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("parse sqlite reply: {e}"))?;
    let rows = reply
        .get("rows")
        .and_then(|r| r.as_array())
        .ok_or("sqlite reply missing rows")?;

    let row = rows
        .first()
        .and_then(|r| r.as_array())
        .ok_or("invalid card")?;
    let username = row.first().and_then(|c| c.as_str()).unwrap_or("");
    let phc = row.get(1).and_then(|c| c.as_str()).unwrap_or("");
    let meta = row.get(2).and_then(|c| c.as_str()).unwrap_or("{}");

    if !verify_secret(card_secret, phc) {
        return Err("invalid card".into());
    }

    // meta is stored as a JSON string; re-embed it as JSON (fall back to a
    // plain string if it isn't valid JSON) so callers get one object.
    let meta_val: serde_json::Value =
        serde_json::from_str(meta).unwrap_or_else(|_| serde_json::Value::String(meta.into()));
    Ok(serde_json::json!({ "username": username, "meta": meta_val }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2_round_trip() {
        let phc = hash_secret("s3cret-card-key").unwrap();
        assert!(phc.starts_with("$argon2id$"), "PHC string: {phc}");
        assert!(verify_secret("s3cret-card-key", &phc));
        assert!(!verify_secret("wrong-key", &phc));
        // a fresh salt each time => different PHC strings for the same secret
        assert_ne!(phc, hash_secret("s3cret-card-key").unwrap());
    }

    #[test]
    fn sql_escaping_doubles_quotes() {
        assert_eq!(q("o'brien"), "'o''brien'");
        assert_eq!(q("plain"), "'plain'");
    }

    #[test]
    fn login_parses_reply_and_verifies() {
        let phc = hash_secret("key-123").unwrap();
        let reply = serde_json::json!({
            "columns": ["username", "phc", "meta"],
            "rows": [["alice", phc, "{\"role\":\"admin\"}"]],
            "changes": 0
        })
        .to_string();

        let out = account_from_reply(&reply, "key-123").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["username"], "alice");
        assert_eq!(v["meta"]["role"], "admin");

        // wrong secret and unknown card both report the same thing
        assert_eq!(account_from_reply(&reply, "nope").unwrap_err(), "invalid card");
        let empty = serde_json::json!({"columns":[],"rows":[],"changes":0}).to_string();
        assert_eq!(account_from_reply(&empty, "key-123").unwrap_err(), "invalid card");
    }
}
