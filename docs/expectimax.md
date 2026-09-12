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
purchases, roads that shorten the path to a reachable site, extend a
contested longest road, or cut the leading opponent's path to their best
site when `opponent_expansion` is set, bank exchanges that reduce a build
deficit, and ending the turn), later steps keep `EndTurn` plus the three,
two, then one best
actions by static value, and long turns only end. With `spend_down`, a hand
over the discard limit also lists up to three bank exchanges and two roads
chosen only to shed cards, so the `hand_risk` term can price spending down
against holding. Purchases, thefts, and the player's own roll branch exactly
over their outcomes; positions reached by different action orders share one
transposition entry. Discards enumerate every legal multiset and keep the four
that best preserve the current build target; with `discard_projection` the kept
hand is first credited with the expected production of the rolls before the
next own turn, so a discard is valued by the target it preserves after that
production. The value of a first action is the maximum over its continuations
of the expectation over chance, so a candidate is credited with the best line
it enables, never an average over lines.

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
| `cache` | `none` | Transposition tables: `none` per depth pass, `decision` across the passes of one decision, `turn` across the decisions of a turn |
| `spend_down` | false | Over the discard limit, also consider bank trades and roads chosen only to shed cards |
| `discard_projection` | false | Rank discards by the kept hand plus expected production before the next own turn |
| `weights` | see below | Leaf weights |
| `inclinations` | all 0.5 | Sliders in `[0, 1]` that scale groups of weights |
| `endgame` | all off | Endgame switches: race leaf, hidden victory points, late leader bias |
| `knight_play_value` | false | Played knights valued by largest-army distance, and a held knight credited with the production it unblocks |
| `buy_tempo` | false | A development purchase is a candidate only when no build is affordable within the next own turn |
| `hold_knight` | false | Main-phase knight plays only when the robber sits on this seat's best hex |

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
| `road` | Longest trail × `road_length`, plus half of `road_contention` when within one road of the award; with `road_contention_gate` set to 1 the credit applies only while the seat ties or leads the table in brick plus lumber production |
| `hand_risk` | Expected cards lost to a seven before the next turn × card value × `hand_risk` |
| `ports` | Matched 2:1 production × `port_match` and a flat `port_generic` |
| `opponent_progress` | Highest opponent score × `opponent_max_point` plus all opponents' scores × `opponent_sum_point` |
| `opponent_production` | Highest opponent production × `opponent_production` |
| `denial` | Open reachable sites other seats' predicted builds are heading for, each discounted by `1 / (1 + their turns to afford the whole line)`, × `denial` (zero by default, so the term is absent) |
| `opponent_expansion` | Leading opponent's best reachable site, discounted by its road distance, × `opponent_expansion` (negative; default 0 disables the term) |
| `terminal` | ± `terminal` at a finished game |

`Inclinations` are five sliders: `aggression` scales the opponent terms, `risk`
scales the discard exposure inversely, `development` scales development-card
terms, `expansion` scales site and road terms, and `trade` sets the margin a
proposal or acceptance must beat the plain alternative by (24 points at 0, none at
1) and scales the reference willingness model. A slider at 0.5 leaves the
weights unchanged; 0 halves and 1 multiplies by 1.5.

### Endgame switches

`endgame` holds three switches, all off by default so the leaf and the candidate
shortlists are unchanged. Each is one hypothesis about the endgame.

| Setting | Default | Meaning |
| --- | --- | --- |
| `race` | false | Race leaf: once an opponent is within `trigger` points of victory, the opponent terms are replaced by the race margin and the leader's expected points next turn |
| `trigger` | 2 | Opponent points from victory that arms the race leaf |
| `margin` | 1.0 | Weight of the own-minus-leader margin, in multiples of one point |
| `tempo` | 1.0 | Weight of the leader's expected points next turn, in the same units |
| `hidden_points` | false | Count the victory cards sampled into each hidden world instead of public points alone |
| `block_leader` | false | Bias the robber and blocking roads toward the points leader late |
| `block_from` | 2/3 | Share of the estimated game length after which `block_leader` arms |
| `block_bias` | 1.0 | Strength of the late leader bias |

When `race` arms, the leaf drops `opponent_progress` and `opponent_production`
and adds two terms: `race_margin` is own points minus the leader's points times
`margin` point values, and `leader_tempo` subtracts the leader's expected new
points next turn times `tempo`. The expected gain is the best of a city, a
reachable settlement, and a development purchase, each times the share of its
cost the leader's sampled hand plus one roll of their production covers, with the
purchase priced at the deck's remaining victory-card share and blocked production
discounted. Own points stay in their own term, so the margin makes a point worth
double while the leader's points count fully against the seat.

`hidden_points` swaps public points for the expected full score in every opponent
term: each sampled world already holds the opponents' hidden development cards
drawn from the remaining deck prior, so a leaf averaged over worlds counts the
expected hidden victory points from public purchases and plays. The race trigger
and the leader's margin read the same estimate.

`block_leader` changes only the candidate shortlists, never the leaf. Robber
placements score the leader's blocked production and stolen cards with
`block_bias` times their usual points weight, and roads that lengthen the
leader's route to the best site both seats contest score half that site's
discounted value per three roads of denial, once the game is past `block_from` of
its estimated `turns_per_player` length.

### Development-card timing switches

Three development-card switches change how the search values and times cards; each is one
registered mechanism and defaults to the behaviour above. `buy_tempo` and `hold_knight`
are candidate rules and act under either leaf. `knight_play_value` shapes the hand-written
development term, so it applies wherever those terms are evaluated: the hand leaf, and the
`leaf.hand` blend under a tables leaf; the tables alone ignore it. With `knight_play_value`
the development term replaces the flat `knight` per played knight with a largest-army
schedule: half the weight when three or more knights from taking the award, the full
weight at two away, 1.75 times at one away, and 1.25 times while holding it; a held
playable knight is also credited with one roll of the production it would unblock, so a
knight held against a robber on this seat's own hex is worth more. With `buy_tempo` a
development purchase is generated only when no road, settlement, or city is affordable
within the next own turn at current production (the leaf's acquisition-time estimate),
making the buy a fallback for stalls rather than a competitor to visible builds. With
`hold_knight` a main-phase knight play is generated only when the robber sits on this
seat's best-producing hex, so knights are kept for the before-the-roll unblock; the roll
phase keeps both choices, and the search still decides whether the play is worth it there.

Contributions in a report are the mass-weighted sums of these terms across the
expanded continuations. The reported `statistics` include transitions, leaves,
afterstates per level, expansions, transpositions, scenarios, the depth completed,
and elapsed milliseconds.

### Opening placement

The two setup settlements and their roads are placed by the opening planner in
`v2/opening.rs`, not by the lookahead. The search's own ranking of setup sites
values raw production over the rolls that remain and adds the noise of the
first few rolls; a paired engine-arena cohort of 256 games (64 deterministic
boards, four rotations) put the planner-decided seat at 47% wins against 38% for
the previous search-ranked placement, and the earlier pip-only planner drafts
lost by 30 points to it, so the currency matters more than the search.

The planner scores a pair of sites as a portfolio, in pips (one pip is one
thirty-sixth of a card per roll):

| Term | Rule |
| --- | --- |
| `production` | Pips per resource × the strategy's resource weights × a scarcity markup for resources thin on the island |
| `coverage` | Pips of the distinct dice numbers the pair covers, so two settlements on the same 6 count once |
| `balance` | A penalty for every resource the pair leaves below three pips, weighted by how much the strategy needs it |
| `tempo` | How soon the pair can afford a settlement with a road, and a city, letting surplus convert at bank and port ratios |
| `ports` | Matched 2:1 production and a small flat value for a 3:1 port |
| `expansion` | The best open site two or three roads away and how many open sites sit two roads away |
| `style` | The seat's `expansion` and `development` inclinations leaning toward the roads, cities, and cards strategies |
| `draft` | Value lost to the picks predicted before the seat's next placement (never positive) |

Five discrete strategies (`coverage`, `cities`, `roads`, `ports`, `cards`)
differ in resource weights, needs, and term weights. `coverage` is calibrated to
the leaf's resource values; the others are blended in by `opening_weights.specialize`
(zero values every pair by coverage alone, one takes the best specialized
strategy in full). The default is zero: in a 256-game sweep against builders the
fully specialized planner lost by 16 points and a half blend cancelled the
planner's gain, while the scarcity markup at full strength kept it (+12 points),
so specialization stays a research knob rather than a default. The seat order decides what is planned: a first placement is
valued by the best pair it still leaves after the seats ahead in the snake draft
have picked, predicted with the same scoring (a greedy draft plus a few sampled
drafts where opponents pick among their top sites); a second placement completes
the pair; the last seat in the order plans both placements together because
nobody picks in between. Roads head for the best open site within two more roads
after the remaining setup picks are predicted, with a little extra for reaching
the seat's other settlement or a 3:1 port.

| Setting | Default | Meaning |
| --- | --- | --- |
| `opening` | `planner` | `planner` decides setup directly; `search` is the previous behaviour: the search's own static-leaf shortlist of six sites ranked by the lookahead |
| `bargain` | `true` | Threat-aware pricing, counters, and reasoned declines; `false` keeps plain acceptance and proposals as the control |
| `opening_weights.production` | 1.0 | Multiplier on the production term |
| `opening_weights.coverage` | 0.15 | Multiplier on distinct-number coverage |
| `opening_weights.balance` | 0.2 | Multiplier on the thin-resource penalty |
| `opening_weights.tempo` | 1.0 | Multiplier on the build-time term |
| `opening_weights.ports` | 1.0 | Multiplier on the port term |
| `opening_weights.expansion` | 0.15 | Multiplier on the expansion term |
| `opening_weights.specialize` | 0 | Share of a specialized strategy's advantage that counts |
| `opening_weights.scarcity` | 1 | Strength of the scarce-resource markup |
| `opening_weights.samples` | 6 | Simulated drafts averaged for a first placement |

Setup candidates in a report carry the planner's terms as contributions, a
`note` such as "cities portfolio: site 38 gives 13 ore pips; expects to pair it
with site 21 (83% of predicted drafts)", and a principal variation that lists the
planned second site. The same planner places the opponents' setup inside the
lookahead, the fast and ETA builders (through `settlers_policies`), and the
browser builders (through the `opening_placement` WASM export), so every seat
opens the same way and the draft prediction is exact against greedy opponents.

### Bargaining

`v2/bargain.rs` replaces the reference trade enumerator for the tunable search.
Every other seat gets a threat in `[0, 1]` from public information: `race`
(ahead in points or near the finish), `space` (how much of this seat's best
reachable ground that seat can reach as fast), and `awards` (a live contest for
the longest road or the largest army). The threat sets a per-partner premium on
the trade margin (30 points at full threat) and a cost on every swap equal to a
share of what the partner gains from it (net cards, a build the swap completes
for them, progress toward one), scaled by the seat's `aggression` inclination.
Seats with a threat at or above 0.75 are never approached with offers, and the
most threatening seat's best reachable site is the `contested` site: an offer
whose unlocked build takes or touches it is worded as a block.

Proposals are one- and two-for-one swaps toward any build cost that give only
cards beyond the largest cost that still needs the wanted resource, plus
two-for-one asks when the partner is known to hold at least two of the wanted
resource. Each partner is valued separately (modeled acceptance × the accepted
continuation), and when two or more safe partners were worth asking, one open
offer to all of them is added with the combined acceptance. Every offer carries
a message naming the partner, the terms, and the reason (the build it completes,
a block, or spare cards).

An open offer addressed to this seat gets three kinds of answers. Accepting is
valued as before, less the partner's threat cost. As the non-current seat, and
once per foreign offer, counters ask for more of what was offered, an extra card
of something this seat is short of, a different resource in the same count, or
give one card less when two were asked; each is priced by the offerer's public
eagerness (a build the swap completes under their minimum known hand, a large
hand, an explicit request) and by the chance the original offer stays open if
the counter is declined. When neither accepting nor countering beats waiting, the
seat declines explicitly with a reason, from the most specific ground: the
offerer leads the race, the swap completes a build for a threatening seat, the
offerer competes for the same ground, the terms are thin (a counter was closer),
this seat needs the asked cards for its own next build, or a plain pass. Early
in the game a pass is short; later the reason is spelled out. Candidates carry
this table talk as `speech` (declines and acceptances) or inside the offer's
`message`, and a `note` with the threat, modeled acceptance, and partner gain.

The engine arena resolves counters: a recipient that answers an offer with its
own offer to the current player publishes it, the current seat is asked, and an
accepted counter closes the original as `resources_changed`. Explicit declines
are recorded as `trade_rejected` events. Seat records count `counters`,
`counters_accepted`, and `declined`.

Four switches separate the mechanisms, all `true` by default. `threat_pricing`
applies the threat premium on the margin and the partner cost; off, every
partner is priced at the plain margin. `counters` lets the seat answer an open
offer with a counter. `asks` lets proposals include two-for-one asks to partners
known to hold at least two of the wanted resource. Each is effective only while
`bargain` is on, and `bargain: false` turns all three off at once, so it is the
all-off control (the control of cohort `0250ba3c` predates the `asks` switch and
still asked two for one). `accept_asks: false` makes the seat refuse any offer or
counter that asks it for more cards than it receives, a stand-in for a stubborn
partner; it changes only what the seat accepts, never what it proposes, and it
does not depend on `bargain`.

The arms were measured on 2026-09-11 in the engine arena at depth 2 with an
ETA and a fast builder, the changed seat in slot 0 and the default in slot 1
(the research notebook's `reports/bargaining-arms.mdx` has the run IDs). That
design carries a seating term: two identical defaults in slots 0 and 1 differ
by +0.176 wins per game (interval +0.083 to +0.268) on boards 0 to 63, because
slot 1 always sits one seat after slot 0 and the seat directly before another
acts first after three of the four rolls in a round (the research notebook's
`reports/arena-seating.mdx`: the term survives `bargain: false`, `propose:
false`, `counters: false`, `asks: false`, and identical builders, and
disappears with a builder between the two searches). Against that null, `threat_pricing: false`
changes nothing (−0.008), `counters: false` costs −0.199, `asks: false` costs
−0.156, `bargain: false` costs −0.297, and the seat with `accept_asks: false`
is +0.059 better than an identical default would be, so refusing asks costs
the refuser nothing. A swapped pair on boards 64 to 127 confirmed counters at
−0.096 (interval −0.156 to −0.035) for the seat without them. The default
keeps every switch on. Any two-search cohort in this arena needs a null arm or
a swapped pair, and the recipient order of a multi-seat offer in
`crates/arena/src/table.rs`, which always starts after the offerer, should
rotate.

### Build prediction

`predict` holds three switches, each off by default so a default search is
unchanged bit for bit. One function, `v2/predict.rs::next_build`, predicts a
seat's next build from a hand and the public board: it runs the builders' own
greedy target choice on that hand (a city on the best own settlement, the best
reachable expansion site with its first road, or a development card), reports
the next piece the line completes, the site that line ends up taking, and how
many turns the piece and the whole line are from affordable at that seat's
production. Inside the search the hand is the seat's sampled hand, so the
prediction is an expectation over the searcher's belief.

- `weights.denial` (default 0) adds the leaf term above: a site the root seat
  could still take or block, which an opponent's predicted line is heading for,
  discounts the leaf by the site's production value and the opponent's
  time-to-afford. Taking or blocking the site removes the discount.
- `predict.opponents` makes the lookahead's opponent model execute the
  predicted build: complete the piece when affordable, otherwise trade toward it
  with surplus cards or end the turn to save, instead of the builder's greedy
  step-by-step policy. Setup, discards, robber, and knight plays keep the
  builder's behaviour.
- `predict.threat` makes the bargaining `space` threat key on the other seat's
  predicted target site instead of scanning every one of this seat's best three
  sites they can reach: only a seat whose predicted line ends on one of those
  sites counts, weighted by that site's share of the frontier.

The engine arena measures the predictor when run with `--predict-diag SEAT`: at
every other seat's first main-phase decision of a turn, the table records that
seat's prediction from the observer seat's public knowledge (the same
`ResourceKnowledge` estimator a seated policy uses) and compares it with the
first build the acting seat completes in the same turn; the game record carries
per-seat match and build counts. Every game record also counts each seat's
`sites_lost`: settlements other seats built on a vertex this seat could reach
within three roads just before the build.
### Robber targeting

Robber placements are shortlisted by a static score and then valued by the
lookahead like any other action. The score adds, per hex, the blocked pips of
every rival building (cities double, weighted by the blocked seat's public
points), and per victim a bonus for hand size and public points; the four
highest-scoring placements enter the search. Three switches change that score,
each testing one hypothesis against the original behaviour:

| Setting | Default | Meaning |
| --- | --- | --- |
| `robber.leader` | false | The hex score counts the points leader's blocked pips in full and every other seat's at a tenth, so the shortlist is led by the leader's own best hex |
| `robber.need` | false | The victim bonus replaces hand size with `2.8 ×` the expected share of the resource this seat most needs, from this seat's resource knowledge (the seat's largest missing-card gap toward the builds it is closest to affording) |
| `robber.threat` | false | Each seat's blocked pips are weighted by `1 + 2 ×` their threat, and a victim whose threat is below 0.5 is never robbed while any placement robbing a threatening seat exists |

The need bonus reads the expected-hand estimate taken at the root of the
decision; a hand that already affords every build keeps the hand-size bonus.

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

### Transposition tables

Positions are keyed by the root seat's information set: everything its
observation would show (board, roads, own hand and cards, bank, deck count,
robber, awards, phase, turn, and the other seats' public counts), never a
hidden hand or the random cursor. The key is a Zobrist-style combination of
per-field mixes in two independent words; the second word verifies a hit so a
table collision cannot return another position's value. Indistinguishable
positions are merged at every branching point (dice outcomes, purchases,
thefts, sampled hands, and the arrival positions after the opponents' round),
in first-appearance order so that sums do not depend on the key function.

Three tables are kept: turn plans (the static best line of a position, which
does not depend on the depth), lookahead values (keyed by position, level, and
depth cap), and the arrival positions of an opponents' round (keyed by
position and level). `cache` decides their lifetime. With `none` they are
rebuilt for every depth pass, as the original search did. With `decision`
they are shared across the depth passes of one decision, so iterative
deepening replans a turn and replays an opponents' round once; this
reproduces `none` bit for bit whenever no node or time budget binds, and
completes more of the tree when one does. With `turn` they persist across the
decisions of the same turn, so after a move the next decision starts from a
subtree the previous one explored; values computed under the earlier
decisions' sampled hands are reused, which is an equally valid estimate but
not the same bits. Reports count `cache_hits`, `cached_transitions` (the
transitions the hits saved), and `cache_entries`.

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
the winner, per-seat points, builds, development cards bought, knights played, largest-army
awards gained, the turn of the first city, trades, discards, decision timing, and search
effort; `--trace` adds each search seat's top candidates per decision. Each seat
record also carries `hand_at_roll_sum` and `hand_at_roll_count`, the seat's hand
sizes summed over the game's rolls before each roll's production or discards, so
the mean hand size at the roll is the sum divided by the count. Player
trades are resolved by asking each recipient in turn, starting from a seat that
rotates with the seed and version so that no slot is always asked first; the
first acceptance commits. The rotation did not remove the seating term between
two searches in slots 0 and 1 (a null pair still differs by +0.180 wins per
game), so two-search contrasts are measured as swapped pairs. This arena is a development and diagnostic tier. It is not the
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
