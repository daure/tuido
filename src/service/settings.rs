// App settings are local user preferences. Keep APIs crate-private; keys and values must never be
// exposed through MCP.
use sqlx::{AssertSqlSafe, Row};

use super::{ServiceResult, TuidoService, storage_error};

impl TuidoService {
    pub(crate) async fn app_setting(&self, key: &str) -> ServiceResult<Option<String>> {
        let sql = format!(
            "SELECT value FROM app_settings WHERE key = {}",
            self.dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(|row| row.try_get("value").map_err(storage_error))
            .transpose()
    }

    pub(crate) async fn set_app_setting(&self, key: &str, value: &str) -> ServiceResult<()> {
        let sql = format!(
            "INSERT INTO app_settings (key, value) VALUES ({}, {}) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await
            .map_err(storage_error)?;
        Ok(())
    }
}
