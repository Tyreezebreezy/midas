import { invoke } from "@tauri-apps/api/core";
import type { AccountType } from "../lib/accountTypes";

export interface Account {
  id: number;
  name: string;
  institution: string | null;
  account_type: AccountType;
  balance_cents: number;
  include_in_net_worth: boolean;
  notes: string | null;
  is_archived: boolean;
  last_synced_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface AccountInput {
  name: string;
  institution: string | null;
  account_type: AccountType;
  balance_cents: number;
  include_in_net_worth: boolean;
  notes: string | null;
}

export const accountsApi = {
  list: (includeArchived: boolean) =>
    invoke<Account[]>("list_accounts", { includeArchived }),
  create: (input: AccountInput) => invoke<Account>("create_account", { input }),
  update: (id: number, input: AccountInput) =>
    invoke<Account>("update_account", { id, input }),
  setArchived: (id: number, archived: boolean) =>
    invoke<Account>("set_account_archived", { id, archived }),
};
