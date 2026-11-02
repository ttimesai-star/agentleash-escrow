extern crate std;

use super::*;
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::testutils::{Address as _, Events, Ledger, MockAuth, MockAuthInvoke};
use soroban_sdk::token::{StellarAssetClient, TokenClient};

const T0: u64 = 1_000_000;
const UNIT: i128 = 10_000_000; // 7 decimals, like testnet USDC
const LIMIT: i128 = 1_000 * UNIT;
const PERIOD: u64 = 3_600;
const MAX_CALL: i128 = 300 * UNIT;
const FUNDED: i128 = 2_000 * UNIT;

struct T {
    env: Env,
    vault: Address,
    v: LeashVaultClient<'static>,
    token: TokenClient<'static>,
    owner: Address,
    agent: Address,
    seller: Address,
    resolver: Address,
    tw_fee: Address,
    sk: SigningKey,
}

fn setup() -> T {
    let env = Env::default();
    env.ledger().set_timestamp(T0);
    env.mock_all_auths();
    let owner = Address::generate(&env);
    let agent = Address::generate(&env);
    let seller = Address::generate(&env);
    let tw_fee = Address::generate(&env);
    let tok = env.register_stellar_asset_contract_v2(owner.clone()).address();
    let tw_hash = env.deployer().upload_contract_wasm(tw::WASM);
    let resolver = Address::generate(&env);
    let vault = env.register(
        LeashVault,
        (
            owner.clone(),
            agent.clone(),
            T0 + 30 * 86_400,
            tok.clone(),
            tw_hash,
            tw_fee.clone(),
            resolver.clone(),
            LIMIT,
            PERIOD,
            MAX_CALL,
        ),
    );
    env.register_at(&resolver, leash_resolver::LeashResolver, (vault.clone(),));
    StellarAssetClient::new(&env, &tok).mint(&vault, &FUNDED);
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let v = LeashVaultClient::new(&env, &vault);
    v.set_payee(&seller, &pk(&env, &sk));
    // From here on no blanket mocks: every require_auth must be satisfied for real or by an explicit mock.
    env.set_auths(&[]);
    let token = TokenClient::new(&env, &tok);
    T { env, vault, v, token, owner, agent, seller, resolver, tw_fee, sk }
}

fn pk(env: &Env, sk: &SigningKey) -> BytesN<32> {
    BytesN::from_array(env, &sk.verifying_key().to_bytes())
}

fn h(env: &Env, b: u8) -> BytesN<32> {
    BytesN::from_array(env, &[b; 32])
}

fn mock(t: &T, who: &Address, f: &'static str, args: soroban_sdk::Vec<soroban_sdk::Val>) {
    t.env.mock_auths(&[MockAuth {
        address: who,
        invoke: &MockAuthInvoke { contract: &t.vault, fn_name: f, args, sub_invokes: &[] },
    }]);
}

fn try_pay_as(t: &T, who: &Address, seller: &Address, amount: i128, timeout: u64) -> Result<Address, Error> {
    let rh = h(&t.env, 1);
    mock(t, who, "pay", (seller.clone(), amount, rh.clone(), timeout).into_val(&t.env));
    match t.v.try_pay(seller, &amount, &rh, &timeout) {
        Ok(Ok(a)) => Ok(a),
        Err(Ok(e)) => Err(e),
        other => panic!("unexpected: {:?}", other),
    }
}

fn try_pay(t: &T, amount: i128, timeout: u64) -> Result<Address, Error> {
    try_pay_as(t, &t.agent.clone(), &t.seller.clone(), amount, timeout)
}

fn pay(t: &T, amount: i128) -> Address {
    try_pay(t, amount, 600).unwrap()
}

fn sign_with(t: &T, sk: &SigningKey, escrow: &Address, result: &BytesN<32>) -> BytesN<64> {
    let p = t.v.payment(escrow);
    let d = receipt_digest(&t.env, &t.vault, escrow, &p.request_hash, result);
    BytesN::from_array(&t.env, &sk.sign(&d.to_array()).to_bytes())
}

fn sign(t: &T, escrow: &Address, result: &BytesN<32>) -> BytesN<64> {
    sign_with(t, &t.sk, escrow, result)
}

fn advance(t: &T, secs: u64) {
    let now = t.env.ledger().timestamp();
    t.env.ledger().set_timestamp(now + secs);
}

fn net(amount: i128) -> i128 {
    amount - amount * 30 / 10_000
}

fn tw_escrow(t: &T, escrow: &Address) -> tw::Escrow {
    tw::Client::new(&t.env, escrow).get_escrow()
}

// ---------------------------------------------------------------- S4 spike: the Trustless Work wiring

#[test]
fn spike_release_through_tw_escrow() {
    let t = setup();
    let amount = 100 * UNIT;
    let e = pay(&t, amount);
    assert_eq!(t.token.balance(&e), amount);
    assert_eq!(t.token.balance(&t.vault), FUNDED - amount);
    let r = h(&t.env, 9);
    t.v.settle(&e, &r, &sign(&t, &e, &r));
    assert_eq!(t.token.balance(&t.seller), net(amount));
    assert_eq!(t.token.balance(&t.tw_fee), amount * 30 / 10_000);
    assert_eq!(t.token.balance(&e), 0);
    let s = tw_escrow(&t, &e);
    assert!(s.flags.released && s.milestones.get(0).unwrap().approved);
    assert_eq!(t.v.payment(&e).status, Status::Settled);
}

#[test]
fn spike_refund_through_tw_dispute_and_resolver() {
    let t = setup();
    let amount = 100 * UNIT;
    let e = pay(&t, amount);
    advance(&t, 601);
    t.v.refund(&e);
    assert_eq!(t.token.balance(&t.vault), FUNDED - amount + net(amount));
    assert_eq!(t.token.balance(&t.seller), 0);
    assert_eq!(t.token.balance(&e), 0);
    let s = tw_escrow(&t, &e);
    assert!(s.flags.resolved && !s.flags.released);
    assert_eq!(t.v.payment(&e).status, Status::Refunded);
}

// ---------------------------------------------------------------- pay: policy

#[test]
fn pay_creates_tw_escrow_with_vault_roles() {
    let t = setup();
    let e = pay(&t, 10 * UNIT);
    let s = tw_escrow(&t, &e);
    assert_eq!(s.roles.approver, t.vault);
    assert_eq!(s.roles.platform, t.vault);
    assert_eq!(s.roles.release_signer, t.vault);
    assert_eq!(s.roles.dispute_resolver, t.resolver);
    assert_eq!(s.roles.service_provider, t.seller);
    assert_eq!(s.roles.receiver, t.seller);
    assert_eq!(s.amount, 10 * UNIT);
    assert_eq!(s.platform_fee, 0);
    assert_eq!(s.trustline.address, t.token.address);
    let p = t.v.payment(&e);
    assert_eq!(p.deadline, T0 + 600);
    assert_eq!(p.receipt_pk, pk(&t.env, &t.sk));
    assert_eq!(p.status, Status::Open);
    assert_eq!(t.v.window().spent, 10 * UNIT);
}

#[test]
fn pay_twice_gives_distinct_escrows() {
    let t = setup();
    let a = pay(&t, UNIT);
    let b = pay(&t, UNIT);
    assert_ne!(a, b);
}

#[test]
fn pay_rejects_unknown_payee() {
    let t = setup();
    let stranger = Address::generate(&t.env);
    assert_eq!(try_pay_as(&t, &t.agent, &stranger, UNIT, 600), Err(Error::PayeeNotAllowed));
}

#[test]
fn pay_rejects_over_max_per_call() {
    let t = setup();
    assert_eq!(try_pay(&t, MAX_CALL + 1, 600), Err(Error::OverMaxPerCall));
    assert!(try_pay(&t, MAX_CALL, 600).is_ok());
}

#[test]
fn pay_rejects_zero_and_negative_amount() {
    let t = setup();
    assert_eq!(try_pay(&t, 0, 600), Err(Error::BadAmount));
    assert_eq!(try_pay(&t, -5, 600), Err(Error::BadAmount));
}

#[test]
fn pay_rejects_over_window_limit() {
    let t = setup();
    pay(&t, MAX_CALL);
    pay(&t, MAX_CALL);
    pay(&t, MAX_CALL);
    assert_eq!(try_pay(&t, 100 * UNIT + 1, 600), Err(Error::OverWindowLimit));
    assert!(try_pay(&t, 100 * UNIT, 600).is_ok());
    assert_eq!(t.v.window().spent, LIMIT);
}

#[test]
fn window_resets_exactly_at_period_end() {
    let t = setup();
    pay(&t, MAX_CALL);
    pay(&t, MAX_CALL);
    pay(&t, MAX_CALL);
    pay(&t, 100 * UNIT);
    advance(&t, PERIOD - 1);
    assert_eq!(try_pay(&t, UNIT, 600), Err(Error::OverWindowLimit));
    advance(&t, 1);
    assert!(try_pay(&t, UNIT, 600).is_ok());
    assert_eq!(t.v.window(), Window { start: T0 + PERIOD, spent: UNIT });
}

#[test]
fn refund_does_not_restore_window() {
    let t = setup();
    let e = pay(&t, MAX_CALL);
    advance(&t, 601);
    t.v.refund(&e);
    assert_eq!(t.v.window().spent, MAX_CALL);
}

#[test]
fn pay_rejects_bad_timeout() {
    let t = setup();
    assert_eq!(try_pay(&t, UNIT, MIN_TIMEOUT - 1), Err(Error::BadTimeout));
    assert_eq!(try_pay(&t, UNIT, MAX_TIMEOUT + 1), Err(Error::BadTimeout));
    assert!(try_pay(&t, UNIT, MIN_TIMEOUT).is_ok());
    assert!(try_pay(&t, UNIT, MAX_TIMEOUT).is_ok());
}

#[test]
fn pay_rejects_insufficient_balance() {
    let t = setup();
    mock(&t, &t.owner, "withdraw", (t.owner.clone(), FUNDED - 5 * UNIT).into_val(&t.env));
    t.v.withdraw(&t.owner, &(FUNDED - 5 * UNIT));
    assert_eq!(try_pay(&t, 6 * UNIT, 600), Err(Error::InsufficientBalance));
    assert!(try_pay(&t, 5 * UNIT, 600).is_ok());
}

#[test]
fn pay_rejects_revoked_agent() {
    let t = setup();
    mock(&t, &t.owner, "revoke_agent", ().into_val(&t.env));
    t.v.revoke_agent();
    assert_eq!(try_pay(&t, UNIT, 600), Err(Error::AgentInactive));
}

#[test]
fn pay_rejects_expired_agent() {
    let t = setup();
    advance(&t, 30 * 86_400);
    assert!(try_pay(&t, UNIT, 600).is_ok()); // valid_until itself is still valid
    advance(&t, 1);
    assert_eq!(try_pay(&t, UNIT, 600), Err(Error::AgentInactive));
}

#[test]
fn pay_requires_agent_signature() {
    let t = setup();
    let rh = h(&t.env, 1);
    assert!(t.v.try_pay(&t.seller, &UNIT, &rh, &600).is_err()); // nobody signed
    assert!(try_pay_as_unchecked(&t, &t.owner)); // owner is not the agent
    assert!(try_pay_as_unchecked(&t, &t.seller));
}

fn try_pay_as_unchecked(t: &T, who: &Address) -> bool {
    let rh = h(&t.env, 1);
    mock(t, who, "pay", (t.seller.clone(), UNIT, rh.clone(), 600u64).into_val(&t.env));
    t.v.try_pay(&t.seller, &UNIT, &rh, &600).is_err()
}

#[test]
fn set_agent_replaces_agent_and_clears_revoke() {
    let t = setup();
    mock(&t, &t.owner, "revoke_agent", ().into_val(&t.env));
    t.v.revoke_agent();
    let a2 = Address::generate(&t.env);
    mock(&t, &t.owner, "set_agent", (a2.clone(), T0 + 100).into_val(&t.env));
    t.v.set_agent(&a2, &(T0 + 100));
    assert!(try_pay_as(&t, &a2, &t.seller, UNIT, 600).is_ok());
    assert!(try_pay_as_unchecked(&t, &t.agent));
    assert!(!t.v.config().revoked);
}

#[test]
fn removed_payee_cannot_be_paid() {
    let t = setup();
    mock(&t, &t.owner, "remove_payee", (t.seller.clone(),).into_val(&t.env));
    t.v.remove_payee(&t.seller);
    assert_eq!(t.v.payee(&t.seller), None);
    assert_eq!(try_pay(&t, UNIT, 600), Err(Error::PayeeNotAllowed));
}

// ---------------------------------------------------------------- owner functions

#[test]
fn only_owner_can_configure() {
    let t = setup();
    let x = Address::generate(&t.env);
    let k = h(&t.env, 3);
    for who in [t.agent.clone(), t.seller.clone()] {
        mock(&t, &who, "set_payee", (x.clone(), k.clone()).into_val(&t.env));
        assert!(t.v.try_set_payee(&x, &k).is_err());
        mock(&t, &who, "remove_payee", (t.seller.clone(),).into_val(&t.env));
        assert!(t.v.try_remove_payee(&t.seller).is_err());
        mock(&t, &who, "set_limits", (LIMIT * 2, PERIOD, MAX_CALL).into_val(&t.env));
        assert!(t.v.try_set_limits(&(LIMIT * 2), &PERIOD, &MAX_CALL).is_err());
        mock(&t, &who, "set_agent", (x.clone(), T0).into_val(&t.env));
        assert!(t.v.try_set_agent(&x, &T0).is_err());
        mock(&t, &who, "revoke_agent", ().into_val(&t.env));
        assert!(t.v.try_revoke_agent().is_err());
        mock(&t, &who, "withdraw", (who.clone(), UNIT).into_val(&t.env));
        assert!(t.v.try_withdraw(&who, &UNIT).is_err());
    }
    assert_eq!(t.token.balance(&t.vault), FUNDED);
    assert_eq!(t.v.config().limit, LIMIT);
}

#[test]
fn set_limits_validates() {
    let t = setup();
    for (l, p, m) in [(0, PERIOD, 0), (LIMIT, PERIOD, LIMIT + 1), (LIMIT, 0, MAX_CALL), (LIMIT, PERIOD, 0)] {
        mock(&t, &t.owner, "set_limits", (l, p, m).into_val(&t.env));
        assert_eq!(t.v.try_set_limits(&l, &p, &m), Err(Ok(Error::BadConfig.into())));
    }
    mock(&t, &t.owner, "set_limits", (50 * UNIT, 60u64, 10 * UNIT).into_val(&t.env));
    t.v.set_limits(&(50 * UNIT), &60, &(10 * UNIT));
    assert_eq!(try_pay(&t, 11 * UNIT, 600), Err(Error::OverMaxPerCall));
}

#[test]
#[should_panic]
fn constructor_rejects_max_per_call_above_limit() {
    let env = Env::default();
    let a = Address::generate(&env);
    env.register(
        LeashVault,
        (a.clone(), a.clone(), 1u64, a.clone(), h(&env, 0), a.clone(), a.clone(), 10i128, 60u64, 11i128),
    );
}

#[test]
fn withdraw_moves_only_free_funds() {
    let t = setup();
    let e = pay(&t, 100 * UNIT);
    let free = FUNDED - 100 * UNIT;
    mock(&t, &t.owner, "withdraw", (t.owner.clone(), free + 1).into_val(&t.env));
    assert!(t.v.try_withdraw(&t.owner, &(free + 1)).is_err());
    mock(&t, &t.owner, "withdraw", (t.owner.clone(), free).into_val(&t.env));
    t.v.withdraw(&t.owner, &free);
    assert_eq!(t.token.balance(&t.owner), free);
    assert_eq!(t.token.balance(&e), 100 * UNIT);
    // the open payment still settles normally
    let r = h(&t.env, 9);
    t.v.settle(&e, &r, &sign(&t, &e, &r));
    assert_eq!(t.token.balance(&t.seller), net(100 * UNIT));
}

// ---------------------------------------------------------------- settle

#[test]
fn settle_at_exact_deadline_is_allowed() {
    let t = setup();
    let e = pay(&t, UNIT);
    advance(&t, 600);
    let r = h(&t.env, 9);
    t.v.settle(&e, &r, &sign(&t, &e, &r));
    assert_eq!(t.v.payment(&e).status, Status::Settled);
}

#[test]
fn settle_after_deadline_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    advance(&t, 601);
    let r = h(&t.env, 9);
    assert_eq!(t.v.try_settle(&e, &r, &sign(&t, &e, &r)), Err(Ok(Error::DeadlinePassed)));
}

#[test]
fn settle_twice_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    let r = h(&t.env, 9);
    let s = sign(&t, &e, &r);
    t.v.settle(&e, &r, &s);
    assert_eq!(t.v.try_settle(&e, &r, &s), Err(Ok(Error::NotOpen)));
    assert_eq!(t.token.balance(&t.seller), net(UNIT));
}

#[test]
fn settle_after_refund_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    let r = h(&t.env, 9);
    let s = sign(&t, &e, &r);
    advance(&t, 601);
    t.v.refund(&e);
    assert_eq!(t.v.try_settle(&e, &r, &s), Err(Ok(Error::NotOpen)));
}

#[test]
fn refund_after_settle_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    let r = h(&t.env, 9);
    t.v.settle(&e, &r, &sign(&t, &e, &r));
    advance(&t, 601);
    assert_eq!(t.v.try_refund(&e), Err(Ok(Error::NotOpen)));
}

#[test]
fn settle_unknown_escrow_is_refused() {
    let t = setup();
    let x = Address::generate(&t.env);
    let r = h(&t.env, 9);
    let s = BytesN::from_array(&t.env, &[0u8; 64]);
    assert_eq!(t.v.try_settle(&x, &r, &s), Err(Ok(Error::UnknownEscrow)));
    assert_eq!(t.v.try_refund(&x), Err(Ok(Error::UnknownEscrow)));
    assert_eq!(t.v.try_payment(&x), Err(Ok(Error::UnknownEscrow)));
}

#[test]
fn forged_receipt_wrong_key_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    let r = h(&t.env, 9);
    let other = SigningKey::from_bytes(&[8u8; 32]);
    assert!(t.v.try_settle(&e, &r, &sign_with(&t, &other, &e, &r)).is_err());
    assert_eq!(t.v.payment(&e).status, Status::Open);
    assert_eq!(t.token.balance(&e), UNIT);
}

#[test]
fn forged_receipt_other_result_hash_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    let s = sign(&t, &e, &h(&t.env, 9));
    assert!(t.v.try_settle(&e, &h(&t.env, 10), &s).is_err());
    assert_eq!(t.v.payment(&e).status, Status::Open);
}

#[test]
fn receipt_of_another_escrow_is_refused() {
    let t = setup();
    let e1 = pay(&t, UNIT);
    let e2 = pay(&t, UNIT);
    let r = h(&t.env, 9);
    let s1 = sign(&t, &e1, &r);
    assert!(t.v.try_settle(&e2, &r, &s1).is_err());
    t.v.settle(&e1, &r, &s1);
    assert_eq!(t.v.payment(&e2).status, Status::Open);
}

#[test]
fn settle_uses_key_recorded_at_pay_time() {
    let t = setup();
    let e = pay(&t, UNIT);
    let new_sk = SigningKey::from_bytes(&[9u8; 32]);
    mock(&t, &t.owner, "set_payee", (t.seller.clone(), pk(&t.env, &new_sk)).into_val(&t.env));
    t.v.set_payee(&t.seller, &pk(&t.env, &new_sk));
    let r = h(&t.env, 9);
    assert!(t.v.try_settle(&e, &r, &sign_with(&t, &new_sk, &e, &r)).is_err());
    mock(&t, &t.owner, "remove_payee", (t.seller.clone(),).into_val(&t.env));
    t.v.remove_payee(&t.seller);
    t.v.settle(&e, &r, &sign(&t, &e, &r)); // old key, payee removed: the open payment is still honoured
    assert_eq!(t.token.balance(&t.seller), net(UNIT));
}

#[test]
fn settle_pays_seller_through_resolver_if_seller_disputed() {
    let t = setup();
    let e = pay(&t, 100 * UNIT);
    t.env.mock_auths(&[MockAuth {
        address: &t.seller,
        invoke: &MockAuthInvoke { contract: &e, fn_name: "dispute_escrow", args: (t.seller.clone(),).into_val(&t.env), sub_invokes: &[] },
    }]);
    tw::Client::new(&t.env, &e).dispute_escrow(&t.seller);
    assert!(tw_escrow(&t, &e).flags.disputed);
    let r = h(&t.env, 9);
    t.v.settle(&e, &r, &sign(&t, &e, &r));
    assert_eq!(t.token.balance(&t.seller), net(100 * UNIT));
    assert!(tw_escrow(&t, &e).flags.resolved);
}

// ---------------------------------------------------------------- refund

#[test]
fn refund_before_or_at_deadline_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    assert_eq!(t.v.try_refund(&e), Err(Ok(Error::DeadlineNotReached)));
    advance(&t, 600);
    assert_eq!(t.v.try_refund(&e), Err(Ok(Error::DeadlineNotReached)));
    advance(&t, 1);
    t.v.refund(&e);
}

#[test]
fn refund_twice_is_refused() {
    let t = setup();
    let e = pay(&t, UNIT);
    advance(&t, 601);
    t.v.refund(&e);
    assert_eq!(t.v.try_refund(&e), Err(Ok(Error::NotOpen)));
}

#[test]
fn refund_works_when_seller_already_disputed() {
    let t = setup();
    let e = pay(&t, 100 * UNIT);
    t.env.mock_auths(&[MockAuth {
        address: &t.seller,
        invoke: &MockAuthInvoke { contract: &e, fn_name: "dispute_escrow", args: (t.seller.clone(),).into_val(&t.env), sub_invokes: &[] },
    }]);
    tw::Client::new(&t.env, &e).dispute_escrow(&t.seller);
    advance(&t, 601);
    t.v.refund(&e);
    assert_eq!(t.token.balance(&t.vault), FUNDED - 100 * UNIT + net(100 * UNIT));
}

#[test]
fn nobody_else_can_release_or_resolve_the_escrow() {
    let t = setup();
    let e = pay(&t, UNIT);
    let twc = tw::Client::new(&t.env, &e);
    // seller cannot approve or release its own payment, even signing for itself
    t.env.mock_all_auths();
    assert!(twc.try_approve_milestone(&0, &t.seller).is_err());
    assert!(twc.try_release_funds(&t.seller, &t.seller).is_err());
    t.env.set_auths(&[]);
    assert_eq!(t.token.balance(&e), UNIT);
}

// ---------------------------------------------------------------- resolver

#[test]
fn resolver_obeys_only_its_vault() {
    let t = setup();
    let e = pay(&t, UNIT);
    t.env.mock_auths(&[MockAuth {
        address: &t.seller,
        invoke: &MockAuthInvoke { contract: &e, fn_name: "dispute_escrow", args: (t.seller.clone(),).into_val(&t.env), sub_invokes: &[] },
    }]);
    tw::Client::new(&t.env, &e).dispute_escrow(&t.seller);
    let rc = leash_resolver::LeashResolverClient::new(&t.env, &t.resolver);
    assert_eq!(rc.vault(), t.vault);
    // nobody signed
    assert!(rc.try_resolve(&e, &t.token.address, &t.seller, &t.seller).is_err());
    // the seller signs as itself: still not the vault
    t.env.mock_auths(&[MockAuth {
        address: &t.seller,
        invoke: &MockAuthInvoke {
            contract: &t.resolver,
            fn_name: "resolve",
            args: (e.clone(), t.token.address.clone(), t.seller.clone(), t.seller.clone()).into_val(&t.env),
            sub_invokes: &[],
        },
    }]);
    assert!(rc.try_resolve(&e, &t.token.address, &t.seller, &t.seller).is_err());
    assert_eq!(t.token.balance(&e), UNIT);
}

#[test]
fn receipt_digest_binds_every_field() {
    let t = setup();
    let x = Address::generate(&t.env);
    let base = receipt_digest(&t.env, &t.vault, &x, &h(&t.env, 1), &h(&t.env, 2));
    assert_ne!(base, receipt_digest(&t.env, &x, &x, &h(&t.env, 1), &h(&t.env, 2)));
    assert_ne!(base, receipt_digest(&t.env, &t.vault, &t.vault, &h(&t.env, 1), &h(&t.env, 2)));
    assert_ne!(base, receipt_digest(&t.env, &t.vault, &x, &h(&t.env, 3), &h(&t.env, 2)));
    assert_ne!(base, receipt_digest(&t.env, &t.vault, &x, &h(&t.env, 1), &h(&t.env, 3)));
}

/// Same vector is checked in test/receipt.test.ts: the TS seller and the contract hash identical bytes.
#[test]
fn receipt_digest_matches_ts_vector() {
    let env = Env::default();
    let vault = Address::from_str(&env, "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA");
    let escrow = Address::from_str(&env, "CAILWU67NWQKJ6OKHPLUZDMMTH2CFLF2YCDBOGGGDLQPNR6NCQDXIDUX");
    let d = receipt_digest(&env, &vault, &escrow, &h(&env, 1), &h(&env, 2));
    let expected = "e7eb3e14b50d974478a6d3ec5af39297a27c90b6b45ba03799ce7a1aa2056346";
    let mut hex = std::string::String::new();
    for b in d.to_array() {
        hex.push_str(&std::format!("{:02x}", b));
    }
    assert_eq!(hex, expected);
}

// ---------------------------------------------------------------- events (audit trail)

fn ev(t: &T, e: &impl soroban_sdk::events::Event) -> soroban_sdk::xdr::ContractEvent {
    e.to_xdr(&t.env, &t.vault)
}

#[test]
fn every_flow_emits_its_vault_event() {
    let t = setup();
    let rh = h(&t.env, 1);
    mock(&t, &t.agent, "pay", (t.seller.clone(), UNIT, rh.clone(), 600u64).into_val(&t.env));
    let e = t.v.pay(&t.seller, &UNIT, &rh, &600);
    let paid = Paid { escrow: e.clone(), seller: t.seller.clone(), amount: UNIT, request_hash: rh, deadline: T0 + 600 };
    assert_eq!(t.env.events().all().filter_by_contract(&t.vault), [ev(&t, &paid)]);

    let r = h(&t.env, 9);
    let sig = sign(&t, &e, &r);
    t.v.settle(&e, &r, &sig);
    let settled = Settled { escrow: e, result_hash: r, via_dispute: false };
    assert_eq!(t.env.events().all().filter_by_contract(&t.vault), [ev(&t, &settled)]);

    let e2 = pay(&t, UNIT);
    advance(&t, 601);
    t.v.refund(&e2);
    assert_eq!(t.env.events().all().filter_by_contract(&t.vault), [ev(&t, &Refunded { escrow: e2 })]);

    mock(&t, &t.owner, "revoke_agent", ().into_val(&t.env));
    t.v.revoke_agent();
    let cfg = ConfigChanged {
        agent: t.agent.clone(),
        agent_valid_until: T0 + 30 * 86_400,
        revoked: true,
        limit: LIMIT,
        period_secs: PERIOD,
        max_per_call: MAX_CALL,
    };
    assert_eq!(t.env.events().all().filter_by_contract(&t.vault), [ev(&t, &cfg)]);
}
