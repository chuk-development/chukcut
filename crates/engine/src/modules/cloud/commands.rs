//! Commands for cloud accounts.
//!
//! Keys go in and never come out: there is no command that returns one, only
//! [`AccountView::key_hint`], the last four characters.

use serde::Serialize;

use super::providers::openai_compat::OpenAiCompatible;
use super::registry::{self, Account, CloudStore, ProviderDescriptor, ProviderKind};
use super::secrets;
use super::{Capability, TestReport};

/// An account as the settings UI shows it.
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    pub account: Account,
    pub has_key: bool,
    /// `••••abcd`, or empty when there is no key.
    pub key_hint: String,
}

/// Every provider kind chukcut knows.
pub fn cloud_providers() -> &'static [ProviderDescriptor] {
    registry::PROVIDERS
}

/// The configured accounts, optionally only those that can do `capability`.
pub fn cloud_accounts(store: &CloudStore, capability: Option<Capability>) -> Vec<AccountView> {
    let secrets = store.secrets();
    store
        .accounts()
        .into_iter()
        .filter(|a| capability.is_none_or(|c| a.can(c)))
        .map(|account| {
            let key = secrets.key(&account.id);
            AccountView {
                has_key: key.is_some(),
                key_hint: key.as_deref().map(secrets::masked).unwrap_or_default(),
                account,
            }
        })
        .collect()
}

/// Add or update an account. `key: None` keeps the stored key, `Some("")`
/// deletes it. Returns the account id.
pub fn cloud_account_set(
    store: &CloudStore,
    account: Account,
    key: Option<&str>,
) -> Result<String, String> {
    store.save_account(&account, key)?;
    Ok(account.id)
}

/// A new OpenAI-compatible account from one of the presets, or a blank one.
pub fn cloud_account_new(name: &str, base_url: &str, model: &str) -> Account {
    Account::new(ProviderKind::OpenaiCompatible, name, base_url, model)
}

pub fn cloud_account_remove(store: &CloudStore, account_id: &str) -> Result<(), String> {
    store.remove_account(account_id)
}

/// One cheap call that proves the URL and key work. Blocking; run it off the
/// UI thread.
pub fn cloud_account_test(store: &CloudStore, account_id: &str) -> TestReport {
    let Some(account) = store.account(account_id) else {
        return TestReport {
            ok: false,
            message: "that account no longer exists".to_string(),
            models: Vec::new(),
        };
    };
    let key = store.key(&account.id);
    match account.kind {
        ProviderKind::OpenaiCompatible => OpenAiCompatible { account, key }.test(),
    }
}
