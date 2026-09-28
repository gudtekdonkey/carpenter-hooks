# The starter

`src/lib.rs` is `mock_hook` — the reference hook `carpenter_amm` is tested against, published
verbatim. It is a **pass-through**: it implements every callback, writes the interface header, and
returns the empty record, plus a lot of test scaffolding that makes it misbehave on purpose.

⛔ **It does not build standalone today.** See [Building](#building) — the honest state is written
out there rather than hidden behind a `cargo build` that fails with a confusing error. Read it as a
worked reference for what a hook *is*, and take from it the parts you need.

⛔ **It is not deployed on any cluster**, and the id it declares —
`8NR5f68SspkCNgyGBvjuWcqMbPw1kDzhbpKgqHkCeWJk` — is not an address you may use. Change it first.

⚠ It is a **test fixture**. Roughly two thirds of the file exists to make a pass-through hook fail in
specific ways so the AMM's refusals can be tested: `MockState`, `MockScript`, the `RETURN_*` modes,
the `DELTA_*` modes, the reported-fee switch. None of that belongs in a hook you intend to deploy.
Delete it.

## What to read first

| In `src/lib.rs` | Why |
|---|---|
| `initialize_hook_config` | writes the 8-byte `HookInterfaceHeader` (`version`, `bump`, `max_rounds`, `callback_accounts`, `flags`, `authority_bump`) into the `["hook_config"]` PDA. This is the account `approve_hook` reads. |
| `HookConfig` | the account that header lives in: the 8 interface bytes first, then whatever you want. The mock keeps an `authority` and its own fields after them. `const _: () = assert!(size_of == 128)` and the `offset_of!` asserts are how it keeps that shape honest — keep that habit. |
| the eight pass-through callbacks | the shape of a callback that has nothing to say: require `pool_signer` to have signed, return `HookRecordV1::pass_through(version, phase, round)`. |
| `before_swap` / `after_swap` / `after_actions` | the three that take full `Args` and reply with `set_return_data`. Note the comment on why they return `Result<()>` rather than a typed value. |
| `forward` | **test-only** (compiled only with the `test-fixture` feature): a generic signed-CPI proxy the AMM's own tests use to act as a hook. It is NOT how a deployed hook acts on the AMM — a real hook makes each `invoke_signed` call from its own logic, for the one instruction it means, with its own checks. |
| `Callback` | every callback's accounts: `pool_signer` first, then your slice as remaining accounts. |
| `HookRecordV1` / `HookAction` | the record's exact Borsh shape, as the IDL carries it. |

The rules those pieces have to satisfy are in [`../docs/hook-interface.md`](../docs/hook-interface.md).

## What to change

1. **The program id.** `declare_id!` in `src/lib.rs`, and the matching entry in your `Anchor.toml`.
   Generate your own keypair; never reuse the id above.
2. **The package name** in `Cargo.toml` and `[lib] name` (and the `#[program] pub mod mock_hook`
   module name, which becomes your program's name in the IDL).
3. **The header's `flags`, `max_rounds` and `callback_accounts`.** In the mock these are arguments to
   `initialize_hook_config` so tests can vary them. A real hook usually wants them fixed —
   they are the contract your pools are opened under, and changing them later means a fresh
   approval, not a `reapprove_hook`.
4. **The hook logic.** Everything the callbacks do. The pass-through record is the floor, not a
   feature.
5. **Delete the test scaffolding**: `MockState`, `MockScript`, `ScriptAction`, `ScriptRound`,
   `SENTINEL`, the `RETURN_*` / `DELTA_*` / `ACTION_*` constants, `set_reported_fees`,
   `reported_fees`, the `scripted` machinery that reads them, **and `forward` with its `Forward`
   accounts struct and the `test-fixture` feature**. ⛔ `forward` is a generic signed-CPI proxy: anyone
   can make your `["hook_authority"]` PDA sign any instruction, so it hands your pools' claims (e.g.
   through `hook_withdraw`) to anyone. Never deploy it, even behind the feature (audit finding 2026-09-28).

## What must not change

- **The header's shape and offsets.** 8 bytes, at account offset 8..16, in the field order
  `version, bump, max_rounds, callback_accounts, flags (u16 LE), authority_bump, pad0`. `approve_hook`
  reads those bytes positionally; a field reordered or a type widened is a header it decodes into
  something you did not mean.
- **`version = 1`.** `HOOK_INTERFACE_VERSION` is 1; anything else is `HookHeaderMismatch`.
- **The PDA seeds.** `["hook_config"]` and `["hook_authority"]`, both derived under **your** program
  id, both spelled exactly like that. The AMM derives them itself; a differently-spelled seed simply
  means the AMM looks at an account that does not exist.
- **The record's Borsh shape** — `HookRecordV1` and `HookAction`, field for field, in that order. The
  AMM's decode is strict: a missing field, an extra field or one trailing byte is `HookBadReturn`.
  (The mock's own comment on `fees` is there because dropping that field failed twelve tests.)
- **`pool_signer` must have signed.** Every callback checks it. Without that check, anyone can call
  your hook pretending to be a pool.
- **The callback names.** `before_initialize`, `after_initialize`, `before_add_liquidity`,
  `after_add_liquidity`, `before_remove_liquidity`, `after_remove_liquidity`, `before_swap`,
  `after_swap`, `before_donate`, `after_donate`, `after_actions` — the AMM CPIs by discriminator,
  which Anchor derives from the name.

## Building

`src/lib.rs` is published **byte-identical** to the copy in the private AMM workspace, which means it
still carries one dependency this repository cannot satisfy:

```
carpenter-types = { path = "../../../crates/carpenter-types" }
```

`carpenter-types` is not on crates.io and its source is not public yet, so **the starter is a
documented reference, not a buildable template**, and `Cargo.toml` here says so rather than pointing
the path at something that does not exist. Nothing in this repository has been compiled.

It takes exactly **six things** from that crate. Every one is part of the interface you are
implementing anyway, so you can declare them yourself in your own crate until the crate is public —
this is the minimal alternative, written out rather than shipped as a guess:

| Used as | What it is |
|---|---|
| `carpenter_types::seeds::HOOK_CONFIG` | `pub const HOOK_CONFIG: &[u8] = b"hook_config";` |
| `carpenter_types::seeds::HOOK_AUTHORITY` | `pub const HOOK_AUTHORITY: &[u8] = b"hook_authority";` |
| `carpenter_types::hook::ArgsHeader` | `struct ArgsHeader { version: u8, phase: u8, round: u8 }` — the prefix every `Args` begins with. |
| `carpenter_types::hook::HookRecordV1` (aliased `TypesRecord`) | the record, with `pass_through(version, phase, round)` returning `Self { version, phase, round, ..Default::default() }`. Its field list is the `HookRecordV1` **already declared at the bottom of `src/lib.rs`** — the IDL twin is byte-for-byte the same Borsh encoding, and the file's own `impl From<TypesRecord> for HookRecordV1` shows the mapping. |
| `carpenter_types::hook::HookAction` (aliased `TypesAction`) | `enum { ModifyPosition { salt: u64, tick_lower: i32, tick_upper: i32, liquidity_delta: i128 }, Swap { zero_for_one: bool, amount_specified: i128, sqrt_price_limit_x64: u128 } }` — again, the same enum is declared in `src/lib.rs` as `HookAction`. |
| `carpenter_types::hook::{FEE_SKIM, FEE_CREATOR, FEE_HOLDER, FEE_PLATFORM}` | the indices `0, 1, 2, 3` into the record's `fees: [u64; 4]`. Used only by the mock's fee-split scaffolding, which you are deleting. |

Beyond that you need the usual Anchor workspace around it: an `Anchor.toml` naming your program id,
a workspace `Cargo.toml` with `[workspace] members = ["programs/<your hook>"]`, and a toolchain that
builds Anchor 1.2.0 programs. Those are not shipped here, because an untested build recipe is worse
than none.

## Licence

Apache-2.0, per this repository's [LICENSE](../LICENSE).

⚠ The private copy's manifest carries `license = "UNLICENSED"`, which is the default a private
test crate was created with. The owner's recorded ruling for FLOOR's own Rust code going public is
Apache-2.0, and that is what is applied here; the `license` field in `starter/Cargo.toml` is the one
line changed from the private manifest for that reason.
