# Rule contract

The reference is the [CATAN base-game rules and almanac](https://www.catan.com/sites/default/files/2021-06/catan_base_rules_2020_200707.pdf), supplemented by the [official base-game FAQ](https://www.catan.com/faq/basegame). The implementation uses the combined trade/build phase described by the rules' experienced-player option. JSettlers' base-game checks and robot strategy descriptions were read as a correctness/design reference; its Java implementation was not copied or translated.

## Implemented base rules

| Area | Contract | Tests |
|---|---|---|
| Board | 19 land hexes, 54 intersections, 72 edges, 9 ports; terrain inventory 3 brick/ore, 4 lumber/wool/grain, 1 desert; standard spiral number tokens, no adjacent 6/8 | `engine/tests/board.rs` |
| Setup | uniform starting seat, cyclic forward/reverse placement; road touches just-placed settlement; initial resources from second settlement only | `setup_building.rs` |
| Construction | distance rule; connected roads; opposing buildings block road transit; 15 roads, 5 settlements, 4 cities; city upgrades return a settlement piece | `setup_building.rs` |
| Production | two fair dice; settlements collect one, cities two; robber blocks its hex; per-resource bank supply is 19 | `economy.rs` |
| Shortage | multiple affected recipients receive none of that resource; a single recipient receives what remains | `economy.rs` |
| Seven | hands above threshold discard half rounded down; discards may arrive in any order; then move robber and steal uniformly from eligible victim cards | `robber_cards.rs` |
| Trade | current player must participate; no gifts or like-resource swaps; bank 4:1, generic harbor 3:1, specific harbor 2:1; choose best available ratio | `economy.rs`, server trade tests |
| Development deck | 14 knights, 2 road building, 2 year of plenty, 2 monopoly, 5 victory points; draw probabilities match a shuffled deck | `robber_cards.rs`, `replay_timeout.rs` |
| Card timing | at most one non-VP development card per turn; purchases mature after the buying turn; cards may be played before rolling | `robber_cards.rs` |
| Card effects | knight moves robber without discards; monopoly takes all chosen resources from opponents; plenty takes two available cards (or remaining bank supply); road building places up to two free roads, with the second optional | `robber_cards.rs` |
| Awards | longest edge-simple road trail of at least 5; opposing buildings split roads; longest holder keeps a tied maximum; unique challenger takes it; largest army at least 3 knights, holder retains tie | `longest_road.rs`, `robber_cards.rs` |
| Victory | settlements 1, cities 2, each award 2, hidden VP cards 1; victory only on the winner's own turn, including before rolling and freshly bought VP cards | `longest_road.rs`, `robber_cards.rs` |
| Information | exact own hand only; opponents' card counts and public pieces/points; hidden development/steal/discard types remain private | `replay_timeout.rs`, server projection tests |

Exact bank-stack counts are excluded from observations, consistent with the FAQ's restriction on counting supply stacks. Resource availability is visible; supply failures are validated by the server.

In deterministic mode two games on one seed face identical dice by roll number, identical theft positions by theft number, and one fixed deck order, whatever the policies do in between; an explicit search branch that takes a different card leaves the deck tape consistent. Ordinary games consume one shared stream, so every random event shifts the ones after it. An agent-only research table may pin its seed index at creation; the seed bytes never appear in any view or event.

Initial roll-offs are represented by uniform seeded starter selection. The terrain arrangement and port types are shuffled; legal number tokens follow a fixed spiral orientation. Bank production, stealing, card draws, and setup all conserve finite inventories. Development draws sample remaining counts, which has the same distribution as drawing from a uniformly shuffled deck.

An opponent may terminate a road at an opposing settlement but cannot continue through it. A cycle counts all its edges; branches cannot reuse an edge. When a holder's road is interrupted, recomputation handles transfers and unclaimed ties. All action errors leave the input state unchanged.

## Settings and research variants

| Setting | Default | Accepted values |
|---|---|---|
| `players` | 4 | 2–4 |
| `points_to_win` | 10 | 3–20 |
| `discard_limit` | 7 | 0–95; discard when strictly above the threshold |
| `turn_seconds` | 120 | 1–86400 |
| `deterministic` | false | Deterministic mode: the k-th roll, k-th theft, and k-th development draw are read from seed-derived tapes indexed by count instead of one shared stream. The board and opening seat are the same in both modes for a given seed. |

Three/four seats use the base game. Two seats are a research variant using the same board, inventory, trade rules, and turn sequence. No neutral pieces, trade tokens, or additional two-player expansion rules are enabled. Expansion boards, ships, Cities & Knights, and 5–6 player special building phases are outside this base-game server.

## Timeouts

Turn time is one deadline across roll, trade/build, discard, and development-card resolution. Setup gets one deadline per settlement-plus-road pair. Ordinary actions do not reset it. An active/pending participant's first disconnection in a turn can extend the deadline to `disconnect_detected_at + turn_seconds`; subsequent disconnects in that turn do not grant repeated extensions. Disconnected players' future turns still get their full normal allotment.

When a deadline expires:

1. Setup picks the lowest-ID legal settlement and then a legal adjacent road.
2. If rolling is pending, the server rolls using the saved random stream.
3. Pending discards remove cards from the largest stack first, with resource-ID tie breaking.
4. Pending robber resolution moves it to the desert without stealing, including when already on the desert.
5. Pending free roads choose legal placements until the card is resolved.
6. The server ends the turn, unless the game has ended.

This is an explicit unattended-play rule. The archive records a `timeout` input and all resulting public effects; replay makes the same choices. Turn deadlines continue across server outages, and expired turns advance after recovery. An archive/storage outage pauses acceptance rather than acknowledging undurable actions.
