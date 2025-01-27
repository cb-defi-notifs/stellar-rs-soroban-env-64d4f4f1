use crate::builtin_contracts::base_types::Address;
use soroban_builtin_sdk_macros::contracttype;
use soroban_env_common::TryIntoVal;

pub(crate) const DAY_IN_LEDGERS: u32 = 17280;
pub(crate) const INSTANCE_EXTEND_AMOUNT: u32 = 7 * DAY_IN_LEDGERS;
pub(crate) const INSTANCE_TTL_THRESHOLD: u32 = INSTANCE_EXTEND_AMOUNT - DAY_IN_LEDGERS;

pub(crate) const BALANCE_EXTEND_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
pub(crate) const BALANCE_TTL_THRESHOLD: u32 = BALANCE_EXTEND_AMOUNT - DAY_IN_LEDGERS;

#[contracttype]
pub struct AllowanceDataKey {
    pub from: Address,
    pub spender: Address,
}

#[contracttype]
pub struct AllowanceValue {
    pub amount: i128,
    pub live_until_ledger: u32,
}

#[contracttype]
pub struct BalanceValue {
    pub amount: i128,
    pub authorized: bool,
    pub clawback: bool,
}

/// Keys for the persistent data associated with stellar asset contract users.
#[contracttype]
pub enum DataKey {
    Allowance(AllowanceDataKey),
    Balance(Address),
}

/// Keys for stellar asset contract instance data.
#[contracttype]
pub enum InstanceDataKey {
    Admin,
    AssetInfo,
}
