//! gen/MANIFEST: three lines that tie a project's generated code to the WIT
//! it was generated from. `crabgen check` recomputes the WIT hash against
//! this so stale bindings can't ship. Format (exact):
//!
//! ```text
//! crabgen 0.1.0
//! lang go
//! wit-sha256 <hex of the .wit file bytes>
//! ```

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// crabgen version that wrote gen/ (informational; freshness is hash-only)
    pub version: String,
    pub lang: String,
    pub wit_sha256: String,
}

/// Lowercase hex sha256 of the raw file bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl Manifest {
    /// A fresh manifest for the current crabgen version.
    pub fn new(lang: &str, wit_bytes: &[u8]) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            lang: lang.to_string(),
            wit_sha256: sha256_hex(wit_bytes),
        }
    }

    /// The exact 3-line file contents.
    pub fn render(&self) -> String {
        format!(
            "crabgen {}\nlang {}\nwit-sha256 {}\n",
            self.version, self.lang, self.wit_sha256
        )
    }

    pub fn parse(s: &str) -> Result<Self> {
        let mut lines = s.lines();
        let version = field(&mut lines, "crabgen")?;
        let lang = field(&mut lines, "lang")?;
        let wit_sha256 = field(&mut lines, "wit-sha256")?;
        if let Some(extra) = lines.next() {
            bail!("MANIFEST has more than 3 lines; unexpected: {extra:?}");
        }
        Ok(Self {
            version,
            lang,
            wit_sha256,
        })
    }

    /// Does gen/ still match these WIT bytes?
    pub fn is_fresh(&self, wit_bytes: &[u8]) -> bool {
        self.wit_sha256 == sha256_hex(wit_bytes)
    }
}

fn field(lines: &mut std::str::Lines, key: &str) -> Result<String> {
    let line = lines
        .next()
        .with_context(|| format!("MANIFEST truncated: missing `{key}` line"))?;
    match line
        .strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(' '))
    {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => bail!("malformed MANIFEST line {line:?}: expected `{key} <value>`"),
    }
}
