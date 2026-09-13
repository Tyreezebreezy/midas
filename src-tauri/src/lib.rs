mod accounting;
mod accounts;
mod backup;
mod budgets;
mod bulk;
mod categories;
mod data_health;
mod dates;
mod db;
mod forecast;
mod import;
mod import_profiles;
mod insights;
mod money;
mod needs_review;
mod ofx;
mod ofx_accounts;
mod overview;
mod pay_cycle;
mod payees;
mod reconciliation;
mod recurring;
mod recurring_occurrences;
mod reports;
mod review;
mod rules;
mod safe_to_spend;
mod savings_goals;
mod settings;
mod signals;
mod splits;
mod tags;
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
            transactions::list_transactions,
            transactions::create_transaction,
            transactions::update_transaction,
            transactions::set_transaction_archived,
            transactions::set_transaction_reconciliation_status,
            transactions::delete_transaction,
            transactions::export_transactions_csv,
            import::preview_csv,
            import::stage_import,
            import::commit_import,
            import::list_imports,
            import::preview_undo_import,
            import::undo_import,
            import::preview_ofx,
            import::stage_ofx,
            import::commit_ofx_import,
            needs_review::list_needs_review,
            needs_review::resolve_needs_review,
            rules::list_rules,
            rules::create_rule,
            rules::delete_rule,
            recurring::list_recurring,
            recurring::detect_recurring,
            recurring::confirm_recurring,
            recurring::reject_recurring,
            recurring::set_recurring_inactive,
            recurring::reactivate_recurring,
            recurring::set_recurring_planning_to_cancel,
            recurring::create_manual_recurring,
            recurring::update_recurring_pattern,
            recurring::move_recurring_account,
            recurring::get_recurring_price_history,
            recurring_occurrences::list_recurring_occurrences,
            recurring_occurrences::get_recurring_occurrence,
            recurring_occurrences::list_upcoming_recurring,
            recurring_occurrences::skip_recurring_occurrence,
            recurring_occurrences::link_recurring_occurrence,
            recurring_occurrences::unlink_recurring_occurrence,
            accounting::reconcile_account_balance,
            accounting::get_net_worth,
            accounting::get_net_worth_history,
            accounting::get_net_worth_change,
            accounting::get_total_spending,
            budgets::list_budgets,
            budgets::create_budget,
            budgets::update_budget,
            budgets::delete_budget,
            budgets::get_budget_suggestions,
            budgets::copy_budgets_from_month,
            budgets::get_unbudgeted_spending,
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
            overview::get_overview,
            reports::get_spending_by_category,
            reports::get_income_vs_spending_trend,
            backup::create_backup_command,
            backup::preview_restore_command,
            backup::restore_backup_command,
            payees::list_payees,
            payees::create_payee,
            payees::update_payee,
            payees::set_payee_archived,
            payees::merge_payees,
            tags::list_tags,
            tags::create_tag,
            tags::delete_tag,
            tags::attach_tag,
            tags::detach_tag,
            tags::list_tags_for_transaction,
            import_profiles::list_import_profiles,
            import_profiles::create_import_profile,
            import_profiles::update_import_profile,
            import_profiles::delete_import_profile,
            import_profiles::set_import_profile_archived,
            reconciliation::get_reconciliation_preview,
            reconciliation::complete_reconciliation,
            reconciliation::reopen_reconciliation,
            reconciliation::list_reconciliations,
            reconciliation::get_reconciliation_summary,
            reconciliation::get_latest_statement_hint,
            reconciliation::get_running_balances,
            splits::list_splits,
            splits::set_splits,
            categories::rename_category,
            bulk::bulk_set_category,
            bulk::bulk_set_payee,
            bulk::bulk_set_one_time,
            bulk::bulk_set_archived,
            bulk::bulk_add_tag,
            bulk::bulk_remove_tag,
            rules::apply_correction_command,
            insights::get_spending_insights,
            insights::get_category_detail,
            insights::get_income_insights,
            insights::get_top_merchants,
            insights::get_merchant_detail,
            insights::get_cash_flow_insights,
            forecast::get_forecast,
            review::list_review,
            review::keep_duplicate_review_transaction,
            data_health::get_data_health,
            signals::get_signals,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
}
