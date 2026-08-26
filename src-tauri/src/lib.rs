mod accounting;
mod accounts;
mod budgets;
mod dates;
mod db;
mod import;
mod money;
mod needs_review;
mod pay_cycle;
mod recurring;
mod rules;
mod safe_to_spend;
mod savings_goals;
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
            accounting::reconcile_account_balance,
            accounting::get_net_worth,
            accounting::get_total_spending,
            budgets::list_budgets,
            budgets::create_budget,
            budgets::update_budget,
            budgets::delete_budget,
            savings_goals::list_savings_goals,
            savings_goals::create_savings_goal,
            savings_goals::update_savings_goal,
            savings_goals::set_savings_goal_archived,
            pay_cycle::get_pay_schedule,
            pay_cycle::set_pay_schedule,
            pay_cycle::get_pay_cycle_summary,
            pay_cycle::get_likely_income_series,
            safe_to_spend::get_safe_to_spend,
            safe_to_spend::get_safe_to_spend_reserve,
            safe_to_spend::set_safe_to_spend_reserve,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
}
