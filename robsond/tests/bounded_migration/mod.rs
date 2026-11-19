//! Public52 acceptance descendants; stages are application facts, never wire
//! authority. All PostgreSQL databases belong to sqlx::test; infrastructure
//! supplies access.
use super::*;

mod cases;
mod catalog_cases;
mod helpers;
mod late;
mod migration39;
mod migration40;
