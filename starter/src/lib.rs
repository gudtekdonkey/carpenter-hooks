//! `mock_hook`: the TEST-ONLY approved hook `carpenter_amm` is exercised against (spec §8.1, §8.4, §9).
//!
//! WP-0 wrote it as a pass-through; WP-A2 (2026-09-14) added the O-1…O-9 failure modes and the R-6
//! sentinel BEHIND A SWITCH, so what an unconfigured mock returns is unchanged.
//!
//! - `initialize_hook_config(flags, max_rounds, callback_accounts)` writes the 8-byte interface
//!   header `carpenter_amm::approve_hook` reads, at `["hook_config"]` under this program.
//! - The eight non-swap callbacks are pass-through: `pool_signer` must SIGN; the record is the echo
//!   of the `version / phase / round` prefix and nothing else.
//! - `before_swap`, `after_swap` and `after_actions` decode their FULL Args (§5.3 twins below).
//!   With no [`MockState`] as the first slice account they are pass-through too. With one, they
//!   follow its script: bad version / round echoes, a fee override, deltas (fixed, "equal to the
//!   specified amount", or "walk output + 1"), `want_callback`, up to N templated actions, signals,
//!   and a return mode (normal, none, from another program, a trailing byte). Every call also
//!   records what it was sent (claims, round, result count) and `before_swap` writes a SENTINEL
//!   into the state account (R-6: the AMM's post-state must preserve bytes the hook wrote).
//! - D-14, mock form: when `MockState.pool` is set, `pool_signer.key` must equal it (`WrongPool`), so
//!   a keypair `assign`ed to the AMM and signing is refused.
//! - A [`MockScript`] as the SECOND slice account (2026-09-17, the bid re-place drift): a per-round
//!   list of up to three DISTINCT actions plus `want_callback`, for `after_swap` (round 1) and each
//!   `after_actions` round. It overrides `MockState`'s one templated action for the rounds it
//!   scripts, so one record can carry `[ModifyPosition −L, Swap]` and a later round
//!   `[ModifyPosition +L over another range]` — the shape FLOOR's re-place takes (§6.4).
//! - `set_reported_fees(fees, fees_after)` (fix-fee-split, 2026-09-17; re-ported onto the A2 mock
//!   the same day; ⛔ split PER VIEW at the branch review, the same day — see below) stores TWO
//!   `[skim, creator, holder, platform]` splits, `HookConfig.reported_fees` for `before_swap` and
//!   `HookConfig.reported_fees_after` for `after_swap`. When this
//!   program's own `hook_config` PDA is ANYWHERE in the callback's remaining accounts (the hook
//!   slice), `before_swap` returns ITS split as `fees` with
//!   `delta_specified = skim + creator + holder`, and `after_swap` returns ITS OWN with
//!   `delta_unspecified` likewise — the exact shape §5.4 O-4 accepts, so `carpenter_amm`'s
//!   `Swapped.fee_split0 / fee_split1` can be tested without FLOOR's hook. Without the account, or
//!   with an all-zero split, that leg is unchanged. ⛔ **The two arrays are why the AMM's call site
//!   is testable at all**: with ONE split reported on both legs, `swap_fees(.., rec0, rec1)` and
//!   `swap_fees(.., rec1, rec0)` return byte-identical pairs, so a transposition at
//!   `carpenter_amm/src/instructions/swap.rs` — which would report `[0;4]` for FLOOR's populated leg,
//!   the exact defect this branch fixes — passed every assertion the suite could make. ⛔ An
//!   all-zero array on ONE leg is a leg that reports nothing while the other reports: that is
//!   FLOOR's own shape (§5.5: "an indexer reads `fee_split0` and finds `fee_split1 == [0; 4]`"),
//!   and it is only expressible because the arrays are separate. ⛔ The slice is scanned by KEY, never
//!   declared as a typed `Option<AccountLoader>`: Anchor parses any present account that is not the
//!   program id as `Some` and then fails its owner / seeds checks, which would break every caller
//!   that approves the mock with `callback_accounts = 1` and passes an arbitrary account
//!   (`amm_pool.rs` does exactly that). ⛔ It OVERRIDES a scripted `MockState` delta on its own leg.
//! - `forward(data)`: a HOOK-INITIATED call. Invokes `target_program` (the second account) with
//!   `data`, the remaining accounts as the instruction's accounts, and `["hook_authority"]` (the
//!   first account) signing (D-13): how `amm_hooks.rs` proves the noSelfCall skip and settlement
//!   against claims, and how `amm_pool.rs` reaches `hook_withdraw` and `register_mint_exclusivity`.
//!
//! ⛔ Never deployed: no devnet entry in `Anchor.toml`, excluded from every deploy script.
//! ⛔ [`MockState`] is written by the TESTS (LiteSVM `set_account`), never by an instruction: the
//! instruction set stays the one `tests/tests/layouts.rs` pins, `forward` included.
//! ⚠ The three swap callbacks return `Result<()>` and call `set_return_data` themselves, because
//! Anchor sets return data for any non-unit return type and two of the return modes must NOT.
//! `HookRecordV1` stays in the IDL through the eight pass-through callbacks.
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::{invoke, invoke_signed, set_return_data};
use carpenter_types::hook::{ArgsHeader, HookAction as TypesAction, HookRecordV1 as TypesRecord};

declare_id!("8NR5f68SspkCNgyGBvjuWcqMbPw1kDzhbpKgqHkCeWJk");

/// What `before_swap` writes into [`MockState::sentinel`] (R-6).
pub const SENTINEL: u64 = 0x5EA1_ED0F_F100_2A6E;

/// `MockState.return_mode` values (applied in the phases of `return_mode_mask`).
pub const RETURN_NORMAL: u8 = 0;
/// No `set_return_data` at all → `HookBadReturn`.
pub const RETURN_NONE: u8 = 1;
/// Return data from ANOTHER program: SPL Token `GetAccountDataSize` on slice[1] (a mint), slice[2]
/// being that token program; the mock sets nothing after it → `HookBadReturn` (O-1).
pub const RETURN_FROM_TOKEN_PROGRAM: u8 = 2;
/// The record plus one trailing byte → strict decode refuses it → `HookBadReturn`.
pub const RETURN_TRAILING_BYTE: u8 = 3;

/// `MockState.before_delta_mode`: `delta_specified = before_delta_specified`.
pub const DELTA_FIXED: u8 = 0;
/// `before_swap`: `delta_specified = −params.amount_specified` (the walk is skipped, §5.5).
pub const DELTA_EQUALS_SPECIFIED: u8 = 1;
/// `after_swap`: `delta_unspecified = output of swap_delta + 1` (→ `HookDeltaExceedsSwapOutput`).
pub const DELTA_OUTPUT_PLUS_ONE: u8 = 1;

/// `MockState.action_kind`.
pub const ACTION_MODIFY_POSITION: u8 = 0;
pub const ACTION_SWAP: u8 = 1;

#[program]
pub mod mock_hook {
    use super::*;

    /// Writes the interface header (`carpenter_types::layout::HookInterfaceHeader`).
    pub fn initialize_hook_config(
        ctx: Context<InitializeHookConfig>,
        flags: u16,
        max_rounds: u8,
        callback_accounts: u8,
    ) -> Result<()> {
        let mut hc = ctx.accounts.hook_config.load_init()?;
        hc.version = 1;
        hc.bump = ctx.bumps.hook_config;
        hc.max_rounds = max_rounds;
        hc.callback_accounts = callback_accounts;
        hc.flags = flags;
        hc.authority_bump = Pubkey::find_program_address(&[carpenter_types::seeds::HOOK_AUTHORITY], &crate::ID).1;
        hc.authority = ctx.accounts.authority.key();
        Ok(())
    }

    pub fn before_initialize(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn after_initialize(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn before_add_liquidity(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn after_add_liquidity(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn before_remove_liquidity(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn after_remove_liquidity(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn before_swap<'info>(ctx: Context<'info, Callback<'info>>, args: BeforeSwapArgs) -> Result<()> {
        let hdr = ArgsHeader { version: args.version, phase: args.phase, round: args.round };
        scripted(&ctx, hdr, View::Before(&args))
    }

    pub fn after_swap<'info>(ctx: Context<'info, Callback<'info>>, args: AfterSwapArgs) -> Result<()> {
        let hdr = ArgsHeader { version: args.version, phase: args.phase, round: args.round };
        scripted(&ctx, hdr, View::After(&args))
    }

    pub fn before_donate(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn after_donate(ctx: Context<Callback>, version: u8, phase: u8, round: u8) -> Result<HookRecordV1> {
        pass_through(&ctx, ArgsHeader { version, phase, round })
    }

    pub fn after_actions<'info>(ctx: Context<'info, Callback<'info>>, args: AfterActionsArgs) -> Result<()> {
        let hdr = ArgsHeader { version: args.version, phase: args.phase, round: args.round };
        scripted(&ctx, hdr, View::Actions(&args))
    }

    /// A hook-initiated call (D-13): `target_program` is invoked with `data` and the remaining
    /// accounts, every occurrence of `["hook_authority"]` marked signer and signed for. The shape is
    /// the one `tests/tests/amm_pool.rs::forward_ix` froze (SPEC-DEFECT A1-2): accounts
    /// `[hook_authority (ro, never a top-level signer), target_program]`, then the target's own
    /// accounts in order. The mock sets no return data after the CPI, so the transaction's return
    /// data is the callee's (`HookOpResultV1` for the AMM).
    /// ⚠ Renamed from `hook_invoke` at wave 3 (2026-09-17): ONE instruction serves A1's
    /// hook-authority cases and A2's suites.
    pub fn forward<'info>(ctx: Context<'info, Forward<'info>>, data: Vec<u8>) -> Result<()> {
        let (authority, bump) = Pubkey::find_program_address(&[carpenter_types::seeds::HOOK_AUTHORITY], &crate::ID);
        require_keys_eq!(ctx.accounts.hook_authority.key(), authority, MockHookError::WrongAuthority);
        let metas: Vec<AccountMeta> = ctx
            .remaining_accounts
            .iter()
            .map(|a| AccountMeta { pubkey: *a.key, is_signer: a.is_signer || *a.key == authority, is_writable: a.is_writable })
            .collect();
        let ix = Instruction { program_id: ctx.accounts.target_program.key(), accounts: metas, data };
        let mut infos: Vec<AccountInfo<'info>> = ctx.remaining_accounts.to_vec();
        infos.push(ctx.accounts.hook_authority.to_account_info());
        infos.push(ctx.accounts.target_program.to_account_info());
        invoke_signed(&ix, &infos, &[&[carpenter_types::seeds::HOOK_AUTHORITY, &[bump]]])?;
        Ok(())
    }

    /// The fee-split switch (fix-fee-split, 2026-09-17): the config's authority stores
    /// `[skim, creator, holder, platform]` for `before_swap` and a SECOND one for `after_swap`;
    /// `[0; 4]` restores the pass-through on that leg, and `[0; 4]` twice switches the whole thing
    /// off. Refuses `platform > skim` and an overflowing charge IN EITHER ARRAY, because such a
    /// record can never pass O-4 and the mock is not the O-4 failure mode.
    /// ⛔ Two arrays, not one: see the module header — one split reported on both legs makes a
    /// transposed `swap_fees(.., rec1, rec0)` in the AMM undetectable.
    pub fn set_reported_fees(ctx: Context<SetReportedFees>, fees: [u64; 4], fees_after: [u64; 4]) -> Result<()> {
        for f in [&fees, &fees_after] {
            require!(f[carpenter_types::hook::FEE_PLATFORM] <= f[carpenter_types::hook::FEE_SKIM], MockHookError::FeesInconsistent);
            charged(f)?;
        }
        let mut hc = ctx.accounts.hook_config.load_mut()?;
        hc.reported_fees = fees;
        hc.reported_fees_after = fees_after;
        Ok(())
    }
}

/// `skim + creator + holder`, the delta a reported split is booked under.
fn charged(fees: &[u64; 4]) -> Result<u64> {
    use carpenter_types::hook::{FEE_CREATOR, FEE_HOLDER, FEE_SKIM};
    fees[FEE_SKIM]
        .checked_add(fees[FEE_CREATOR])
        .and_then(|s| s.checked_add(fees[FEE_HOLDER]))
        .ok_or_else(|| error!(MockHookError::FeesInconsistent))
}

/// The stored splits — `(before_swap's, after_swap's)` — when this program's `hook_config` PDA is
/// among the remaining accounts (the hook slice) and at least one of them is non-zero. Any other
/// account in the slice is ignored.
///
/// The candidate is found by owner + discriminator (`AccountLoader::try_from`) and then pinned to
/// the PDA with `create_program_address` over the STORED bump — one syscall, not a
/// `find_program_address` search — so a look-alike account this program owns is refused, and a
/// foreign account is skipped rather than failed.
/// ⛔ Scanned by KEY, never declared as a typed `Option<AccountLoader>`: Anchor parses any present
/// account that is not the program id as `Some` and then fails its owner / seeds checks, which
/// would break every caller that approves the mock with `callback_accounts = 1` and passes an
/// arbitrary account (`amm_pool.rs` does exactly that).
/// ⚠ The `MockState` script (A2) is read out of the SAME slice, by its own discriminator, so the
/// two switches compose: a state account first, a `hook_config` anywhere after it.
/// ⚠ The lifetime is NAMED and used on both the slice and the `AccountInfo`: `AccountInfo<'a>` is
/// INVARIANT over `'a`, so an elided `&[AccountInfo]` gives two unrelated lifetimes and the loop
/// fails to borrow-check ("`'1` must outlive `'2`").
fn reported_fees<'info>(remaining: &'info [AccountInfo<'info>]) -> Result<Option<([u64; 4], [u64; 4])>> {
    for ai in remaining {
        if ai.owner != &crate::ID {
            continue;
        }
        let Ok(loader) = AccountLoader::<HookConfig>::try_from(ai) else { continue };
        let hc = loader.load()?;
        let want = Pubkey::create_program_address(&[carpenter_types::seeds::HOOK_CONFIG, &[hc.bump]], &crate::ID)
            .map_err(|_| error!(MockHookError::ConfigMismatch))?;
        require_keys_eq!(*ai.key, want, MockHookError::ConfigMismatch);
        let (before, after) = (hc.reported_fees, hc.reported_fees_after);
        return Ok(if before == [0; 4] && after == [0; 4] { None } else { Some((before, after)) });
    }
    Ok(None)
}

/// Echo the Args prefix and say nothing else.
///
/// ⚠ These callbacks declare `version, phase, round` as their arguments: every §5.3 Args begins
/// with exactly those three `u8`s, and Anchor 1.2 decodes arguments with a non-strict
/// `deserialize` that ignores the trailing bytes (`anchor-syn 1.2.0
/// codegen/program/handlers.rs:117`), so one signature accepts the full Args of any phase.
fn pass_through(ctx: &Context<Callback>, hdr: ArgsHeader) -> Result<HookRecordV1> {
    require!(ctx.accounts.pool_signer.is_signer, MockHookError::PoolNotSigner);
    Ok(TypesRecord::pass_through(hdr.version, hdr.phase, hdr.round).into())
}

enum View<'a> {
    Before(&'a BeforeSwapArgs),
    After(&'a AfterSwapArgs),
    Actions(&'a AfterActionsArgs),
}

/// The slice's first account, when it is a `MockState` of this program.
fn mock_state<'info>(remaining: &'info [AccountInfo<'info>]) -> Option<&'info AccountInfo<'info>> {
    let a = remaining.first()?;
    if *a.owner != crate::ID {
        return None;
    }
    let data = a.try_borrow_data().ok()?;
    (data.len() == MockState::LEN && data[..8] == *MockState::DISCRIMINATOR).then_some(a)
}

/// The slice's second account, when it is a `MockScript` of this program.
fn mock_script<'info>(remaining: &'info [AccountInfo<'info>]) -> Option<&'info AccountInfo<'info>> {
    let a = remaining.get(1)?;
    if *a.owner != crate::ID {
        return None;
    }
    let data = a.try_borrow_data().ok()?;
    (data.len() == MockScript::LEN && data[..8] == *MockScript::DISCRIMINATOR).then_some(a)
}

fn i128_of(limbs: [u64; 2]) -> i128 {
    ((limbs[0] as u128) | ((limbs[1] as u128) << 64)) as i128
}

fn u128_of(limbs: [u64; 2]) -> u128 {
    (limbs[0] as u128) | ((limbs[1] as u128) << 64)
}

fn scripted<'info>(ctx: &Context<'info, Callback<'info>>, hdr: ArgsHeader, view: View<'_>) -> Result<()> {
    require!(ctx.accounts.pool_signer.is_signer, MockHookError::PoolNotSigner);
    let mut rec = TypesRecord::pass_through(hdr.version, hdr.phase, hdr.round);
    let mut return_mode = RETURN_NORMAL;
    // `View` holds references; `matches!` does not move it.
    let (is_before, is_after) = (matches!(view, View::Before(_)), matches!(view, View::After(_)));

    if let Some(info) = mock_state(ctx.remaining_accounts) {
        let mut s: MockState = {
            let data = info.try_borrow_data()?;
            *bytemuck::from_bytes::<MockState>(&data[8..MockState::LEN])
        };
        if s.pool != Pubkey::default() {
            require_keys_eq!(ctx.accounts.pool_signer.key(), s.pool, MockHookError::WrongPool);
        }
        let bit = 1u16 << hdr.phase;

        match view {
            View::Before(a) => {
                s.sentinel = SENTINEL;
                s.last_claims0 = a.hook_claims0;
                s.last_claims1 = a.hook_claims1;
                rec.delta_specified = match s.before_delta_mode {
                    DELTA_EQUALS_SPECIFIED => a.params.amount_specified.checked_neg().ok_or(MockHookError::Overflow)?,
                    _ => i128_of(s.before_delta_specified),
                };
                rec.delta_unspecified = i128_of(s.before_delta_unspecified);
            }
            View::After(a) => {
                s.last_claims0 = a.hook_claims0;
                s.last_claims1 = a.hook_claims1;
                rec.delta_unspecified = match s.after_delta_mode {
                    DELTA_OUTPUT_PLUS_ONE => {
                        let out = if a.params.zero_for_one { a.swap_delta.amount1 } else { a.swap_delta.amount0 };
                        out.checked_add(1).ok_or(MockHookError::Overflow)?
                    }
                    _ => i128_of(s.after_delta_unspecified),
                };
                rec.signals = s.signals;
            }
            View::Actions(a) => {
                s.last_claims0 = a.hook_claims0;
                s.last_claims1 = a.hook_claims1;
                s.last_results = a.results.len() as u32;
            }
        }
        if s.fee_override_mask & bit != 0 {
            rec.fee_override_pips = Some(s.fee_override_pips);
        }
        if s.want_callback_mask & bit != 0 {
            rec.want_callback = true;
        }
        if s.actions_mask & bit != 0 {
            let action = if s.action_kind == ACTION_MODIFY_POSITION {
                TypesAction::ModifyPosition {
                    salt: s.action_salt,
                    tick_lower: s.action_tick_lower,
                    tick_upper: s.action_tick_upper,
                    liquidity_delta: i128_of(s.action_liquidity_delta),
                }
            } else {
                TypesAction::Swap {
                    zero_for_one: s.action_zero_for_one != 0,
                    amount_specified: i128_of(s.action_amount_specified),
                    sqrt_price_limit_x64: u128_of(s.action_sqrt_price_limit),
                }
            };
            rec.actions = vec![action; s.action_count as usize];
        }
        if s.bad_version_mask & bit != 0 {
            rec.version = rec.version.wrapping_add(1);
        }
        if s.bad_round_mask & bit != 0 {
            rec.round = rec.round.wrapping_add(1);
        }
        if s.return_mode_mask & bit != 0 {
            return_mode = s.return_mode;
        }
        s.calls += 1;
        s.last_phase = hdr.phase;
        s.last_round = hdr.round;
        if info.is_writable {
            let mut data = info.try_borrow_mut_data()?;
            *bytemuck::from_bytes_mut::<MockState>(&mut data[8..MockState::LEN]) = s;
        }
    }

    // The per-round script (bid-re-place drift, 2026-09-17): rounds[round − 1] for round ≥ 1.
    if let Some(info) = mock_script(ctx.remaining_accounts) {
        if hdr.round >= 1 && (hdr.round as usize) <= SCRIPT_ROUNDS {
            let data = info.try_borrow_data()?;
            let script = bytemuck::from_bytes::<MockScript>(&data[8..MockScript::LEN]);
            let r = &script.rounds[hdr.round as usize - 1];
            if r.want_callback != 0 {
                rec.want_callback = true;
            }
            if r.action_count > 0 {
                let n = (r.action_count as usize).min(SCRIPT_ACTIONS);
                rec.actions = r.actions[..n]
                    .iter()
                    .map(|a| {
                        if a.kind == ACTION_MODIFY_POSITION {
                            TypesAction::ModifyPosition {
                                salt: a.salt,
                                tick_lower: a.tick_lower,
                                tick_upper: a.tick_upper,
                                liquidity_delta: i128_of(a.liquidity_delta),
                            }
                        } else {
                            TypesAction::Swap {
                                zero_for_one: a.zero_for_one != 0,
                                amount_specified: i128_of(a.amount_specified),
                                sqrt_price_limit_x64: u128_of(a.sqrt_price_limit),
                            }
                        }
                    })
                    .collect();
            }
        }
    }

    // The fee-split switch: this program's own `hook_config` anywhere in the slice, carrying a
    // non-zero `[skim, creator, holder, platform]` FOR THIS LEG. `before_swap` books its own
    // array's charge as `delta_specified`, `after_swap` books the OTHER array's as
    // `delta_unspecified` — the exact shape §5.4 O-4 accepts, so `carpenter_amm`'s
    // `Swapped.fee_split0 / fee_split1` can be tested without FLOOR's hook.
    // ⛔ PER LEG, never one array on both (branch review, 2026-09-17): with the same split on both
    // records the AMM's `swap_fees(.., rec0, rec1)` and `swap_fees(.., rec1, rec0)` are
    // byte-identical, so no assertion downstream could tell a transposed call site from a correct
    // one. A leg whose array is `[0; 4]` reports nothing and keeps whatever the script gave it —
    // which is how FLOOR's own one-leg shape (§5.5) is expressed here.
    // ⛔ It OVERRIDES a scripted delta on its own leg. The two switches live in different accounts
    // and are set by different suites; a test that wants both must expect this one to win.
    // ⚠ `after_actions` never carries fees (O-4 refuses them from any other phase).
    if let Some((before, after)) = reported_fees(ctx.remaining_accounts)? {
        let leg = if is_before {
            Some(before)
        } else if is_after {
            Some(after)
        } else {
            None
        };
        if let Some(fees) = leg.filter(|f| *f != [0; 4]) {
            let charge = i128::from(charged(&fees)?);
            if is_before {
                rec.delta_specified = charge;
            } else {
                rec.delta_unspecified = charge;
            }
            rec.fees = fees;
        }
    }

    let mut bytes = borsh::to_vec(&rec).map_err(|_| MockHookError::Overflow)?;
    match return_mode {
        RETURN_NONE => {}
        RETURN_FROM_TOKEN_PROGRAM => {
            let mint = ctx.remaining_accounts.get(1).ok_or(ErrorCode::AccountNotEnoughKeys)?;
            let token_program = ctx.remaining_accounts.get(2).ok_or(ErrorCode::AccountNotEnoughKeys)?;
            // SPL Token `GetAccountDataSize` (tag 21) sets return data from the TOKEN program.
            let ix = Instruction {
                program_id: *token_program.key,
                accounts: vec![AccountMeta::new_readonly(*mint.key, false)],
                data: vec![21],
            };
            invoke(&ix, &[mint.clone(), token_program.clone()])?;
        }
        RETURN_TRAILING_BYTE => {
            bytes.push(0);
            set_return_data(&bytes);
        }
        _ => set_return_data(&bytes),
    }
    Ok(())
}

#[derive(Accounts)]
pub struct SetReportedFees<'info> {
    pub authority: Signer<'info>,
    #[account(mut, seeds = [carpenter_types::seeds::HOOK_CONFIG], bump = hook_config.load()?.bump,
              constraint = hook_config.load()?.authority == authority.key() @ MockHookError::NotAuthority)]
    pub hook_config: AccountLoader<'info, HookConfig>,
}

#[derive(Accounts)]
pub struct InitializeHookConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(init, payer = authority, space = 8 + 128, seeds = [carpenter_types::seeds::HOOK_CONFIG], bump)]
    pub hook_config: AccountLoader<'info, HookConfig>,
    pub system_program: Program<'info, System>,
}

/// Every callback: `pool_signer` first (§5.2), then the hook slice as remaining accounts.
#[derive(Accounts)]
pub struct Callback<'info> {
    /// CHECK: must sign; compared to `MockState.pool` when a script sets it (D-14, mock form).
    pub pool_signer: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Forward<'info> {
    /// CHECK: must be this program's `["hook_authority"]` PDA (checked in the handler); it never
    /// signs at the top level — the mock signs for it inside the CPI.
    pub hook_authority: UncheckedAccount<'info>,
    /// CHECK: the program being called (the AMM); the remaining accounts are its instruction's.
    pub target_program: UncheckedAccount<'info>,
}

/// `["hook_config"]`: the 8-byte interface header, then mock-only fields.
#[account(zero_copy)]
pub struct HookConfig {
    pub version: u8,
    pub bump: u8,
    pub max_rounds: u8,
    pub callback_accounts: u8,
    pub flags: u16,
    pub authority_bump: u8,
    pub pad0: u8,
    pub authority: Pubkey,
    /// The fee-split switch (fix-fee-split, 2026-09-17, re-ported onto the A2 mock 2026-09-17):
    /// `before_swap`'s `[skim, creator, holder, platform]`; `[0; 4]` = that leg reports nothing.
    /// Carved out of `reserved`, so the account is still 128 B and `initialize_hook_config`'s
    /// `space` is unchanged.
    pub reported_fees: [u64; 4],
    /// `after_swap`'s split (branch review, 2026-09-17). A SECOND array, out of the same reserved
    /// region, because one split reported on both legs makes a transposed `swap_fees` call site in
    /// the AMM undetectable — see the module header.
    pub reported_fees_after: [u64; 4],
    pub reserved: [u8; 24],
}
const _: () = assert!(core::mem::size_of::<HookConfig>() == 128);
const _: () = assert!(core::mem::offset_of!(HookConfig, authority) == 8);
const _: () = assert!(core::mem::offset_of!(HookConfig, reported_fees) == 40);
const _: () = assert!(core::mem::offset_of!(HookConfig, reported_fees_after) == 72);

/// The test script (256 B struct, 264 B account). Written by tests with LiteSVM `set_account`
/// (owner = this program, the Anchor discriminator, then these bytes); the mock updates the
/// recording fields when the account is passed writable. Phase masks are `1 << PHASE_*`.
/// i128 / u128 values are `[low, high]` limbs.
#[account(zero_copy)]
#[derive(Debug)]
pub struct MockState {
    /// D-14 (mock form): when set, `pool_signer.key` must equal it.
    pub pool: Pubkey,
    /// R-6: `before_swap` writes [`SENTINEL`].
    pub sentinel: u64,
    /// Swap-phase callbacks served with this state.
    pub calls: u64,
    pub bad_version_mask: u16,
    pub bad_round_mask: u16,
    pub fee_override_mask: u16,
    pub want_callback_mask: u16,
    pub actions_mask: u16,
    pub return_mode_mask: u16,
    pub return_mode: u8,
    pub before_delta_mode: u8,
    pub after_delta_mode: u8,
    pub action_count: u8,
    pub fee_override_pips: u32,
    pub action_kind: u8,
    pub action_zero_for_one: u8,
    /// `after_swap` returns these signals.
    pub signals: u8,
    /// Recorded: the phase of the last call.
    pub last_phase: u8,
    pub before_delta_specified: [u64; 2],
    pub before_delta_unspecified: [u64; 2],
    pub after_delta_unspecified: [u64; 2],
    pub action_salt: u64,
    pub action_tick_lower: i32,
    pub action_tick_upper: i32,
    pub action_liquidity_delta: [u64; 2],
    pub action_amount_specified: [u64; 2],
    pub action_sqrt_price_limit: [u64; 2],
    /// Recorded: the round of the last call.
    pub last_round: u8,
    pub pad0: [u8; 3],
    /// Recorded: `results.len()` of the last `after_actions`.
    pub last_results: u32,
    /// Recorded: the claims the last call was sent.
    pub last_claims0: u64,
    pub last_claims1: u64,
    pub reserved: [u8; 48],
}
impl MockState {
    pub const LEN: usize = 8 + 256;
}
const _: () = assert!(core::mem::size_of::<MockState>() == 256);
const _: () = assert!(core::mem::offset_of!(MockState, before_delta_specified) == 72);
const _: () = assert!(core::mem::offset_of!(MockState, last_claims0) == 192);

/// Rounds a [`MockScript`] can script: `after_swap` (1) and `after_actions` 2, 3, 4
/// (`MAX_ROUNDS_CAP` is 4, so round 4 is never reached; the slot keeps the array square).
pub const SCRIPT_ROUNDS: usize = 4;
/// Distinct actions per scripted round.
pub const SCRIPT_ACTIONS: usize = 3;

/// One scripted action (72 B). `kind` is `ACTION_MODIFY_POSITION` / `ACTION_SWAP`; the i128 /
/// u128 fields are `[low, high]` limbs.
#[zero_copy]
#[derive(Debug)]
pub struct ScriptAction {
    pub kind: u8,
    pub zero_for_one: u8,
    pub pad0: [u8; 6],
    pub salt: u64,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity_delta: [u64; 2],
    pub amount_specified: [u64; 2],
    pub sqrt_price_limit: [u64; 2],
}
const _: () = assert!(core::mem::size_of::<ScriptAction>() == 72);

/// One round's script (224 B): `action_count` (0 = leave `MockState`'s template in force) of
/// `actions`, and whether to ask for another round.
#[zero_copy]
#[derive(Debug)]
pub struct ScriptRound {
    pub want_callback: u8,
    pub action_count: u8,
    pub pad0: [u8; 6],
    pub actions: [ScriptAction; SCRIPT_ACTIONS],
}
const _: () = assert!(core::mem::size_of::<ScriptRound>() == 224);

/// The per-round action script (896 B struct, 904 B account), the SECOND slice account when
/// present. `rounds[r − 1]` serves round `r` (`after_swap` is round 1). Written by tests with
/// `set_account`, exactly as [`MockState`] is; never written by an instruction.
#[account(zero_copy)]
#[derive(Debug)]
pub struct MockScript {
    pub rounds: [ScriptRound; SCRIPT_ROUNDS],
}
impl MockScript {
    pub const LEN: usize = 8 + 896;
}
const _: () = assert!(core::mem::size_of::<MockScript>() == 896);

// ---- §5.3 Args twins (IDL-visible; the same Borsh bytes as carpenter_types::hook) --------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolSnapshot {
    pub sqrt_price_x64: u128,
    pub tick: i32,
    pub liquidity: u128,
    pub lp_fee_pips: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BalanceDelta {
    pub amount0: i128,
    pub amount1: i128,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwapParams {
    pub zero_for_one: bool,
    pub amount_specified: i128,
    pub sqrt_price_limit_x64: u128,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct BeforeSwapArgs {
    pub version: u8,
    pub phase: u8,
    pub round: u8,
    pub pool: [u8; 32],
    pub sender: [u8; 32],
    pub params: SwapParams,
    pub pre: PoolSnapshot,
    pub supply0: u64,
    pub supply1: u64,
    pub hook_claims0: u64,
    pub hook_claims1: u64,
    pub hook_data: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AfterSwapArgs {
    pub version: u8,
    pub phase: u8,
    pub round: u8,
    pub pool: [u8; 32],
    pub sender: [u8; 32],
    pub params: SwapParams,
    pub pre: PoolSnapshot,
    pub post: PoolSnapshot,
    pub swap_delta: BalanceDelta,
    pub before_delta_specified: i128,
    pub before_delta_unspecified: i128,
    pub supply0: u64,
    pub supply1: u64,
    pub hook_claims0: u64,
    pub hook_claims1: u64,
    pub hook_data: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionResult {
    pub index: u8,
    pub kind: u8,
    pub delta: BalanceDelta,
    pub fees0: u64,
    pub fees1: u64,
    pub post_sqrt_price_x64: u128,
    pub post_tick: i32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AfterActionsArgs {
    pub version: u8,
    pub phase: u8,
    pub round: u8,
    pub pool: [u8; 32],
    pub post: PoolSnapshot,
    pub supply0: u64,
    pub supply1: u64,
    pub hook_claims0: u64,
    pub hook_claims1: u64,
    pub results: Vec<ActionResult>,
}

/// The §5.3 record, IDL-visible. Same Borsh bytes as `carpenter_types::hook::HookRecordV1`
/// (`tests/tests/layouts.rs` pins the field list).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct HookRecordV1 {
    pub version: u8,
    pub phase: u8,
    pub round: u8,
    pub want_callback: bool,
    pub fee_override_pips: Option<u32>,
    pub delta_specified: i128,
    pub delta_unspecified: i128,
    pub signals: u8,
    // ⛔ 2026-09-17: the twin must carry `fees` in the SAME position as
    // `carpenter_types::hook::HookRecordV1`, or the eight pass-through callbacks (which serialize THIS
    // struct, unlike the three scripted ones, which serialize the types record directly) emit 42
    // bytes where the AMM's strict decode wants 74, and every `initialize_pool` / liquidity /
    // donate callback fails `HookBadReturn`. MEASURED 2026-09-17: dropping it fails 12 `amm_pool.rs`
    // cases while all 27 of `amm_hooks.rs` stay green.
    // ⛔ `//`, never `///`: Anchor puts doc comments in the IDL and
    // `layouts.rs::mock_hook_record_type_matches_floor_hook_record_type` compares the two type
    // objects WHOLE — a doc on one side and not the other fails it. (Measured the same day.)
    pub fees: [u64; 4],
    pub actions: Vec<HookAction>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAction {
    ModifyPosition { salt: u64, tick_lower: i32, tick_upper: i32, liquidity_delta: i128 },
    Swap { zero_for_one: bool, amount_specified: i128, sqrt_price_limit_x64: u128 },
}

impl From<TypesRecord> for HookRecordV1 {
    fn from(r: TypesRecord) -> Self {
        Self {
            version: r.version,
            phase: r.phase,
            round: r.round,
            want_callback: r.want_callback,
            fee_override_pips: r.fee_override_pips,
            delta_specified: r.delta_specified,
            delta_unspecified: r.delta_unspecified,
            signals: r.signals,
            fees: r.fees,
            actions: r
                .actions
                .into_iter()
                .map(|a| match a {
                    TypesAction::ModifyPosition { salt, tick_lower, tick_upper, liquidity_delta } => {
                        HookAction::ModifyPosition { salt, tick_lower, tick_upper, liquidity_delta }
                    }
                    TypesAction::Swap { zero_for_one, amount_specified, sqrt_price_limit_x64 } => {
                        HookAction::Swap { zero_for_one, amount_specified, sqrt_price_limit_x64 }
                    }
                })
                .collect(),
        }
    }
}

/// ⛔ APPEND ONLY (Anchor numbers from 6000 in declaration order).
#[error_code]
pub enum MockHookError {
    #[msg("pool_signer did not sign")]
    PoolNotSigner,
    #[msg("pool_signer is not the pool this mock state is bound to (D-14, mock form)")]
    WrongPool,
    #[msg("arithmetic overflow in a scripted delta")]
    Overflow,
    #[msg("forward: account 0 is not this program's [\"hook_authority\"] PDA")]
    WrongAuthority,
    #[msg("signer is not the hook_config authority")]
    NotAuthority,
    #[msg("reported fees: platform cut above the skim, or the charge overflows u64")]
    FeesInconsistent,
    #[msg("a HookConfig in the hook slice is not this program's [\"hook_config\"] PDA")]
    ConfigMismatch,
}
