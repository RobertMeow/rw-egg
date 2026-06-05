use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};

pub fn validate_token(token: &str, public_key: &str) -> Result<(), jsonwebtoken::errors::Error> {
    let decoding_key = DecodingKey::from_rsa_pem(public_key.as_bytes())?;
    let validation = Validation::new(Algorithm::RS256);
    decode::<serde_json::Value>(token, &decoding_key, &validation)?;
    Ok(())
}
