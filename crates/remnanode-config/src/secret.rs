use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretKey {
    pub ca_cert_pem: String,
    pub jwt_public_key: String,
    pub node_cert_pem: String,
    pub node_key_pem: String,
}

pub fn parse_secret_key(encoded: &str) -> Result<SecretKey, String> {
    let json_str = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| format!("Base64 decode failed: {e}"))?,
    )
    .map_err(|e| format!("UTF-8 decode failed: {e}"))?;

    // Try camelCase first (as sent by panel)
    #[derive(Deserialize)]
    #[allow(non_snake_case)]
    struct RawSecret {
        caCertPem: String,
        jwtPublicKey: String,
        nodeCertPem: String,
        nodeKeyPem: String,
    }

    let raw: RawSecret = serde_json::from_str(&json_str)
        .map_err(|e| format!("JSON parse failed: {e}"))?;

    Ok(SecretKey {
        ca_cert_pem: raw.caCertPem,
        jwt_public_key: raw.jwtPublicKey,
        node_cert_pem: raw.nodeCertPem,
        node_key_pem: raw.nodeKeyPem,
    })
}
