use anyhow::{Context, Result};
use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation, decode, decode_header};
use serde::Deserialize;
use std::{collections::HashMap, env, fmt};

const FIREBASE_KEYS_URL: &str =
    "https://www.googleapis.com/robot/v1/metadata/x509/securetoken@system.gserviceaccount.com";

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Claims {
    sub: String,
    iss: String,
    aud: String,
    exp: usize,
    iat: usize,
}

pub async fn verify_firebase_token(id_token: &str) -> Result<String> {
    let header = decode_header(id_token)?;
    let kid = header
        .kid
        .ok_or_else(|| anyhow::anyhow!("No 'kid' field in token header"))?;

    let project_id = env::var("FIREBASE_PROJECT_ID")
        .context("FIREBASE_PROJECT_ID must be set in environment")?;

    let keys: HashMap<String, String> = reqwest::get(FIREBASE_KEYS_URL)
        .await
        .context("Failed to fetch Firebase public keys")?
        .json()
        .await
        .context("Failed to parse Firebase public keys")?;

    let cert_pem = keys
        .get(&kid)
        .context("No public key found for given 'kid'")?;

    let decoding_key = DecodingKey::from_rsa_pem(cert_pem.as_bytes())
        .context("Failed to parse RSA key from cert")?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[&project_id]);
    validation.set_issuer(&[&format!("https://securetoken.google.com/{project_id}")]);

    let token_data: TokenData<Claims> = decode::<Claims>(id_token, &decoding_key, &validation)
        .context("Token validation failed")?;

    Ok(token_data.claims.sub)
}

#[derive(Debug)]
pub enum AuthError {
    MissingBearerToken,
    InvalidBearerToken,
    TokenVerification(anyhow::Error),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBearerToken => f.write_str("missing authorization bearer token"),
            Self::InvalidBearerToken => f.write_str("invalid authorization bearer token"),
            Self::TokenVerification(err) => write!(f, "token verification failed: {err}"),
        }
    }
}

impl std::error::Error for AuthError {}

pub fn parse_authorization_bearer(authorization: Option<&str>) -> Result<&str, AuthError> {
    let auth_header = authorization.ok_or(AuthError::MissingBearerToken)?;

    auth_header
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(AuthError::InvalidBearerToken)
}

pub async fn authenticate_bearer_authorization(
    authorization: Option<&str>,
) -> Result<String, AuthError> {
    let token = parse_authorization_bearer(authorization)?;

    verify_firebase_token(token)
        .await
        .map_err(AuthError::TokenVerification)
}
