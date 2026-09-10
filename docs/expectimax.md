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

## The tunable search: `expectimax-v2`

`settlers_expectimax::v2::analyze(&view, &history, &config, offered_turn, &clock)`
is a second search in the same crate. It keeps the reference's inputs, resource
knowledge, hidden-world sampling, trade enumeration, and report shape, and
replaces the search and the leaf. The reference is unchanged and remains the
frozen comparator; a report from the tunable search carries `policy:
"expectimax-v2"` and echoes its `search` configuration.

### Search

A level is one of the root player's turns. The current turn is planned as a
small tree: every candidate first action is kept (builds, development plays,
purchases, roads that shorten the path to a reachable site or extend a
contested longest road, bank exchanges that reduce a build deficit, and ending
the turn), later steps keep `EndTurn` plus the three, two, then one best
actions by static value, and long turns only end. Purchases, thefts, and the
player's own roll branch exactly over their outcomes; positions reached by
different action orders share one transposition entry. Discards enumerate every
legal multiset and keep the four that best preserve the current build target.
The value of a first action is the maximum over its continuations of the
expectation over chance, so a candidate is credited with the best line it
enables, never an average over lines.

Lookahead is spent on the afterstates of the chosen line. Between levels, every
other seat plays a fixed builder policy on its sampled hand under common random
scenarios: stratified dice for the first rolls of the round, then a private
stream for further rolls, thefts, and draws. The same scenarios apply to every
candidate, so comparisons are paired. At deeper levels the player's own turn
follows one greedy line guided by the leaf, and its afterstate is either
evaluated (deepest level) or followed through another round. Leaves are
evaluated at the end of the deepest planned turn.

Iterative deepening runs depth 1, 2, and so on up to `depth`, and plays the
deepest depth that completed within `time_budget_ms` and `node_budget`. A depth
is abandoned as soon as either budget runs out; deepening also stops early when
the elapsed time exceeds `deepen_fraction` of the budget (default 0.3), because
the next depth costs several times more. Two decision-aware rules spend depth
where it matters: a position with a single candidate never deepens past the
first depth, and with `deepen_gap` above zero deepening stops as soon as the
best candidate leads the runner-up by at least that many leaf units (a settled
decision), so close decisions get the deep search and clear ones do not. With
`time_budget_ms: 0` only the node budget and the gap rule apply and reports are
reproducible bit for bit.

| Setting | Default | Meaning |
| --- | --- | --- |
| `depth` | 2 | Cap on planned own turns (1–6) |
| `time_budget_ms` | 0 | Wall-clock limit; 0 disables the clock |
| `node_budget` | 400000 | Engine transitions per analysis |
| `samples` | 4 | Sampled hidden worlds (1–32) |
| `scenarios` | 8 | Common random scenarios after the current turn (1–64) |
| `inner_scenarios` | 4 | Scenarios at deeper levels |
| `beam` | 4 | Reserved; later steps of the current turn keep three, two, then one action |
| `afterstates` | 96 | Reserved cap on end-of-turn positions per planned turn |
| `iterative` | true | Iterative deepening; false searches only `depth` |
| `deepen_fraction` | 0.3 | Deepen only while elapsed time is below this share of the budget |
| `deepen_gap` | 0 | Stop deepening once the best candidate leads by this many leaf units; 0 disables |
| `explain` | false | Report the expected end-of-turn outcomes of the chosen action with probabilities |
| `weights` | see below | Leaf weights |
| `inclinations` | all 0.5 | Sliders in `[0, 1]` that scale groups of weights |

### Leaf

Points, cards, and production share one currency. A card is worth `card_value`
heuristic points; production is worth its expected cards over the rolls that
remain, estimated from `turns_per_player` and the current turn. The terms are:

| Term | Rule |
| --- | --- |
| `points` | Own victory points × `point` |
| `hand` | Cards held × `card_value` × `hand_discount` |
| `production` | Σ rate × `resource_value` × `card_value` × remaining rolls |
| `plan` | Best target (city, settlement plus roads, development) valued at its gain divided by one plus the turns until affordable at current production, × `readiness` |
| `expansion` | Best reachable site's production-equivalent × `expansion` plus `expansion_sites` per other site |
| `diversity` | Share of resource types produced at a useful rate × `diversity` |
| `development` | Held non-victory cards × `development` plus knights played × `knight` |
| `road` | Longest trail × `road_length`, plus half of `road_contention` when within one road of the award |
| `hand_risk` | Expected cards lost to a seven before the next turn × card value × `hand_risk` |
| `ports` | Matched 2:1 production × `port_match` and a flat `port_generic` |
| `opponent_progress` | Highest opponent score × `opponent_max_point` plus all opponents' scores × `opponent_sum_point` |
| `opponent_production` | Highest opponent production × `opponent_production` |
| `terminal` | ± `terminal` at a finished game |

`Inclinations` are five sliders: `aggression` scales the opponent terms, `risk`
scales the discard exposure inversely, `development` scales development-card
terms, `expansion` scales site and road terms, and `trade` sets the margin a
proposal or acceptance must beat the plain alternative by (24 points at 0, none
at 1) and scales the reference willingness model. A slider at 0.5 leaves the
weights unchanged; 0 halves and 1 multiplies by 1.5.

Contributions in a report are the mass-weighted sums of these terms across the
expanded continuations. The reported `statistics` include transitions, leaves,
afterstates per level, expansions, transpositions, scenarios, the depth completed,
and elapsed milliseconds.

### Learned n-tuple leaf

`leaf` selects the evaluator. By default the hand-written terms above apply.
With `"leaf": {"tables": "PATH"}` the search reads a file of learned lookup
tables (`settlers_expectimax::ntuple`) and values every position as the sum of
about eighty looked-up numbers, in win-probability units, multiplied by
`leaf.scale` (default 1000, so one win is worth ten points on the hand-written
currency). `leaf.hand` adds the hand-written terms scaled by that factor; zero,
the default, uses the tables alone. Nothing else in the search changes: the
tables are consulted at end-of-turn positions, for static narrowing, and for
the leaf-guided opponent model when that switch is on. A finished game is
exact in both leaves.

Three families of windows share tables across the board: hex windows (resource,
pips, robber, six corners in five states, canonical under the hex's twelve
symmetries), intersection windows (the three touching hexes as a sorted triple,
the building or blocked state, own or foreign road adjacency, the port), and
five portfolio tables (production pips per resource; hand, hand total,
settlements and cities left; own and the two best opponent scores with the
turn bucket; knights, cards held, army and road holders, own trail, roads
left, deck bucket; the largest opponent hand, best opponent production,
opponent count, own production, best opponent score). Every window reads only
the seat's own hand and cards plus public information, so one table serves
every seat, the search leaf, and training.

Tables load once per process and are shared by every seat naming the same path;
reports from a tables leaf carry `policy: "expectimax-v2-tables"`. The browser
build rejects a tables leaf: learned tables are a server-side policy that
connects to games through the `remote` runner.

`train` in `crates/arena` learns the tables by self-play temporal-difference
training on shared tables across all threads. Every seat plans its turn with the
depth-1 search over the tables on the true state (the leaf never sees hidden
hands); at game end each seat's chain of end-of-turn positions is updated
backward with TD(lambda) toward 1 for the winner and 0 otherwise, plus a small
reward per point gained, with temporal-coherence step sizes. The first
`--warmup-games` are played by the fast builder so the tables start from
finished games. Each chunk it can validate the frozen tables as the depth-2
leaf against the hand-written leaf with the two builders on fixed deterministic
seeds, keeps the best validation point as `best.bin`, and stops on a game, wall,
or plateau budget:

```sh
cargo run --release -p settlers-arena --bin train -- \
  --out runs/ntuple/full --layout hex,vertex,portfolio --wall-seconds 7200 \
  --chunk-games 40000 --validate-seeds 32 --plateau-window 3
```

### Explain mode

With `explain: true` a report carries `outcomes`: for the chosen plain action,
one list per planned level of the end-of-turn positions the search expects,
each with its probability along that action (dice, purchases, thefts, and
sampled hands inside the turn, and the common random scenarios of the
opponents' round between turns), the value assigned to it, and a public-safe
summary relative to the root position: the turn, own points, hand size,
settlements, cities, and roads gained, development cards bought, and whether
the game has ended and who won. Level 1 is the end of the current turn, level 2
the end of the next own turn, and so on; positions reached by different orders
or scenarios are merged by information set, the twelve most likely are listed,
and `other` holds the remaining mass. Explain mode never changes the decision
or the values; it records positions the search would have discarded, so it
costs memory and is off by default. The arena's `--trace` output carries the
outcomes for every recorded decision.

### Browser defaults

Seated browser workers request the tunable search with a depth cap of 3, a
one-second budget, four sampled worlds, eight scenarios, and five per-agent
inclination sliders sampled from the game and agent IDs. Native cost by depth on
sampled positions is roughly 15, 90, 465, and 1650 ms at depths 1 to 4; WASM is
about three times slower, so a one-second budget usually completes depth 2 and
sometimes depth 3.

### Offline engine arena

`crates/arena` plays complete games between seat policies without a server or
network. Every seat receives `observe(&state, Some(seat))` and the participant
event batches it would see, including theft receipts only for thief and victim.
Games take an explicit seed, so lineups can be compared on identical boards, and
`tournament` rotates a lineup through every seat for each seed:

```sh
cargo run --release -p settlers-arena --bin tournament -- \
  --seat 'v2:{"depth":3,"time_budget_ms":800}' --seat eta --seat fast --seat eta \
  --seeds 0-63 --threads 32 --out games.jsonl --trace
```

Seat specs are `fast`, `eta`, `legal`, `v1`, `v2`, or `v1:{json}` and
`v2:{json}` with a configuration object; `v2:{"leaf":{"tables":"PATH"}}` seats
the search with learned tables. Output is one JSON line per game with
the winner, per-seat points, builds, trades, discards, decision timing, and search
effort; `--trace` adds each search seat's top candidates per decision. Player
trades are resolved by asking each recipient in seat order; the first acceptance
commits. This arena is a development and diagnostic tier. It is not the
authoritative protocol arena, and results from it are labelled as engine-arena
evidence in the research repository.

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
`{"view": ..., "config": ...}` for the reference search, or
`{"view": ..., "search": {...}}` for the tunable search, whose `time_budget_ms`
uses the browser clock. Event batches are keyed by sequence, making
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
