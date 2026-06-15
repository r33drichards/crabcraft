//! The application half of this guest: implement `gen::AuthImpl` here.
//! crabgen scaffolds this file ONCE and never overwrites it; `crabgen regen`
//! prints any missing method signatures instead of editing it.
//!
//! Public-key accounts. A ComputerCraft floppy is the "card" and carries an
//! Ed25519 PRIVATE key; sqlite stores only the matching PUBLIC key (keyed by a
//! random user-id), so a DB leak can neither forge nor clone a card. Login is a
//! challenge-response: the turtle signs a fresh nonce LOCALLY with the floppy's
//! key (via `sign`, run on the turtle so the key never leaves it) and `verify`
//! checks the signature against the stored public key over the mesh.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crab_sdk::{decode, encode_to_vec, mesh_call, Type, Value};

use crate::gen::AuthImpl;

/// Function address of sqlite's `exec` on the target (store) workload.
const EXEC_FN: &str = "crab:sqlite/db@0.1.0#exec";

pub struct App;

// ---- ed25519 helpers (pure, unit-testable) --------------------------------

/// Generate a keypair: returns (user_id, public_key_hex, private_key_hex).
/// The user-id is 8 random bytes (a friendly handle distinct from the key, so
/// the key can rotate); the private key is the 32-byte Ed25519 seed.
fn gen_keypair() -> Result<(String, String, String), String> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| format!("rng unavailable: {e}"))?;
    let sk = SigningKey::from_bytes(&seed);
    let pub_hex = hex::encode(sk.verifying_key().to_bytes());

    let mut id = [0u8; 8];
    getrandom::getrandom(&mut id).map_err(|e| format!("rng unavailable: {e}"))?;
    Ok((hex::encode(id), pub_hex, hex::encode(seed)))
}

/// Sign `nonce` with a hex-encoded 32-byte private key; returns hex signature.
fn sign_nonce(private_key_hex: &str, nonce: &str) -> Result<String, String> {
    let seed: [u8; 32] = hex::decode(private_key_hex)
        .map_err(|e| format!("bad private key hex: {e}"))?
        .try_into()
        .map_err(|_| "private key must be 32 bytes".to_string())?;
    let sk = SigningKey::from_bytes(&seed);
    Ok(hex::encode(sk.sign(nonce.as_bytes()).to_bytes()))
}

/// Verify a hex signature of `nonce` against a hex public key.
fn verify_sig(public_key_hex: &str, nonce: &str, signature_hex: &str) -> Result<(), String> {
    let pk: [u8; 32] = hex::decode(public_key_hex)
        .map_err(|e| format!("bad public key hex: {e}"))?
        .try_into()
        .map_err(|_| "public key must be 32 bytes".to_string())?;
    let sig: [u8; 64] = hex::decode(signature_hex)
        .map_err(|e| format!("bad signature hex: {e}"))?
        .try_into()
        .map_err(|_| "signature must be 64 bytes".to_string())?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("bad public key: {e}"))?;
    vk.verify(nonce.as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| "signature does not verify".to_string())
}

// ---- sqlite over the mesh -------------------------------------------------

/// Single-quote a value for inline SQL (sqlite `exec` takes a raw statement,
/// not bound params, so we escape `'` by doubling it). Keys/ids are hex, but
/// username/meta are user-supplied.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Run one SQL statement on the `store` workload and return its JSON reply.
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

/// Extract the single string cell at `col` from row 0 of a sqlite reply.
/// `None` = no rows (unknown user).
fn cell(reply: &serde_json::Value, col: usize) -> Option<String> {
    reply.get("rows")?.as_array()?.first()?.as_array()?
        .get(col)?
        .as_str()
        .map(|s| s.to_string())
}

// ---- the exported interface ----------------------------------------------

impl AuthImpl for App {
    /// crab:auth/accounts@0.1.0#init — create the users table if absent.
    fn init(&self, store: String) -> Result<(), String> {
        exec(
            &store,
            "CREATE TABLE IF NOT EXISTS users(\
                user_id TEXT PRIMARY KEY, \
                pubkey TEXT NOT NULL, \
                username TEXT NOT NULL, \
                meta TEXT NOT NULL DEFAULT '{}')",
        )?;
        Ok(())
    }

    /// crab:auth/accounts@0.1.0#register — keypair + store pubkey, return privkey once.
    fn register(&self, store: String, username: String, meta: String) -> Result<String, String> {
        if username.is_empty() {
            return Err("username is required".into());
        }
        let (user_id, pub_hex, priv_hex) = gen_keypair()?;
        let meta = if meta.is_empty() { "{}" } else { &meta };
        let sql = format!(
            "INSERT INTO users(user_id, pubkey, username, meta) VALUES({}, {}, {}, {})",
            q(&user_id),
            q(&pub_hex),
            q(&username),
            q(meta),
        );
        exec(&store, &sql)?;
        Ok(serde_json::json!({
            "user_id": user_id,
            "public_key": pub_hex,
            "private_key": priv_hex,
        })
        .to_string())
    }

    /// crab:auth/accounts@0.1.0#sign — sign a nonce locally (no storage). Run
    /// this on the turtle so the private key never crosses the network.
    fn sign(&self, private_key: String, nonce: String) -> Result<String, String> {
        sign_nonce(&private_key, &nonce)
    }

    /// crab:auth/accounts@0.1.0#verify — check a signature against the stored
    /// public key. Unknown user and bad signature both return "access denied".
    fn verify(
        &self,
        store: String,
        user_id: String,
        nonce: String,
        signature: String,
    ) -> Result<String, String> {
        let sql = format!(
            "SELECT pubkey, username, meta FROM users WHERE user_id = {}",
            q(&user_id),
        );
        let reply: serde_json::Value =
            serde_json::from_str(&exec(&store, &sql)?).map_err(|e| format!("parse reply: {e}"))?;

        // Unknown user -> "access denied" (don't reveal which ids exist).
        let pubkey = cell(&reply, 0).ok_or("access denied")?;
        let username = cell(&reply, 1).unwrap_or_default();
        let meta = cell(&reply, 2).unwrap_or_else(|| "{}".into());

        verify_sig(&pubkey, &nonce, &signature).map_err(|_| "access denied".to_string())?;

        let meta_val: serde_json::Value =
            serde_json::from_str(&meta).unwrap_or_else(|_| serde_json::Value::String(meta.clone()));
        Ok(serde_json::json!({
            "user_id": user_id,
            "username": username,
            "meta": meta_val,
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_sign_verify_round_trip() {
        let (user_id, pub_hex, priv_hex) = gen_keypair().unwrap();
        assert_eq!(user_id.len(), 16); // 8 bytes hex
        assert_eq!(pub_hex.len(), 64); // 32 bytes hex
        assert_eq!(priv_hex.len(), 64);

        let nonce = "challenge-12345";
        let sig = sign_nonce(&priv_hex, nonce).unwrap();
        assert_eq!(sig.len(), 128); // 64 bytes hex
        assert!(verify_sig(&pub_hex, nonce, &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampering() {
        let (_id, pub_hex, priv_hex) = gen_keypair().unwrap();
        let sig = sign_nonce(&priv_hex, "nonce-A").unwrap();

        // wrong nonce (replay of a sig for a different challenge) fails
        assert!(verify_sig(&pub_hex, "nonce-B", &sig).is_err());
        // a different keypair's public key fails
        let (_id2, other_pub, _p2) = gen_keypair().unwrap();
        assert!(verify_sig(&other_pub, "nonce-A", &sig).is_err());
        // a flipped signature byte fails
        let mut bad = sig.clone();
        bad.replace_range(0..2, if &sig[0..2] == "00" { "ff" } else { "00" });
        assert!(verify_sig(&pub_hex, "nonce-A", &bad).is_err());
    }

    #[test]
    fn verify_sig_rejects_malformed_hex() {
        let (_id, pub_hex, priv_hex) = gen_keypair().unwrap();
        let sig = sign_nonce(&priv_hex, "x").unwrap();
        assert!(verify_sig("not-hex", "x", &sig).is_err());
        assert!(verify_sig(&pub_hex, "x", "deadbeef").is_err()); // wrong length
    }

    #[test]
    fn sql_escaping_doubles_quotes() {
        assert_eq!(q("o'brien"), "'o''brien'");
    }
}
