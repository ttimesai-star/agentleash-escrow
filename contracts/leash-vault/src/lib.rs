#![no_std]
//! LeashVault: spending leash for an AI agent on Stellar, paying through Trustless Work escrows.
//!
//! The owner funds the Vault and sets limits; the agent can only move money with `pay`, which checks the
//! policy, deploys a Trustless Work single-release escrow from a pinned wasm hash, and funds it.
//! The seller is paid (`settle`) only against an ed25519 receipt over the result hash; without a receipt
//! the money comes back to the Vault after the deadline (`refund`, via dispute -> LeashResolver).
//! Trustless Work keeps its 0.3% fee on both paths.
//! Prototype, unaudited, testnet only.

use soroban_sdk::auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype,
    panic_with_error, symbol_short, token, vec, Address, Bytes, BytesN, ContractExecutable, Env, IntoVal,
    String,
};

pub mod tw {
    soroban_sdk::contractimport!(
        file = "../../tw/tw_escrow.wasm",
        sha256 = "7c3f7b2af92ad86092708b23babf80f9e1308d7f3ce18b703b9499192ecc934b"
    );
}

#[contractclient(name = "ResolverClient")]
pub trait Resolver {
    fn resolve(env: Env, escrow: Address, token: Address, to: Address, tw_fee: Address);
}

pub const MIN_TIMEOUT: u64 = 60;
pub const MAX_TIMEOUT: u64 = 86_400;
const TTL_MIN: u32 = 17_280; // ~1 day
const TTL_MAX: u32 = 518_400; // ~30 days
const RECEIPT_TAG: &[u8] = b"ALSH-RCPT-v1";

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    BadConfig = 1,
    AgentInactive = 2,
    PayeeNotAllowed = 3,
    BadAmount = 4,
    OverMaxPerCall = 5,
    OverWindowLimit = 6,
    BadTimeout = 7,
    InsufficientBalance = 8,
    UnknownEscrow = 9,
    NotOpen = 10,
    DeadlinePassed = 11,
    DeadlineNotReached = 12,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub owner: Address,
    pub agent: Address,
    pub agent_valid_until: u64,
    pub revoked: bool,
    pub token: Address,
    pub tw_wasm_hash: BytesN<32>,
    pub tw_fee: Address,
    pub resolver: Address,
    pub limit: i128,
    pub period_secs: u64,
    pub max_per_call: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Window {
    pub start: u64,
    pub spent: i128,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Open,
    Settled,
    Refunded,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Payment {
    pub seller: Address,
    pub amount: i128,
    pub request_hash: BytesN<32>,
    pub receipt_pk: BytesN<32>,
    pub deadline: u64,
    pub status: Status,
}

#[contracttype]
pub enum Key {
    Config,
    Window,
    Nonce,
    Payee(Address),
    Payment(Address),
}

#[contractevent(topics = ["leash", "pay"])]
pub struct Paid {
    #[topic]
    pub escrow: Address,
    pub seller: Address,
    pub amount: i128,
    pub request_hash: BytesN<32>,
    pub deadline: u64,
}

#[contractevent(topics = ["leash", "settle"])]
pub struct Settled {
    #[topic]
    pub escrow: Address,
    pub result_hash: BytesN<32>,
    pub via_dispute: bool,
}

#[contractevent(topics = ["leash", "refund"])]
pub struct Refunded {
    #[topic]
    pub escrow: Address,
}

#[contractevent(topics = ["leash", "payee"])]
pub struct PayeeSet {
    #[topic]
    pub seller: Address,
    pub receipt_pk: Option<BytesN<32>>,
}

#[contractevent(topics = ["leash", "config"])]
pub struct ConfigChanged {
    pub agent: Address,
    pub agent_valid_until: u64,
    pub revoked: bool,
    pub limit: i128,
    pub period_secs: u64,
    pub max_per_call: i128,
}

#[contractevent(topics = ["leash", "withdraw"])]
pub struct Withdrawn {
    pub to: Address,
    pub amount: i128,
}

/// sha256("ALSH-RCPT-v1" || xdr(vault) || xdr(escrow) || request_hash || result_hash) — what the seller signs.
pub fn receipt_digest(
    env: &Env,
    vault: &Address,
    escrow: &Address,
    request_hash: &BytesN<32>,
    result_hash: &BytesN<32>,
) -> BytesN<32> {
    let mut m = Bytes::from_slice(env, RECEIPT_TAG);
    m.append(&vault.clone().to_xdr(env));
    m.append(&escrow.clone().to_xdr(env));
    m.append(&Bytes::from(request_hash));
    m.append(&Bytes::from(result_hash));
    env.crypto().sha256(&m).to_bytes()
}

fn hex32(env: &Env, b: &BytesN<32>) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let a = b.to_array();
    let mut o = [0u8; 64];
    for i in 0..32 {
        o[2 * i] = H[(a[i] >> 4) as usize];
        o[2 * i + 1] = H[(a[i] & 15) as usize];
    }
    String::from_bytes(env, &o)
}

fn config(env: &Env) -> Config {
    env.storage().instance().get(&Key::Config).unwrap()
}

fn save_config(env: &Env, c: &Config) {
    if c.limit <= 0 || c.max_per_call <= 0 || c.max_per_call > c.limit || c.period_secs == 0 {
        panic_with_error!(env, Error::BadConfig);
    }
    env.storage().instance().set(&Key::Config, c);
    env.storage().instance().extend_ttl(TTL_MIN, TTL_MAX);
    ConfigChanged {
        agent: c.agent.clone(),
        agent_valid_until: c.agent_valid_until,
        revoked: c.revoked,
        limit: c.limit,
        period_secs: c.period_secs,
        max_per_call: c.max_per_call,
    }
    .publish(env);
}

fn owner_config(env: &Env) -> Config {
    let c = config(env);
    c.owner.require_auth();
    c
}

fn load_open(env: &Env, escrow: &Address) -> Result<Payment, Error> {
    let p: Payment = env
        .storage()
        .persistent()
        .get(&Key::Payment(escrow.clone()))
        .ok_or(Error::UnknownEscrow)?;
    if p.status != Status::Open {
        return Err(Error::NotOpen);
    }
    Ok(p)
}

fn store_payment(env: &Env, escrow: &Address, mut p: Payment, status: Status) {
    p.status = status;
    let k = Key::Payment(escrow.clone());
    env.storage().persistent().set(&k, &p);
    env.storage().persistent().extend_ttl(&k, TTL_MIN, TTL_MAX);
}

#[contract]
pub struct LeashVault;

#[contractimpl]
impl LeashVault {
    #[allow(clippy::too_many_arguments)]
    pub fn __constructor(
        env: Env,
        owner: Address,
        agent: Address,
        agent_valid_until: u64,
        token: Address,
        tw_wasm_hash: BytesN<32>,
        tw_fee: Address,
        resolver: Address,
        limit: i128,
        period_secs: u64,
        max_per_call: i128,
    ) {
        let c = Config {
            owner,
            agent,
            agent_valid_until,
            revoked: false,
            token,
            tw_wasm_hash,
            tw_fee,
            resolver,
            limit,
            period_secs,
            max_per_call,
        };
        save_config(&env, &c);
        let w = Window { start: env.ledger().timestamp(), spent: 0 };
        env.storage().instance().set(&Key::Window, &w);
    }

    // ---------------------------------------------------------------- owner

    pub fn set_payee(env: Env, seller: Address, receipt_pk: BytesN<32>) {
        owner_config(&env);
        let k = Key::Payee(seller.clone());
        env.storage().persistent().set(&k, &receipt_pk);
        env.storage().persistent().extend_ttl(&k, TTL_MIN, TTL_MAX);
        PayeeSet { seller, receipt_pk: Some(receipt_pk) }.publish(&env);
    }

    pub fn remove_payee(env: Env, seller: Address) {
        owner_config(&env);
        env.storage().persistent().remove(&Key::Payee(seller.clone()));
        PayeeSet { seller, receipt_pk: None }.publish(&env);
    }

    pub fn set_limits(env: Env, limit: i128, period_secs: u64, max_per_call: i128) {
        let mut c = owner_config(&env);
        c.limit = limit;
        c.period_secs = period_secs;
        c.max_per_call = max_per_call;
        save_config(&env, &c);
    }

    pub fn set_agent(env: Env, agent: Address, valid_until: u64) {
        let mut c = owner_config(&env);
        c.agent = agent;
        c.agent_valid_until = valid_until;
        c.revoked = false;
        save_config(&env, &c);
    }

    pub fn revoke_agent(env: Env) {
        let mut c = owner_config(&env);
        c.revoked = true;
        save_config(&env, &c);
    }

    /// Owner takes back free funds. Money already in escrows is not in the Vault balance.
    pub fn withdraw(env: Env, to: Address, amount: i128) {
        let c = owner_config(&env);
        token::Client::new(&env, &c.token).transfer(&env.current_contract_address(), &to, &amount);
        Withdrawn { to, amount }.publish(&env);
    }

    // ---------------------------------------------------------------- agent

    /// Pay `seller` through a fresh Trustless Work escrow. Returns the escrow address.
    pub fn pay(
        env: Env,
        seller: Address,
        amount: i128,
        request_hash: BytesN<32>,
        timeout_secs: u64,
    ) -> Result<Address, Error> {
        let c = config(&env);
        c.agent.require_auth();
        let now = env.ledger().timestamp();
        if c.revoked || now > c.agent_valid_until {
            return Err(Error::AgentInactive);
        }
        let receipt_pk: BytesN<32> = env
            .storage()
            .persistent()
            .get(&Key::Payee(seller.clone()))
            .ok_or(Error::PayeeNotAllowed)?;
        if amount <= 0 {
            return Err(Error::BadAmount);
        }
        if amount > c.max_per_call {
            return Err(Error::OverMaxPerCall);
        }
        let mut w: Window = env.storage().instance().get(&Key::Window).unwrap();
        if now >= w.start + c.period_secs {
            w = Window { start: now, spent: 0 };
        }
        let spent = w.spent.checked_add(amount).ok_or(Error::OverWindowLimit)?;
        if spent > c.limit {
            return Err(Error::OverWindowLimit);
        }
        if !(MIN_TIMEOUT..=MAX_TIMEOUT).contains(&timeout_secs) {
            return Err(Error::BadTimeout);
        }
        let me = env.current_contract_address();
        let tok = token::Client::new(&env, &c.token);
        if tok.balance(&me) < amount {
            return Err(Error::InsufficientBalance);
        }

        // State first, external calls after.
        w.spent = spent;
        env.storage().instance().set(&Key::Window, &w);
        let nonce: u64 = env.storage().instance().get(&Key::Nonce).unwrap_or(0) + 1;
        env.storage().instance().set(&Key::Nonce, &nonce);
        env.storage().instance().extend_ttl(TTL_MIN, TTL_MAX);

        let mut salt = [0u8; 32];
        salt[24..].copy_from_slice(&nonce.to_be_bytes());
        let escrow = env
            .deployer()
            .with_current_contract(BytesN::from_array(&env, &salt))
            .deploy_contract(ContractExecutable::Wasm(c.tw_wasm_hash.clone()), ());

        let deadline = now + timeout_secs;
        let p = Payment {
            seller: seller.clone(),
            amount,
            request_hash: request_hash.clone(),
            receipt_pk,
            deadline,
            status: Status::Open,
        };
        store_payment(&env, &escrow, p, Status::Open);

        let props = tw::Escrow {
            engagement_id: hex32(&env, &request_hash),
            title: String::from_str(&env, "AgentLeash payment"),
            description: String::from_str(&env, "Held until seller receipt or timeout refund"),
            roles: tw::Roles {
                approver: me.clone(),
                service_provider: seller.clone(),
                platform: me.clone(),
                release_signer: me.clone(),
                dispute_resolver: c.resolver.clone(),
                receiver: seller.clone(),
            },
            amount,
            platform_fee: 0,
            milestones: vec![
                &env,
                tw::Milestone {
                    description: String::from_str(&env, "paid API call"),
                    status: String::from_str(&env, "pending"),
                    evidence: String::from_str(&env, ""),
                    approved: false,
                },
            ],
            flags: tw::Flags { disputed: false, released: false, resolved: false },
            trustline: tw::Trustline { address: c.token.clone() },
            receiver_memo: 0,
        };
        let twc = tw::Client::new(&env, &escrow);
        let stored = twc.initialize_escrow(&props);
        // fund_escrow calls token.transfer(vault -> escrow): authorize that nested call as the Vault.
        env.authorize_as_current_contract(vec![
            &env,
            InvokerContractAuthEntry::Contract(SubContractInvocation {
                context: ContractContext {
                    contract: c.token.clone(),
                    fn_name: symbol_short!("transfer"),
                    args: (me.clone(), escrow.clone(), amount).into_val(&env),
                },
                sub_invocations: vec![&env],
            }),
        ]);
        twc.fund_escrow(&me, &stored, &amount);

        Paid { escrow: escrow.clone(), seller, amount, request_hash, deadline }.publish(&env);
        Ok(escrow)
    }

    // ---------------------------------------------------------------- anyone

    /// Release to the seller against its receipt signature (before or at the deadline).
    pub fn settle(env: Env, escrow: Address, result_hash: BytesN<32>, sig: BytesN<64>) -> Result<(), Error> {
        let p = load_open(&env, &escrow)?;
        if env.ledger().timestamp() > p.deadline {
            return Err(Error::DeadlinePassed);
        }
        let me = env.current_contract_address();
        let digest = receipt_digest(&env, &me, &escrow, &p.request_hash, &result_hash);
        env.crypto().ed25519_verify(&p.receipt_pk, &Bytes::from(&digest), &sig);
        let seller = p.seller.clone();
        store_payment(&env, &escrow, p, Status::Settled);

        let c = config(&env);
        let twc = tw::Client::new(&env, &escrow);
        let via_dispute = twc.get_escrow().flags.disputed;
        if via_dispute {
            // The seller opened a dispute itself; a valid receipt still pays it, through the resolver.
            ResolverClient::new(&env, &c.resolver).resolve(&escrow, &c.token, &seller, &c.tw_fee);
        } else {
            twc.approve_milestone(&0, &me);
            twc.release_funds(&me, &c.tw_fee);
        }
        Settled { escrow, result_hash, via_dispute }.publish(&env);
        Ok(())
    }

    /// No receipt by the deadline: dispute the escrow and resolve it back to the Vault.
    pub fn refund(env: Env, escrow: Address) -> Result<(), Error> {
        let p = load_open(&env, &escrow)?;
        if env.ledger().timestamp() <= p.deadline {
            return Err(Error::DeadlineNotReached);
        }
        store_payment(&env, &escrow, p, Status::Refunded);

        let c = config(&env);
        let me = env.current_contract_address();
        let twc = tw::Client::new(&env, &escrow);
        if !twc.get_escrow().flags.disputed {
            twc.dispute_escrow(&me);
        }
        ResolverClient::new(&env, &c.resolver).resolve(&escrow, &c.token, &me, &c.tw_fee);
        Refunded { escrow }.publish(&env);
        Ok(())
    }

    // ---------------------------------------------------------------- read

    pub fn config(env: Env) -> Config {
        config(&env)
    }

    pub fn window(env: Env) -> Window {
        env.storage().instance().get(&Key::Window).unwrap()
    }

    pub fn payee(env: Env, seller: Address) -> Option<BytesN<32>> {
        env.storage().persistent().get(&Key::Payee(seller))
    }

    pub fn payment(env: Env, escrow: Address) -> Result<Payment, Error> {
        env.storage()
            .persistent()
            .get(&Key::Payment(escrow))
            .ok_or(Error::UnknownEscrow)
    }
}

#[cfg(test)]
mod test;
