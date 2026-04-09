use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Hash a plaintext password with Argon2id.
///
/// Returns a PHC-format string that includes the salt and all parameters,
/// suitable for storage in the database.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a stored PHC hash string.
///
/// Returns `true` if the password matches, `false` if it does not.
/// Returns `Err` only when the hash string itself is malformed.
pub fn verify_password(password: &str, hash: &str) -> Result<bool, argon2::password_hash::Error> {
    let parsed_hash = PasswordHash::new(hash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_correct_password() {
        let password = "super-secret-password-123!";
        let hash = hash_password(password).expect("hashing should succeed");
        let ok = verify_password(password, &hash).expect("verification should not error");
        assert!(ok, "correct password should verify successfully");
    }

    #[test]
    fn wrong_password_returns_false() {
        let hash = hash_password("correct-horse-battery-staple").unwrap();
        let ok = verify_password("wrong-password", &hash).unwrap();
        assert!(!ok, "wrong password should not verify");
    }

    #[test]
    fn different_passwords_produce_different_hashes() {
        let hash1 = hash_password("password-one").unwrap();
        let hash2 = hash_password("password-two").unwrap();
        assert_ne!(
            hash1, hash2,
            "distinct passwords must produce distinct hashes"
        );
    }

    #[test]
    fn same_password_different_salts_produce_different_hashes() {
        let password = "shared-password";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();
        // Different salts → different PHC strings, but both verify correctly.
        assert_ne!(
            hash1, hash2,
            "same password hashed twice should differ due to random salt"
        );
        assert!(verify_password(password, &hash1).unwrap());
        assert!(verify_password(password, &hash2).unwrap());
    }

    #[test]
    fn malformed_hash_returns_error() {
        let err = verify_password("any-password", "not-a-valid-phc-hash");
        assert!(err.is_err(), "malformed hash should return an error");
    }
}
