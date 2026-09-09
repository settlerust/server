# Policy and game-theory research

## Fair information sets

CATAN is a multiplayer, partially observed stochastic game with negotiation. A deterministic full-state expectimax value is useful for diagnostics, but a fair policy must act from its information set. The server supplies exact own resources/development cards, public board and production, card counts, public actions, and permitted communication. It never supplies opponents' hands, development types, random seed, or future outcomes.

A language-independent policy loop is:

1. Acquire/persist one JWT and join a seat.
2. Consume typed public event batches, with a persistent event cursor.
3. Maintain beliefs about hidden resources/development cards and opponent behavior.
4. On each snapshot, check whether the policy has a decision (including off-turn discards or trade offers).
5. Score choices and submit one UUID/versioned command.
6. On conflict refresh and re-evaluate; on uncertain transport resend the original request; on reconnect recover events before continuing a history-dependent policy.

Use structured communication intent/resource fields for feature extraction, with natural-language text as an optional additional observation. Directed communication and private filters alter information availability; model that explicitly in experiments.

## Rust simulation API

```rust
use settlers_engine::{State, Settings, Action, apply, legal_actions, chance_outcomes, apply_with_chance};
let state = State::new(Settings::default(), [7; 32])?;
let action = legal_actions(&state, state.current)[0];
let successor = apply(&state, state.current, action)?;
# Ok::<(), settlers_engine::RuleError>(())
```

`State` is a small `Copy` value. Normal application consumes deterministic ChaCha12 randomness. Exact chance enumeration is available once a roll, purchase, or theft is legal:

```rust
# use settlers_engine::*;
# let mut state = State::new(Settings::default(), [7; 32])?;
# while state.turn == 0 { state = timeout(&state)?.0; }
let branches = chance_outcomes(&state, state.current, Action::Roll)?;
for branch in branches {
    let probability = branch.numerator as f64 / branch.denominator as f64;
    let next = apply_with_chance(&state, state.current, Action::Roll, branch.outcome)?;
    // Accumulate probability * evaluator(next).
}
# Ok::<(), RuleError>(())
```

Dice have 36 ordered equiprobable outcomes. Theft branches are weighted by the victim's resource counts. Development branches are weighted by the remaining deck counts. Explicit outcomes validate admissibility and **do not advance the seeded RNG cursor**. A solver should continue using explicit branches for its stochastic nodes rather than interpreting the unchanged cursor as a sample conditioned on that branch.

Only the offline engine accepts `Chance`. The network protocol never accepts fabricated dice, card draws, or stolen resources. For fair search, draw hidden-state determinizations from a belief compatible with the player's observation, then use exact branches within those sampled states. Keep deterministic RNG internals out of a strategic transposition key when using explicit chance branches. Preserve current player, phase, newly bought cards, played-card flag, award holders, and rule settings; they affect legal continuations even when occupancy is identical.

## Baselines

`crates/policies/fast_builder/` uses production-rich opening placement, short road paths toward settlements, city upgrades, resource-preserving discards, bank conversion, and leader-aware robber choices. `eta_builder/` ranks construction using estimated acquisition time and production gain. Both play development cards and can accept trades that reduce their current build deficit.

The baseline ideas were compared with JSettlers' `OpeningBuildStrategy`, `DiscardStrategy`, `RobberStrategy`, `MonopolyStrategy`, and `SOCRobotDM` FAST/SMART descriptions. These are independently implemented, intentionally simple baselines. The ETA policy uses a local acquisition-time estimate; it does not reproduce JSettlers' full opponent tracking or win-game ETA search. Neither baseline has privileged access to authoritative state. Shared planning lives outside the strategy-specific directories.

`Policy` provides `choose_action`, `accept_trade`, `on_event`, and `conversation` hooks. Its default `choose` checks acceptable incoming offers, invokes the `conversation` hook for an optional public message, then selects a game action. Stateful conversation hooks should return each intended message once so they do not starve game actions. New policies can override `choose` to originate offers, send directed messages, and set filters. Strategies in any other language use the same `Command` schema and need not link Rust.

## Offline engine arena

`crates/arena` runs paired-seed tournaments between the builders and both
expectimax searches with only redacted observations and participant events
crossing into a seat. See [expectimax.md](expectimax.md#offline-engine-arena).
It is a development tier: the protocol arena in the research repository remains
the confirmation tier.

## Experiments

Record the game ID, policy versions, rules configuration, complete typed observations/communications, action UUIDs, and evaluation protocol. The private archive is suitable for auditing exact states and replay; evaluate policies against redacted information rather than training/evaluating with future or hidden cards by accident.

Start with reproducible baseline win rates, seat effects, completion rates, average turn count, trade acceptance/rejection, robber targets, and communication ablations. Compare policies across many seeds and balanced seats. Report uncertainty and match sample counts. An all-knowing expectimax upper-bound policy should be labeled separately from a fair policy.

The engine benchmark isolates core operations. The simulation runner includes view construction and heuristic decision overhead. The arena additionally includes authentication, HTTP, serialization, and durable commits. These are distinct measurements; storage throughput is not engine transition throughput.
