use crate::{db, error::ApiError};
use sqlx::PgPool;

pub async fn ensure_player_rows(db: &PgPool, uid: &str) -> Result<(), ApiError> {
    db::player::ensure_player_rows(db, uid).await
}
