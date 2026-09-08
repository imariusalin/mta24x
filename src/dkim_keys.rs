use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::rngs::OsRng;
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::{RsaPrivateKey, RsaPublicKey};

#[derive(Debug, Clone)]
pub struct DkimKeyPair {
    pub private_pem: String,
    pub public_b64: String,
}

pub fn generate_rsa_2048() -> Result<DkimKeyPair> {
    let private = RsaPrivateKey::new(&mut OsRng, 2048).context("rsa keygen")?;
    let public = RsaPublicKey::from(&private);
    let private_pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .context("pem encode")?
        .to_string();
    let der = public.to_pkcs1_der().context("der encode")?;
    Ok(DkimKeyPair {
        private_pem,
        public_b64: STANDARD.encode(der.as_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_pem_and_dns_ready_public() {
        let pair = generate_rsa_2048().expect("keygen");
        assert!(pair.private_pem.contains("BEGIN PRIVATE KEY"));
        assert!(pair.public_b64.len() > 200);
        assert!(!pair.public_b64.contains('\n'));
    }
}
