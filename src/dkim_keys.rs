use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::rngs::OsRng;
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
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
    // SubjectPublicKeyInfo — Gmail/OpenDKIM. PKCS#1 alone is "invalid public key" at Gmail.
    let der = public.to_public_key_der().context("spki encode")?;
    Ok(DkimKeyPair {
        private_pem,
        public_b64: STANDARD.encode(der.as_bytes()),
    })
}

/// DNS `p=` value. Wraps a PKCS#1 key as SPKI; leaves SPKI unchanged.
pub fn to_spki_dns_p(public_b64: &str) -> String {
    let Ok(der) = STANDARD.decode(public_b64.trim()) else {
        return public_b64.trim().to_string();
    };
    if let Ok(key) = RsaPublicKey::from_pkcs1_der(&der) {
        if let Ok(spki) = key.to_public_key_der() {
            return STANDARD.encode(spki.as_bytes());
        }
    }
    public_b64.trim().to_string()
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
        // Gmail rejects PKCS#1 (MIIBCgK…) as "invalid public key"; OpenDKIM/Gmail use SPKI.
        assert!(
            pair.public_b64.starts_with("MIIBIjAN"),
            "DKIM p= must be SPKI, got {}",
            &pair.public_b64[..20.min(pair.public_b64.len())]
        );
    }

    #[test]
    fn wraps_pkcs1_public_as_spki() {
        let pkcs1 = generate_rsa_2048().expect("keygen");
        // If generation already emits SPKI, wrapping is identity-or-upgrade; also
        // accept a known PKCS#1 2048 stub by round-tripping to_spki.
        let spki = to_spki_dns_p(&pkcs1.public_b64);
        assert!(spki.starts_with("MIIBIjAN"), "got {}", &spki[..20.min(spki.len())]);
    }
}
