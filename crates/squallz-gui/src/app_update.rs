//! Desktop adapter for the shared stable-release update checker.

use std::collections::HashMap;

use crate::dto::ErrorDto;

#[tauri::command]
pub async fn check_for_updates() -> Result<squallz_update::UpdateCheck, ErrorDto> {
    squallz_update::check_for_updates(
        env!("CARGO_PKG_VERSION"),
        squallz_update::ReleasePackage::Desktop,
    )
    .await
    .map_err(|error| ErrorDto {
        key: error.i18n_key().to_owned(),
        params: HashMap::new(),
        detail: error.detail().to_owned(),
    })
}
