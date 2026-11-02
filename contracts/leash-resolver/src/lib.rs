#![no_std]
//! LeashResolver: the `dispute_resolver` role of every Trustless Work escrow opened by one LeashVault.
//! A TW dispute resolver may not open a dispute itself, so the Vault (approver) opens it and this
//! contract resolves it. It obeys only the Vault fixed at deploy time.
//! Prototype, unaudited, testnet only.

use soroban_sdk::{contract, contractclient, contractimpl, symbol_short, token, Address, Env, Map, Symbol};

const VAULT: Symbol = symbol_short!("VAULT");

/// The one Trustless Work escrow function this contract calls.
#[contractclient(name = "TwEscrowClient")]
pub trait TwEscrow {
    fn resolve_dispute(
        env: Env,
        dispute_resolver: Address,
        trustless_work_address: Address,
        distributions: Map<Address, i128>,
    );
}

#[contract]
pub struct LeashResolver;

#[contractimpl]
impl LeashResolver {
    pub fn __constructor(env: Env, vault: Address) {
        env.storage().instance().set(&VAULT, &vault);
        env.storage().instance().extend_ttl(17_280, 518_400);
    }

    /// Resolve a disputed escrow: the whole escrow balance goes to `to` (TW takes its 0.3% fee).
    pub fn resolve(env: Env, escrow: Address, token: Address, to: Address, tw_fee: Address) {
        let vault: Address = env.storage().instance().get(&VAULT).unwrap();
        vault.require_auth();
        env.storage().instance().extend_ttl(17_280, 518_400);
        let balance = token::Client::new(&env, &token).balance(&escrow);
        let mut dist = Map::new(&env);
        dist.set(to, balance);
        TwEscrowClient::new(&env, &escrow).resolve_dispute(&env.current_contract_address(), &tw_fee, &dist);
    }

    pub fn vault(env: Env) -> Address {
        env.storage().instance().get(&VAULT).unwrap()
    }
}
