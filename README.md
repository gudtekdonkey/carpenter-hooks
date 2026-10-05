# Carpenter hooks

This repository is two things and nothing else:

1. **Where hook approval applications are filed.** One issue per application, on the form in
   [`.github/ISSUE_TEMPLATE/hook-application.yml`](.github/ISSUE_TEMPLATE/hook-application.yml).
   Carpenter's **Apply** page prefills that issue for you.
2. **The hook-author starter and the interface it implements** —
   [`docs/hook-interface.md`](docs/hook-interface.md) and [`starter/`](starter/).

No Carpenter program source lives here. Everything below is either readable on chain or is told to
you when you apply.

## What Carpenter is

Carpenter is a concentrated-liquidity AMM on Solana, shaped after Uniswap v4: a pool is keyed by its
two mints, its fee, its tick spacing **and its hook program**, and the pool calls that hook at the
phases the hook asked for.

## What a hook is here

A hook is **your own Solana program**. Carpenter does not run your code in its address space and
does not link it: it CPIs into your program at the phases you declared, hands you the pool's state,
and reads a `HookRecordV1` back out of the call's return data. You declare which phases you want in
an 8-byte **interface header** stored in an account your program owns at the seed `["hook_config"]`,
and Carpenter reads that header when it approves you.

Two things a hook can do that a passive callback cannot, and they are why the header has the shape
it does:

- **Take a delta.** With `BEFORE_SWAP_RETURNS_DELTA` / `AFTER_SWAP_RETURNS_DELTA` your record can
  move value in the swap that called you (this is how a fee skim is expressed).
- **Ask for actions.** With `REQUESTS_ACTIONS` your record can ask the pool to run positions and
  swaps on your behalf and then call you back with the results, up to `max_rounds` times.

Your program also owns one PDA that Carpenter treats as your identity: `["hook_authority"]`, derived
under **your** program id. Every hook-initiated instruction is signed by it.

## The lifecycle

| | |
|---|---|
| 1. **Deploy** | Deploy your hook program under the upgradeable loader (loader-v3). Carpenter reads its `ProgramData`, so a program deployed `--final` is fine but a non-upgradeable-loader program is not. |
| 2. **Publish your header** | Create your `["hook_config"]` account and write the 8-byte `HookInterfaceHeader` into it — `version`, `bump`, `max_rounds`, `callback_accounts`, `flags`, `authority_bump`. This is the account Carpenter reads; see [`docs/hook-interface.md`](docs/hook-interface.md). |
| 3. **Apply** | Open an issue here (below). Carpenter's Apply page runs the same checks the chain runs, against your live accounts, and puts what it measured in the issue. |
| 4. **`approve_hook`** | The config authority signs `carpenter_amm::approve_hook(max_rounds, callback_accounts, allow_public_pools, allow_exclusivity, name)`. That writes an `ApprovedHook` record at `["approved_hook", your_program]`, pinning your terms and your program's current `ProgramData.slot`. |
| 5. **Pools** | Pools may now be initialized against your hook. Your terms are copied into every pool at `initialize_pool`, so a pool trades under the terms it was opened with. |

Three things happen after that, and they are the whole of the post-approval story:

- **An upgrade freezes NEW pools.** `ApprovedHook.programdata_slot` is pinned at approval and checked
  by every `initialize_pool`. The moment you upgrade your program, that slot no longer matches and
  **new pools are refused `HookUpgradedReapprove`**. Live pools keep trading. It is released by the
  config authority calling `reapprove_hook`, which re-reads your header, requires it to **still**
  agree with the stored terms (`version`, `flags`, `max_rounds`, `callback_accounts`), and moves the
  slot pin. Nothing else in the record moves — an upgrade that changes any of those four is a
  different interface and `reapprove_hook` refuses it `HookHeaderMismatch`.
- **A hook can be paused.** `set_hook_paused(paused)` sets `ApprovedHook.paused`. A paused hook
  blocks **new pools only** (`HookPausedForNewPools`); existing pools are untouched and no funds move.
- **Approval happens once.** `approve_hook` creates its account with Anchor `init`, so it can never
  run twice for one program id. Your terms are written at approval and are not editable afterwards;
  `reapprove_hook` moves the slot pin alone.

## The five checks an application must pass

These are `approve_hook`'s own checks, in the order the chain runs them, with the error each one
raises. The Apply page runs the same list against your live accounts before you submit.

| # | Check | Refusal |
|---|---|---|
| 1 | The hook program is **executable**, owned by the upgradeable loader, and its `Program` account points at the `ProgramData` passed | `HookNotExecutable` |
| 2 | The header's **flags are valid** — the v4 `isValidHookAddress` rules plus Carpenter's own (see the interface doc). Separately: asking for `allow_public_pools: false` **requires** `BEFORE_INITIALIZE`, because only your own `before_initialize` can refuse an initializer | `HookFlagsInvalid` |
| 3 | The **header agrees with the arguments**: `version == 1`, and the header's `max_rounds` and `callback_accounts` equal the ones being approved. (The `hook_config` account must also be the `["hook_config"]` PDA **owned by your program**.) | `HookHeaderMismatch` |
| 4 | `max_rounds <= 4` (`MAX_ROUNDS_CAP`) | `RoundsTooHigh` |
| 5 | If `AFTER_SWAP` is set, `max_rounds >= 2` | `HookFlagsInvalid` |

## How to apply

Use Carpenter's **Apply** page. It reads your program, your `ProgramData` and your `["hook_config"]`
account, runs the nine-row pre-check, and opens a prefilled issue here carrying every value it
measured — so a reviewer reads the same figures you saw.

⛔ **Only a NON-UPGRADEABLE hook is listed** (owner ruling 2026-10-05: "hooks should be non-upgradebale to be listed by us"). Before applying, set your program's upgrade authority to none (`solana program set-upgrade-authority <program> --final`). Carpenter refuses an application whose `ProgramData` still names an upgrade authority. Why: a hook runs on every swap and liquidity change of its pools and signs as the pool, so code that can change after review could block withdrawals or misuse that signature. FLOOR's own hook is the one exception: it is upgradeable by FLOOR's deployer key, as published.

Applying by hand works too: open an issue on the **Hook approval application** form and fill it in.
The form asks for the same things; it just does not have the measured values, which a reviewer will
then read for themselves.

You are asked for: your program id and cluster, a name (**32 bytes**, not 32 characters — it is
stored on chain as `[u8; 32]`), a contact, a link to your source, an optional audit report, two
lines on what the hook does, and which pools it is for. `max_rounds` and `callback_accounts` are
**not** asked for: they are your header's, and the reviewer reads them off the chain.

## What approval is, and what it is not

Approval is a **compatibility check, signed by one key**. Today a single deployer key holds
`AmmConfig.authority`, and that key decides whether a hook program may have pools on Carpenter.

It is **not an audit**, it is **not an endorsement**, and it is not a statement that your program is
safe, correct, or fit for anyone's money. It says that your program presented a header Carpenter can
read, that its flags and rounds pass the rules above, and that the authority chose to write the
record. Read the source of any hook whose pools you trade — including this one's starter.

There is no timeline here, and this document deliberately does not offer one.

## Devnet

Everything below is on **devnet** and readable on chain. Nothing is deployed on mainnet.

| | |
|---|---|
| `carpenter_amm` | `bwAPyuax51SULNhTY1JCKhYZWMHmsveZyZG5T6hzofh` |
| `AmmConfig` `["amm_config"]` | `3aceathyD6ne25kYpWtSyEJqMpjjbQ17qbDvkD8vNrFA` |
| FLOOR's hook (`floor_hook`) | `3KLZ1HMJSyfoAp9BrQuuiXtw6oAP2MHN2NqXZGrBUSxG` |
| FLOOR's `ApprovedHook` `["approved_hook", floor_hook]` | `9ZaAMpLjENVEocAi1xUskxiyPa79tko8twitnFjdprqY` |
| FLOOR's launch program (`floor_launch`) | `2MSgDHy6xs2eXtDLb1gDJ6QQZQFb4HpEC7Xbs6dNyxxn` |
| Address lookup table | `GnZs8G7c2grcSMvpGqocaXkHjHn5LVhpMynS5f8mx5t8` (18 addresses, active) |

FLOOR's hook is approved with `flags 0x60CC`, `max_rounds 3`, `callback_accounts 1`,
`allow_public_pools false`, `allow_exclusivity true`, name `FLOOR` — a worked example of the header
in [`docs/hook-interface.md`](docs/hook-interface.md).

The starter's program id, `8NR5f68SspkCNgyGBvjuWcqMbPw1kDzhbpKgqHkCeWJk`, is **not deployed on any
cluster** and must not be reused: change it before you build (see [`starter/README.md`](starter/README.md)).

## Licence

[Apache-2.0](LICENSE).
