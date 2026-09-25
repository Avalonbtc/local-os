use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use argon2::{
    Argon2, PasswordHasher, PasswordVerifier,
    password_hash::{PasswordHash, SaltString},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::{RngCore, rngs::OsRng};
use rig_domain::*;

pub struct Vault {
    cipher: Aes256Gcm,
}
impl Vault {
    pub fn new(key: &str) -> Result<Self> {
        let bytes = STANDARD
            .decode(key)
            .map_err(|_| Error::Validation("主密钥需为 32 字节的 Base64".into()))?;
        let cipher = Aes256Gcm::new_from_slice(&bytes)
            .map_err(|_| Error::Validation("主密钥长度必须为 32 字节".into()))?;
        Ok(Self { cipher })
    }
}
impl SecretVault for Vault {
    fn encrypt(&self, value: &Credential) -> Result<String> {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let clear = serde_json::to_vec(value).map_err(|e| Error::Internal(e.to_string()))?;
        let encrypted = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), clear.as_ref())
            .map_err(|_| Error::Internal("凭据加密失败".into()))?;
        let mut result = nonce.to_vec();
        result.extend(encrypted);
        Ok(STANDARD.encode(result))
    }
    fn decrypt(&self, value: &str) -> Result<Credential> {
        let bytes = STANDARD
            .decode(value)
            .map_err(|_| Error::Internal("凭据格式损坏".into()))?;
        if bytes.len() < 28 {
            return Err(Error::Internal("凭据格式损坏".into()));
        }
        let clear = self
            .cipher
            .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
            .map_err(|_| Error::Internal("凭据解密失败，请检查主密钥".into()))?;
        serde_json::from_slice(&clear).map_err(|_| Error::Internal("凭据格式损坏".into()))
    }
    fn password_hash(&self, value: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(value.as_bytes(), &salt)
            .map(|v| v.to_string())
            .map_err(|e| Error::Internal(e.to_string()))
    }
    fn verify_password(&self, value: &str, hash: &str) -> bool {
        PasswordHash::new(hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(value.as_bytes(), &hash)
                .is_ok()
        })
    }
    fn random_token(&self) -> String {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_encryption() {
        let vault = Vault::new(&STANDARD.encode([7u8; 32])).unwrap();
        let secret = Credential::Password {
            password: "secret-value".into(),
            sudo_password: None,
        };
        let encrypted = vault.encrypt(&secret).unwrap();
        assert!(!encrypted.contains("secret-value"));
        assert!(
            matches!(vault.decrypt(&encrypted).unwrap(),Credential::Password{password,..} if password=="secret-value")
        );
        let wrong = Vault::new(&STANDARD.encode([8u8; 32])).unwrap();
        assert!(wrong.decrypt(&encrypted).is_err());
    }
}
