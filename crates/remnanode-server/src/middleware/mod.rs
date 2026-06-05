pub mod jwt;

use axum::{
    extract::State,
    http::Request,
    middleware::Next,
    response::Response,
    body::Body,
};
use remnanode_config::SecretKey;
use crate::state::AppState;

pub async fn jwt_auth(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, axum::http::StatusCode> {
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => return Err(axum::http::StatusCode::UNAUTHORIZED),
    };

    if jwt::validate_token(token, &state.secret.jwt_public_key).is_err() {
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
