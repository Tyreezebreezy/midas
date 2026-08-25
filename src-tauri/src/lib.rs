mod accounts;
mod db;
mod import;
mod money;
mod needs_review;
mod recurring;
mod rules;
mod settings;
mod transactions;

use db::DbState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_dir)?;
            let conn = db::open(&app_dir.join("midas.sqlite3"))?;
            app.manage(DbState(Mutex::new(conn)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts::list_accounts,
            accounts::create_account,
            accounts::update_account,
            accounts::set_account_archived,
            settings::get_setting,
            settings::set_setting,
            transactions::list_transactions,
            transactions::create_transaction,
            transactions::update_transaction,
            transactions::set_transaction_archived,
            transactions::delete_transaction,
            import::preview_csv,
            import::stage_import,
            import::commit_import,
            needs_review::list_needs_review,
            needs_review::resolve_needs_review,
            rules::list_rules,
            rules::create_rule,
            rules::delete_rule,
            recurring::list_recurring,
            recurring::detect_recurring,
            recurring::confirm_recurring,
            recurring::reject_recurring,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
}
