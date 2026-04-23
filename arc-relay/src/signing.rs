use ed25519_dalek::{Signature, Signer, Verifier, SigningKey, VerifyingKey};
use ed25519_dalek::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
use sqlx::PgPool;

pub fn verify_developer_signature(
    public_key_pem: &str,
    sha256_hex: &str,
    signature_hex: &str,
) -> Result<bool, String> {
    let verifying_key = VerifyingKey::from_public_key_pem(public_key_pem)
        .map_err(|e| format!("Invalid public key: {e}"))?;
    let sig_bytes = hex::decode(signature_hex)
        .map_err(|e| format!("Invalid hex signature: {e}"))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| format!("Invalid signature bytes: {e}"))?;
    Ok(verifying_key.verify(sha256_hex.as_bytes(), &signature).is_ok())
}

pub fn countersign(server_private_key_pem: &str, sha256_hex: &str) -> Result<String, String> {
    let signing_key = SigningKey::from_pkcs8_pem(server_private_key_pem)
        .map_err(|e| format!("Invalid server key: {e}"))?;
    let sig: Signature = signing_key.sign(sha256_hex.as_bytes());
    Ok(hex::encode(sig.to_bytes()))
}

pub async fn get_or_create_server_key(pool: &PgPool) -> Result<String, String> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM server_config WHERE key = 'signing_private_key'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("DB: {e}"))?;

    if let Some((pem,)) = row {
        return Ok(pem);
    }

    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    let pem = key
        .to_pkcs8_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
        .map_err(|e| format!("PEM encode: {e}"))?;

    sqlx::query("INSERT INTO server_config (key, value) VALUES ('signing_private_key', $1) ON CONFLICT DO NOTHING")
        .bind(pem.as_str())
        .execute(pool)
        .await
        .map_err(|e| format!("DB: {e}"))?;

    let pub_pem = key
        .verifying_key()
        .to_public_key_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
        .map_err(|e| format!("PEM encode: {e}"))?;

    sqlx::query("INSERT INTO server_config (key, value) VALUES ('signing_public_key', $1) ON CONFLICT DO NOTHING")
        .bind(&pub_pem)
        .execute(pool)
        .await
        .map_err(|e| format!("DB: {e}"))?;

    Ok(pem.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let pem = key
            .to_pkcs8_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
            .unwrap();
        let pub_pem = key
            .verifying_key()
            .to_public_key_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
            .unwrap();

        let data = "abc123deadbeef";
        let sig = countersign(pem.as_str(), data).unwrap();
        assert!(verify_developer_signature(&pub_pem, data, &sig).unwrap());
    }

    #[test]
    fn wrong_data_fails_verification() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let pem = key
            .to_pkcs8_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
            .unwrap();
        let pub_pem = key
            .verifying_key()
            .to_public_key_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
            .unwrap();

        let sig = countersign(pem.as_str(), "correct").unwrap();
        assert!(!verify_developer_signature(&pub_pem, "wrong", &sig).unwrap());
    }
}
