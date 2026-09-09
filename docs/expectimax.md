# Reference expectimax player

`crates/expectimax` is a Rust policy library that can run natively or compile to
WebAssembly for a browser Web Worker. It ranks ordinary game commands and
precomputed trade proposals from a seated player's redacted observation. The
authoritative engine remains independent of policy, networking, credentials,
storage, and model inference.

The policy is a bounded reference implementation, named `expectimax-v1` in its
reports. Its values are additive heuristic points. They are not calibrated win
probabilities, and the implementation makes no claim of superiority over other
players.

## Inputs and public resource knowledge

The native entry point is:

```rust,ignore
settlers_expectimax::analyze(&view, &history, config, offered_turn)
```

`view` is a participant `GameView`; `history` contains that same participant's
redacted `EventBatch` history. `offered_turn` remembers the most recent turn in
which the policy's own offer was committed. Spectator `research_hands` input is
explicitly rejected. Never pass authoritative states, another identity's event
history, or the research spectator projection into a policy.

`settlers_protocol::knowledge::ResourceKnowledge` makes resource accounting
available to other Rust policies as well. Use `from_history_at_version` for a
versioned snapshot. For each seat it exposes `minimum`, `maximum`, `expected`,
and public `total`; resource arrays follow engine order: brick, lumber, wool,
grain, ore. `can_have(Resources)` rejects requests impossible under those bounds,
while `is_exact()` distinguishes exact counts from uncertain estimates.

`Observation.bank` publishes the exact remaining count of each resource to every
player and spectator; `bank_available` remains its Boolean convenience projection.
Knowledge reports repeat `bank` and provide seated `opponent_totals`:
`19 - bank[resource] - own_hand[resource]`. These are exact combined holdings across
all other seats, not a guess about which opponent holds the cards. A full bank
pile proves nobody holds that resource; a nearly full pile leaves few circulating
cards, rather than making player trades easier. With two players, the bank and
own hand determine the other resource hand completely. Development-card types
and future randomness remain hidden.

Public setup grants, production, spending, bank trades, completed player trades,
and revealed development-card effects update these estimates. Private theft
receipts are used only when present for the receiving identity. Hidden losses
widen bounds and proportionally adjust the point estimate. Missing, stale, or
inconsistent history widens allocation knowledge while retaining exact public
bank constraints. This may recover a hand even without event history. A
funded open offer supplies an additional lower bound on its sender's cards.

The bounds are conservative marginals, not a joint Bayesian posterior. Sampling
enforces each player's public total and resource bounds, the global inventory of
19 cards per resource, and every exact public bank count. Bounds propagate across
seats and resources; proportional bounded projections adapt point estimates to
circulating abundance. A small integral flow with randomized, prior-weighted
edge ordering samples feasible hands without rejecting unlucky partial allocations.
This guarantees inventory feasibility when the bounded allocation is feasible,
but does not make the resulting sample an exact posterior. Hidden development cards
are sampled without replacement from the remaining public prior, accounting for
known own cards and public plays. Opponent held development cards are treated as
playable old cards; their acquisition-turn uncertainty is not modeled exactly.
The policy uses its own deterministic sampling streams and never the server's
hidden RNG seed or future deck.

## Search depth and fair leaves

| Setting | Default | Accepted range | Meaning |
| --- | --- | --- | --- |
| `depth` | 4 | 1–6 | Number of the root player's decisions searched explicitly |
| `beam` | 3 | 1–6 | Ordered continuation actions; end-turn is also retained in main phase |
| `samples` | 4 | 1–16 | Sampled hidden worlds |
| `node_budget` | 96 | 1–4096 | Per-root-candidate expansion allocation, divided among branches |

Values outside these ranges are clamped. The root compares its generated legal
commands and trade proposals; the beam applies to later decisions. Trade roots
split their allocation between accepted and rejected continuations. This budget
is not a strict total runtime or total-node cap: completing a fair continuation
still requires rollout work.

Depth and horizon are separate. Every nonterminal leaf occurs after the player's
next normal turn has ended. During the player's own turn, the policy completes
both the current turn and that player's next turn. Off-turn, it completes the
upcoming turn. During setup, it completes setup and that player's first normal
turn. A terminal win/loss can end a branch earlier.

For example, a four-player game at turn 9 with the root player currently acting
uses turn 14 as the earliest nonterminal leaf: turns 9 and 13 are that player's
current and next turns, and turn 14 begins after the next one ends. The report
includes `horizon_turn`, `minimum_leaf_turn`, and terminal-leaf counts so this
invariant can be inspected.

When depth or expansion allocation runs out, deterministic action ordering and
sampled chance continue each world to that same horizon. Rollouts force an end
turn after 16 main-phase actions and have a 400-transition continuation bound.
If continuation cannot reach the horizon, analysis returns an error instead of
evaluating an unfair early leaf.

MAX decisions are shared between worlds with the same seated observation, so a
player cannot choose a different action merely because sampled hidden cards
differ. Opponents use a fixed builder policy that reads their own sampled hand
and public board. This is not an equilibrium solver or an explicit search over
opponents' strategic intentions.

Dice totals use the exact 11 weighted outcomes corresponding to 36 ordered dice
pairs. Development draws and robber theft use the engine's card-count
probabilities. Chance outcomes are enumerated when the current allocation
permits; otherwise they are sampled with reproducible common streams. Reducing
the budget therefore changes the approximation, not the fairness horizon.

## Precomputed trades

Before sending a proposal, the policy enumerates one-resource 1:1 and 2:1 swaps
toward road, settlement, city, and development-card costs. It gives only surplus
cards relative to the target cost, checks recipient bounds, and considers each
recipient separately. It currently proposes at most once per turn and does not
search chains of future negotiated offers. Settlement relocation is also omitted
from this reference policy's candidates.

Each proposal has separate accepted and rejected continuations. The report
records modeled acceptance probability, sampled partner affordability,
conditional `acceptance_when_affordable`,
conditional accepted/rejected values, and newly affordable build actions. This
allows a city-unlocking trade to be identified before asking for it. Receiving
agents independently choose whether to accept through the normal game protocol;
simulation never grants an actual trade acceptance.

The baseline acceptance model is explicitly uncalibrated: 0.7 when the swap
reduces at least one recipient build deficit, otherwise 0.45 for a favorable card
count or 0.2 for a neutral count, halved when the proposing player is within two
public points of victory. An unaffordable trade has zero modeled acceptance.
These probabilities are assumptions for comparison, not observed intentions.
Uncertainty can make a resource possible without making it likely or guaranteed.
The reported unconditional probability is
`partner_affordability * acceptance_when_affordable`. Public bank counts affect
the first factor by constraining feasible holdings, not by asserting a partner's
willingness. Bank abundance is not a direct willingness bonus. A scarce circulating
resource can have uncertain ownership even when its combined opponent total is exact.
When sampled affordability is zero, `acceptance_when_affordable` is also zero as
a numerical placeholder; it provides no evidence about the partner's willingness.

## Leaf value and explainability

The leaf score sums the following fixed terms:

| Contribution | Rule |
| --- | --- |
| `points` | Own victory points × 100 |
| `production` | Expected unblocked resource production per roll × 24 |
| `diversity` | Sum of each resource production rate capped at 0.15, × 16 |
| `readiness` | Best of `12 / (1 + missing cards)` for city, settlement, or development purchase |
| `expansion` | Best reachable legal settlement site's production value divided by one plus required roads, × 16 |
| `development` | Own non-victory-point development cards × 6 |
| `opponent_progress` | Highest opponent public victory-point count × −35 |
| `terminal` | +10,000 for a root win; −10,000 for another player's win |

The heuristic reads own cards and public board/opponent progress. Expected
contributions sum to each candidate's reported value. These are exact additive
terms of this chosen heuristic, not SHAP values, causal explanations, or a
learned value model. `minimum_leaf_value` and `maximum_leaf_value` are extrema
among explored leaves, not guarantees across every possible future. The reported
principal variation is a representative continuation, not a complete contingent
plan.

## Browser and model integration

Build with the pinned WASM packager from this repository:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked --root target/wasm-tools
```

Then run `npm run build:expectimax` in the sibling web application. Its script
builds `settlers_expectimax.wasm` in release mode and runs the matching
`wasm-bindgen --target web`. Generated JavaScript, declarations, and WASM live in
the web application's ignored `lib/agents/wasm/` directory. Set `WASM_BINDGEN` to
use a different installation of the same pinned CLI version. The browser loader
uses a module asset URL inside each seated Web Worker.

The exported `ExpectimaxPlayer` accepts `on_events(batch_json)`,
`analyze(request_json)`, and `offered(turn)`. Its analysis request is
`{"view": ..., "config": ...}`. Event batches are keyed by sequence, making
replay and out-of-order delivery safe. Each worker maintains its own instance;
the WASM interface neither performs network requests nor receives an API key.

The web policies are `expectimax` and `expectimax-llm`. The former chooses the
best reference candidate. The latter sends the report to an authenticated
server-side endpoint, which loads a database-pinned model profile and private
notebook. It supplies the top three IDs, all candidate explanations, bounded
participant-visible social context, and an unchanged fair reference. A lower
scoring selection records the foregone heuristic value so cooperation, rivalry,
or bluffing hypotheses remain distinguishable from the search result.

The model selects one allowed decision string in a single pass. A single
`record_decision` tool call may also contain private scratchpad/strategy/memory
summaries and separate optional public table talk. There is one retry for an
invalid choice, followed by the best expectimax candidate. Missing configuration,
HTTP/network failure, or timeout also falls back; a single candidate skips model
inference. The model cannot change actual dice/card outcomes, invent a command,
or modify the search's fixed chance/acceptance assumptions. Public claims and
messages remain untrusted input. Public messages are limited to one per turn.

Credentials belong in the web/service server environments. Model IDs belong in
database profiles, not `BASETEN_MODEL`. The host configures and reviews agents at
`/game/GAME_ID/agents`; an agent's private notebook and revision history are
separate from public game messages. Optional carryover snapshots support iterated
games without assuming that opponent seat identities remain stable.

## Native smoke tests and validation

Build and inspect a deterministic city-trade fixture from this repository:

```sh
cargo build -p settlers-expectimax --release --locked --bins
target/release/expectimax-fixture > /tmp/settlers-expectimax-input.json
target/release/expectimax < /tmp/settlers-expectimax-input.json > /tmp/settlers-expectimax-report.json
```

The binary reads one JSON object with `view`, optional `history`, and optional
`config` from stdin. It writes one report to stdout and search timing to stderr.
The fixture is synthetic; its known target is a city that needs a precomputed
ore trade. It is not an offline record of a real player's private cards.

Focused checks:

```sh
cargo test -p settlers-expectimax --release --locked
cargo test -p settlers-protocol --test knowledge --locked
cargo test -p settlers-engine --test public_board --locked
```

Tests cover resource accounting/redaction, fair leaves with minimal allocation,
chance probabilities, reproducibility, trade branches, WASM-facing serialization,
and complete two-, three-, and four-player smoke games. Native completion tests
start from a completed setup fixture and include one reference seat with fixed
builder opponents and four reference seats playing together. Their funded offers
are assumed accepted; they are completion checks, not negotiation-strength or
competitive benchmark results.
Browser tests exercise the actual worker/WASM bundle and private MDX review.
Run the repository-wide checks in [development](development.md) before delivery.

For the combined live inference smoke, build the binaries above, configure the
web application's `.env`, and run from the sibling service repository:

```sh
node --import tsx ../web/scripts/smoke-expectimax-baseten.mts openai/gpt-oss-120b
```

That command makes a billable request, stores the explicit model in an isolated
test profile, and records report/summary/SQLite artifacts under the web
application's `.local/expectimax-smoke/` directory. It is an integration check,
not evidence that a model improves long-term results.
