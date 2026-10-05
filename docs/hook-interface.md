# The Carpenter hook interface

Everything an approved hook must present, and everything `carpenter_amm` checks before it will write
your `ApprovedHook` record. Each claim below names the symbol it comes from — `approve_hook`,
`reapprove_hook`, `hook_flags_valid`, `HookInterfaceHeader`, `ApprovedHook`, `initialize_pool` — so
you can check it against the IDL and against the accounts on chain.

Interface version: **`HOOK_INTERFACE_VERSION = 1`**. A header carrying any other `version` is
refused `HookHeaderMismatch`.

---

## 1. `HookInterfaceHeader`

The **first 8 bytes of the struct** of the account your program owns at the seed `["hook_config"]`,
derived under **your** program id. The account is an Anchor account, so its 8-byte discriminator
comes first: the header is at **account bytes 8..16** (`HookInterfaceHeader::ACCOUNT_OFFSET == 8`).
`approve_hook` reads exactly those bytes and nothing else — everything after byte 16 is yours.

`#[repr(C)]`, `Pod`, 8 bytes, no padding beyond the one byte named below.

| Struct offset | Account offset | Field | Type | What it is |
|---|---|---|---|---|
| 0 | 8 | `version` | `u8` | The interface version. Must be `1`. |
| 1 | 9 | `bump` | `u8` | The bump of your own `["hook_config"]` PDA. Yours to use; Carpenter does not check it. |
| 2 | 10 | `max_rounds` | `u8` | How many callback rounds a swap may run. See §4. |
| 3 | 11 | `callback_accounts` | `u8` | How many accounts the pool forwards to every callback. See §5. |
| 4 | 12 | `flags` | `u16` (LE) | Which phases you want. See §2. |
| 6 | 14 | `authority_bump` | `u8` | The bump of your `["hook_authority"]` PDA. Yours; Carpenter derives its own (§6). |
| 7 | 15 | `pad0` | `u8` | Padding. Write zero. |

A `hook_config` account shorter than 16 bytes cannot be read and is refused `HookHeaderMismatch`
(`HookInterfaceHeader::from_account_data` returns `None`).

⛔ The account must be **owned by your hook program** and sit at the `["hook_config"]` PDA under it.
Both are constraints on the `approve_hook` accounts; a mismatch is `HookHeaderMismatch`.

⛔ The header is **the contract your pools are opened under**. `initialize_pool` copies `flags`,
`max_rounds`, `callback_accounts` and your `hook_authority` into every `Pool`. That is why
`reapprove_hook` refuses an upgrade whose header changed any of `version`, `flags`, `max_rounds` or
`callback_accounts` — it is a different interface, and re-pinning it would silently change the terms
live pools trade under.

## 2. The flags

`flags: u16`. Bit 15 is undefined and must be zero.

| Bit | Value | Flag | Phase it enables |
|---|---|---|---|
| 14 | `0x4000` | `REQUESTS_ACTIONS` | your record may ask the pool to run actions, and you are called back at `after_actions` |
| 13 | `0x2000` | `BEFORE_INITIALIZE` | `before_initialize` |
| 12 | `0x1000` | `AFTER_INITIALIZE` | `after_initialize` |
| 11 | `0x0800` | `BEFORE_ADD_LIQUIDITY` | `before_add_liquidity` |
| 10 | `0x0400` | `AFTER_ADD_LIQUIDITY` | `after_add_liquidity` |
| 9 | `0x0200` | `BEFORE_REMOVE_LIQUIDITY` | `before_remove_liquidity` |
| 8 | `0x0100` | `AFTER_REMOVE_LIQUIDITY` | `after_remove_liquidity` |
| 7 | `0x0080` | `BEFORE_SWAP` | `before_swap` |
| 6 | `0x0040` | `AFTER_SWAP` | `after_swap` |
| 5 | `0x0020` | `BEFORE_DONATE` | `before_donate` |
| 4 | `0x0010` | `AFTER_DONATE` | `after_donate` |
| 3 | `0x0008` | `BEFORE_SWAP_RETURNS_DELTA` | `before_swap`'s record may carry a delta |
| 2 | `0x0004` | `AFTER_SWAP_RETURNS_DELTA` | `after_swap`'s record may carry a delta |
| 1 | `0x0002` | `AFTER_ADD_LIQUIDITY_RETURNS_DELTA` | ⛔ refused — see below |
| 0 | `0x0001` | `AFTER_REMOVE_LIQUIDITY_RETURNS_DELTA` | ⛔ refused — see below |

`ALL_HOOK_FLAGS = 0x7FFF`. The v4-inherited subset is `V4_HOOK_FLAGS = 0x3FFF`; `REQUESTS_ACTIONS`
(bit 14) is Carpenter's own.

### The rules — `hook_flags_valid`

Every one of these is `HookFlagsInvalid` at `approve_hook`, and each is checked again by
`reapprove_hook`:

1. **No bit outside `ALL_HOOK_FLAGS`** — i.e. bit 15 must be zero.
2. `BEFORE_SWAP_RETURNS_DELTA` **requires** `BEFORE_SWAP`.
3. `AFTER_SWAP_RETURNS_DELTA` **requires** `AFTER_SWAP`.
4. `AFTER_ADD_LIQUIDITY_RETURNS_DELTA` **requires** `AFTER_ADD_LIQUIDITY`.
5. `AFTER_REMOVE_LIQUIDITY_RETURNS_DELTA` **requires** `AFTER_REMOVE_LIQUIDITY`.
6. `REQUESTS_ACTIONS` **requires** `AFTER_SWAP` — actions come back from `after_swap` /
   `after_actions` and nowhere else.
7. ⛔ **`AFTER_ADD_LIQUIDITY_RETURNS_DELTA` and `AFTER_REMOVE_LIQUIDITY_RETURNS_DELTA` are refused
   outright**, whatever else is set. Uniswap v4 lets `afterAddLiquidity` / `afterRemoveLiquidity`
   return a delta; Carpenter's record cannot express a delta outside the two swap phases, so a hook
   approved with either flag would silently never take the delta it was written to take. Rules 4 and
   5 therefore only ever matter as the reason a *pair* is refused: in practice bits 1 and 0 must be
   zero.

Worked example — FLOOR's hook on devnet, `flags = 0x60CC`:

```
0x60CC = 0110 0000 1100 1100
         bit 14 REQUESTS_ACTIONS
         bit 13 BEFORE_INITIALIZE
         bit  7 BEFORE_SWAP
         bit  6 AFTER_SWAP
         bit  3 BEFORE_SWAP_RETURNS_DELTA
         bit  2 AFTER_SWAP_RETURNS_DELTA
```

It passes: both delta flags have their phase, `REQUESTS_ACTIONS` has `AFTER_SWAP`, bit 15 is clear,
and neither liquidity-delta bit is set. It carries `BEFORE_INITIALIZE`, which is what lets it ask for
`allow_public_pools: false` (§7).

## 3. The phases

The `phase` byte your callbacks are handed, and the one your record echoes back:

| Phase | Callback |
|---|---|
| 0 | `before_initialize` |
| 1 | `after_initialize` |
| 2 | `before_swap` |
| 3 | `after_swap` |
| 4 | `after_actions` |
| 5 | `before_add_liquidity` |
| 6 | `after_add_liquidity` |
| 7 | `before_remove_liquidity` |
| 8 | `after_remove_liquidity` |
| 9 | `before_donate` |
| 10 | `after_donate` |

`after_actions` has no flag of its own: it is gated by `REQUESTS_ACTIONS`, and its rounds are driven
by your record's `want_callback`.

Every callback is a CPI into your program with the **pool PDA as a signer** (read-only) followed by
your slice (§5). ⛔ **Verify it is that PDA, on every callback**: `is_signer` alone is forgeable (any
keypair signs; any account can be `assign`ed to `carpenter_amm`). Require that `pool_signer` is
owned by `carpenter_amm`, is a `Pool` account whose `hook` field is your program id, and that its key
equals `create_program_address(["pool", mint0, mint1, fee u32 LE, tick_spacing u16 LE, hook, [bump]],
carpenter_amm)` from its own fields — only `carpenter_amm` can sign as that. The starter's
`verify_pool_signer` does exactly this. You reply by setting the call's **return data** to a Borsh-encoded `HookRecordV1`;
the decode is strict, and a record the AMM cannot decode — including one with a trailing byte, or no
return data at all — is `HookBadReturn`. The record's shape is in
[`starter/src/lib.rs`](../starter/src/lib.rs) (`HookRecordV1`, `HookAction`), which is the twin the
IDL carries.

## 4. `max_rounds`

`max_rounds` bounds the callback rounds a single swap may run: `after_swap` is round 1, and each
`after_actions` your record asks for with `want_callback` is the next.

| | |
|---|---|
| Upper bound | `max_rounds <= MAX_ROUNDS_CAP`, and **`MAX_ROUNDS_CAP = 4`**. Above it: `RoundsTooHigh`. |
| Lower bound | If `AFTER_SWAP` is set, **`max_rounds >= 2`**. Below it: `HookFlagsInvalid`. |
| Exceeded at runtime | `HookRoundsExceeded`. |

The value is not the AMM's to choose: the header's `max_rounds` must equal the argument
`approve_hook` is called with, or `HookHeaderMismatch`.

## 5. `callback_accounts`

The hook **slice**: the accounts a caller appends to an AMM instruction's remaining accounts, which
the pool forwards to every callback, in order, with the writability the client marked — **never as
signers**.

- The length is exact. `dispatch` refuses any other length with `HookSliceLengthMismatch`. A
  mis-sliced list would otherwise forward the AMM's own tick arrays or positions into your program.
- A slice key equal to any account the AMM owns in that instruction (vaults, bitmap, tick arrays,
  positions), to the acting signer, or to the pool itself is refused
  (`HookSliceAliasesAmmAccount`).
- Like `max_rounds`, the header's `callback_accounts` must equal the `approve_hook` argument, or
  `HookHeaderMismatch`.

Choose the smallest number that works: every pool on your hook pays it on every callback, and it is
fixed for the life of the approval.

## 6. `hook_authority`

```
hook_authority = find_program_address([b"hook_authority"], your_program_id)
```

`approve_hook` derives it itself and stores the address and bump in `ApprovedHook`. It is your
program's identity to the AMM:

- Every **hook-initiated** instruction is signed by it.
- An AMM instruction whose acting signer is the pool's `hook_authority` is a **self-call**: every
  callback is skipped and it settles against claims (so you cannot re-enter your own hook through
  the AMM).
- `register_mint_exclusivity` requires the signer to be `approved_hook.hook_authority`, and
  `approved_hook.allow_exclusivity == 1`, or `ExclusivityNotAllowed` / `NotHookAuthority`.

Your header's `authority_bump` is for your own `invoke_signed`; Carpenter does not read it.

## 7. `allow_public_pools`

An `approve_hook` **argument**, not a flag, stored in the record.

- `true` — anyone may initialize a pool against your hook.
- `false` — **your own `before_initialize` is what refuses an unregistered initializer.** The AMM
  never enforces this field; it is a statement in the record that frontends and indexers read.
  Because only you can enforce it, `approve_hook` requires `BEFORE_INITIALIZE` to be set whenever
  `allow_public_pools` is `false` — asking for `false` without it is `HookFlagsInvalid`.

`allow_exclusivity` is the other term the header cannot carry: it is what lets your `hook_authority`
call `register_mint_exclusivity`. Both are the applicant's to ask for and governance's to grant, and
neither is re-read by `reapprove_hook`.

## 8. Every refusal, by name

`approve_hook`, in the order it checks — the Apply page's nine rows follow the same order:

| Order | What failed | Error |
|---|---|---|
| — | The signer is not `AmmConfig.authority` | `Unauthorized` |
| — | A record already exists for this program (Anchor `init`) | the `init` fails; a hook is approved once |
| 1 | Not executable, not owned by the upgradeable loader, or its `Program` account does not point at the `ProgramData` passed | `HookNotExecutable` |
| 2 | `hook_flags_valid` refused the flags (§2) | `HookFlagsInvalid` |
| 2 | `allow_public_pools: false` without `BEFORE_INITIALIZE` | `HookFlagsInvalid` |
| 3 | `hook_config` is not the `["hook_config"]` PDA owned by the hook program, is too short, or its `version` / `max_rounds` / `callback_accounts` disagree with the arguments | `HookHeaderMismatch` |
| 4 | `max_rounds > 4` | `RoundsTooHigh` |
| 5 | `AFTER_SWAP` with `max_rounds < 2` | `HookFlagsInvalid` |

Afterwards, at `initialize_pool`:

| What failed | Error |
|---|---|
| No `ApprovedHook`, or one that names another program | `HookNotApproved` |
| `ApprovedHook.paused != 0` | `HookPausedForNewPools` |
| The hook's live `ProgramData.slot` is not the pinned `programdata_slot` | `HookUpgradedReapprove` |

And at `reapprove_hook`:

| What failed | Error |
|---|---|
| Not the config authority | `Unauthorized` |
| The record names another program | `HookNotApproved` |
| The stored flags no longer pass `hook_flags_valid` | `HookFlagsInvalid` |
| The live header's `version` / `flags` / `max_rounds` / `callback_accounts` no longer equal the stored terms | `HookHeaderMismatch` |

Runtime refusals a hook can trip once it is live, named so you can recognise them in a log:
`HookBadReturn`, `HookFieldNotAllowed`, `HookDeltaExceedsSwapAmount`, `HookDeltaExceedsSwapOutput`,
`HookDeltaNegative`, `HookActionAccountMissing`, `HookClaimsInsufficient`, `HookRoundsExceeded`,
`HookSliceLengthMismatch`, `HookSliceAliasesAmmAccount`, `HookFeesInconsistent`, `NotHookAuthority`,
`NoHook`.

## 9. `ApprovedHook`, as stored

What `approve_hook` writes, and what anyone can read back at `["approved_hook", hook_program]` —
192 struct bytes, 200 with the discriminator. Struct offsets (account offset = struct offset + 8):

| Offset | Field | Type |
|---|---|---|
| 0 | `version` | `u8` |
| 1 | `bump` | `u8` |
| 2 | `paused` | `u8` |
| 3 | `max_rounds` | `u8` |
| 4 | `callback_accounts` | `u8` |
| 5 | `allow_public_pools` | `u8` |
| 6 | `flags` | `u16` |
| 8 | `hook_program` | `[u8; 32]` |
| 40 | `hook_authority` | `[u8; 32]` |
| 72 | `hook_authority_bump` | `u8` |
| 73 | `pad0` | `[u8; 7]` |
| 80 | `programdata_slot` | `u64` |
| 88 | `approved_slot` | `u64` |
| 96 | `name` | `[u8; 32]`, zero-padded UTF-8 |
| 128 | `allow_exclusivity` | `u8` |
| 129 | `base_transfer_hook` | `Pubkey` — the ONE armed Token-2022 transfer hook admitted on a base mint for pools of this hook (zero = none); set once by `set_base_transfer_hook` |
| 161 | `reserved` | `[u8; 31]` |

⛔ `name` is **32 bytes, not 32 characters** — `é` is two bytes and an emoji four.

The `HookApproved`, `HookReapproved` and `HookPaused` events carry the same facts, so an indexer
never has to poll the account to learn that something changed.
