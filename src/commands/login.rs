//! `photoflow login` — prompt for B2 credentials (hidden input) and store them in the OS
//! keyring. Idempotent: overwrites any prior values. The secret is never echoed or logged.

use anyhow::bail;

use crate::config::{self, Credentials, KEYRING_SERVICE};

pub fn run() -> anyhow::Result<()> {
    let key_id = rpassword::prompt_password("B2 key ID: ")?;
    let app_key = rpassword::prompt_password("B2 application key: ")?;

    if key_id.trim().is_empty() || app_key.trim().is_empty() {
        bail!("key ID and application key must both be non-empty; nothing stored");
    }

    config::store_credentials(&Credentials {
        key_id: key_id.trim().to_string(),
        app_key: app_key.trim().to_string(),
    })?;

    println!("Stored B2 credentials in the keyring (service '{KEYRING_SERVICE}').");
    Ok(())
}
