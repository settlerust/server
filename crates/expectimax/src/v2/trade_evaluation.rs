//! Trade evaluation compares accepting, countering, declining, and waiting under one sampled market.
use super::{
    RootValue, Row, SearchConfig, Weights, actor, bargain, leaf, normalize,
    search::{Search, World},
};
use crate::{search::TradeAnalysis, trades};
use settlers_engine::*;
use settlers_protocol::{GameView, OfferStatus, knowledge::ResourceKnowledge};
use std::collections::BTreeMap;
pub(super) struct Market<'a> {
    pub config: &'a SearchConfig,
    pub view: &'a GameView,
    pub o: &'a Observation,
    pub knowledge: &'a ResourceKnowledge,
    pub worlds: &'a [World],
    pub decisions: &'a [bargain::Decision],
    pub seat: u8,
    pub weights: Weights,
    pub threats: [bargain::Threat; 4],
    pub partner_factor: [f64; 4],
}

pub(super) fn evaluate(
    market: &Market<'_>,
    search: &mut Search<'_>,
    rows: &mut Vec<Row>,
    best_plain: Option<f64>,
    wait_value: Option<&leaf::Terms>,
    actions_empty: bool,
) -> Result<(), RuleError> {
    let Market {
        config,
        view,
        o,
        knowledge,
        worlds,
        decisions,
        seat,
        weights,
        threats,
        partner_factor,
    } = *market;
    let root = &worlds[0].state;
    let top_threat = bargain::top_threat(&threats, o.settings.players);
    let contested = top_threat.and_then(|(q, _)| bargain::contested_site(root, seat, q));
    let margin = config.inclinations.trade_margin();
    let aggression = 0.5 + config.inclinations.clamped().aggression;
    let threat_pricing = config.bargain && config.threat_pricing;
    let fallback_plain = best_plain.or(wait_value.map(|v| v.total()));
    // Margin and cost of dealing with a seat: the plain margin plus a threat premium, and a
    // share of what the partner gains scaled by their threat and this seat's aggression.
    let threat_scale = if threat_pricing { 1.0 } else { 0.0 };
    let margin_for = |q: u8| margin + threats[q as usize].total * 30.0 * threat_scale;
    let threat_cost =
        |q: u8, gain: f64| threats[q as usize].total * aggression * 0.5 * gain * threat_scale;
    let card_value = weights.card_value;
    let point = weights.point;
    // Accept values per offer id, for the counters' fallback and the decline reasons.
    let mut accept_totals: BTreeMap<String, f64> = BTreeMap::new();
    let mut counter_best: BTreeMap<String, f64> = BTreeMap::new();
    // Per-partner proposal analyses, for the combined offers to every safe partner.
    let mut per_partner: Vec<PartnerOffer> = Vec::new();
    if !search.aborted {
        for d in decisions {
            if search.aborted {
                break;
            }
            let Action::Trade {
                other: partner,
                give,
                receive,
            } = d.action
            else {
                continue;
            };
            match &d.kind {
                bargain::Kind::Offer { partner: _, ask } => {
                    let Some(best_plain) = best_plain else {
                        continue;
                    };
                    let mut accepted = Vec::new();
                    let mut affordable = 0.0;
                    let mut unlocked = Vec::new();
                    let mut gain = 0.0;
                    for w in worlds {
                        let prob = trades::acceptance(&w.state, seat, d.action)
                            * (0.5 + config.inclinations.trade)
                            * partner_factor[partner as usize]
                            * if *ask { 0.85 } else { 1.0 };
                        if let Ok(after) = apply(&w.state, seat, d.action) {
                            affordable += w.weight;
                            for a in trades::unlocked(&w.state, &after, seat) {
                                if !unlocked.contains(&a) {
                                    unlocked.push(a);
                                }
                            }
                            gain += w.weight
                                * bargain::partner_gain(
                                    &w.state, partner, give, receive, card_value, point,
                                );
                            accepted.push(World {
                                state: after,
                                weight: w.weight * prob.min(1.0),
                            });
                        }
                    }
                    let probability: f64 = accepted.iter().map(|w| w.weight).sum();
                    if probability <= 1e-9 {
                        continue;
                    }
                    if affordable > 0.0 {
                        gain /= affordable;
                    }
                    let accepted_value = search.plan(accepted, 1)?.0;
                    if search.aborted {
                        break;
                    }
                    let accepted_total = accepted_value.total();
                    let cost = threat_cost(partner, gain);
                    let mut value = leaf::Terms::default();
                    value.0.insert(
                        "expected",
                        best_plain + probability * (accepted_total - best_plain),
                    );
                    value.0.insert("trade_margin", -margin_for(partner));
                    value.0.insert("partner_threat", -probability * cost);
                    per_partner.push((
                        give,
                        receive,
                        partner,
                        probability,
                        accepted_total,
                        cost,
                        unlocked.clone(),
                    ));
                    let block = blocks(&unlocked, contested)
                        .then_some(top_threat.map(|t| t.0))
                        .flatten();
                    let text = bargain::offer_text(
                        view,
                        &[partner],
                        give,
                        receive,
                        &unlocked,
                        block,
                        *ask,
                    );
                    let command = settlers_protocol::Command::OfferTrade {
                        to: vec![partner],
                        give,
                        receive,
                        expires_in_ms: None,
                        message: Some(settlers_protocol::Communication {
                            intent: settlers_protocol::Intent::RequestTrade,
                            resources: Some(receive),
                            text: Some(text),
                        }),
                    };
                    rows.push(Row {
                        command: Some(command),
                        action: d.action,
                        root: RootValue {
                            value,
                            trade: Some(TradeAnalysis {
                                acceptance_probability: probability,
                                acceptance_when_affordable: if affordable > 0.0 {
                                    probability / affordable
                                } else {
                                    0.0
                                },
                                accepted_value: Some(accepted_total),
                                rejected_value: best_plain,
                                partner_affordability: affordable,
                                unlocked_actions: unlocked,
                            }),
                        },
                        note: Some(format!(
                            "{}: threat {:.2}, modeled acceptance {:.2}, partner gain {:.0}",
                            bargain::name(view, partner),
                            threats[partner as usize].total,
                            probability,
                            gain
                        )),
                        speech: None,
                    });
                }
                bargain::Kind::Accept { offer } => {
                    // Only worlds where the partner can pay.
                    let feasible: Vec<World> = worlds
                        .iter()
                        .filter_map(|w| {
                            apply(&w.state, seat, d.action).ok().map(|state| World {
                                state,
                                weight: w.weight,
                            })
                        })
                        .collect();
                    if feasible.is_empty() {
                        continue;
                    }
                    // The partner gets what this seat gives and pays what it receives.
                    let gain = worlds
                        .iter()
                        .map(|w| {
                            w.weight
                                * bargain::partner_gain(
                                    &w.state, partner, give, receive, card_value, point,
                                )
                        })
                        .sum::<f64>();
                    let mut value = if actor(&feasible[0].state) == seat
                        && feasible[0].state.phase == Phase::Main
                    {
                        search.plan(feasible, 1)?.0
                    } else {
                        search.round(&normalize(feasible), 0)?
                    };
                    value.0.insert("trade_margin", -margin_for(partner));
                    value
                        .0
                        .insert("partner_threat", -threat_cost(partner, gain));
                    accept_totals.insert(offer.id.clone(), value.total());
                    rows.push(Row {
                        command: Some(d.command.clone()),
                        action: d.action,
                        root: RootValue { value, trade: None },
                        note: Some(format!(
                            "{}: threat {:.2}, partner gain {:.0}",
                            bargain::name(view, partner),
                            threats[partner as usize].total,
                            gain
                        )),
                        speech: Some(bargain::accept_text(view, offer)),
                    });
                }
                bargain::Kind::Counter { offer, acceptance } => {
                    let Some(fallback_plain) = fallback_plain else {
                        continue;
                    };
                    let mut accepted = Vec::new();
                    let mut gain = 0.0;
                    let mut affordable = 0.0;
                    for w in worlds {
                        if let Ok(after) = apply(&w.state, seat, d.action) {
                            affordable += w.weight;
                            gain += w.weight
                                * bargain::partner_gain(
                                    &w.state, partner, give, receive, card_value, point,
                                );
                            accepted.push(World {
                                state: after,
                                weight: w.weight,
                            });
                        }
                    }
                    if accepted.is_empty() {
                        continue;
                    }
                    gain /= affordable;
                    let probability =
                        (acceptance * partner_factor[partner as usize] * affordable).min(1.0);
                    let accepted_value = search.round(&normalize(accepted), 0)?;
                    if search.aborted {
                        break;
                    }
                    let accepted_total = accepted_value.total() - threat_cost(partner, gain);
                    // If the counter is declined the original offer usually stays open.
                    let original = accept_totals.get(&offer.id).copied();
                    let fallback = match original {
                        Some(accept) => fallback_plain + 0.6 * (accept - fallback_plain).max(0.0),
                        None => fallback_plain,
                    };
                    let mut value = leaf::Terms::default();
                    value.0.insert(
                        "expected",
                        probability * accepted_total + (1.0 - probability) * fallback,
                    );
                    value.0.insert("trade_margin", -margin_for(partner));
                    let total = value.total();
                    counter_best
                        .entry(offer.id.clone())
                        .and_modify(|b| *b = b.max(total))
                        .or_insert(total);
                    rows.push(Row {
                        command: Some(d.command.clone()),
                        action: d.action,
                        root: RootValue {
                            value,
                            trade: Some(TradeAnalysis {
                                acceptance_probability: probability,
                                acceptance_when_affordable: *acceptance,
                                accepted_value: Some(accepted_total),
                                rejected_value: fallback,
                                partner_affordability: affordable,
                                unlocked_actions: vec![],
                            }),
                        },
                        note: Some(format!(
                            "counter to {}: estimated eagerness {:.2}, threat {:.2}",
                            bargain::name(view, partner),
                            acceptance,
                            threats[partner as usize].total
                        )),
                        speech: None,
                    });
                }
                bargain::Kind::Reject { .. } => {}
            }
        }
    }
    // One offer to every safe partner at once, when several were worth asking.
    if !search.aborted
        && let Some(best_plain) = best_plain
    {
        let mut done: Vec<(Resources, Resources)> = Vec::new();
        for &(give, receive, _, _, _, _, _) in &per_partner {
            if done.contains(&(give, receive)) {
                continue;
            }
            done.push((give, receive));
            let group: Vec<_> = per_partner
                .iter()
                .filter(|x| x.0 == give && x.1 == receive && threats[x.2 as usize].total < 0.5)
                .collect();
            if group.len() < 2 {
                continue;
            }
            let miss: f64 = group.iter().map(|x| 1.0 - x.3).product();
            let probability = 1.0 - miss;
            let mass: f64 = group.iter().map(|x| x.3).sum();
            if probability <= 1e-9 || mass <= 1e-9 {
                continue;
            }
            let accepted_total: f64 = group.iter().map(|x| x.3 * x.4).sum::<f64>() / mass;
            let cost: f64 = group.iter().map(|x| x.3 * x.5).sum::<f64>() / mass;
            let to: Vec<u8> = group.iter().map(|x| x.2).collect();
            let margin_group = group.iter().map(|x| margin_for(x.2)).fold(0.0, f64::max);
            let mut unlocked: Vec<Action> = Vec::new();
            for x in &group {
                for a in &x.6 {
                    if !unlocked.contains(a) {
                        unlocked.push(*a);
                    }
                }
            }
            let mut value = leaf::Terms::default();
            value.0.insert(
                "expected",
                best_plain + probability * (accepted_total - best_plain),
            );
            value.0.insert("trade_margin", -margin_group);
            value.0.insert("partner_threat", -probability * cost);
            let block = blocks(&unlocked, contested)
                .then_some(top_threat.map(|t| t.0))
                .flatten();
            let text = bargain::offer_text(view, &to, give, receive, &unlocked, block, false);
            rows.push(Row {
                command: Some(settlers_protocol::Command::OfferTrade {
                    to: to.clone(),
                    give,
                    receive,
                    expires_in_ms: None,
                    message: Some(settlers_protocol::Communication {
                        intent: settlers_protocol::Intent::RequestTrade,
                        resources: Some(receive),
                        text: Some(text),
                    }),
                }),
                action: Action::Trade {
                    other: to[0],
                    give,
                    receive,
                },
                root: RootValue {
                    value,
                    trade: Some(TradeAnalysis {
                        acceptance_probability: probability,
                        acceptance_when_affordable: probability,
                        accepted_value: Some(accepted_total),
                        rejected_value: best_plain,
                        partner_affordability: 1.0,
                        unlocked_actions: unlocked,
                    }),
                },
                note: Some(format!(
                    "open to {}",
                    to.iter()
                        .map(|&q| bargain::name(view, q))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                speech: None,
            });
        }
    }
    // Explicit declines: an open offer to this seat that neither accepting nor countering
    // beats waiting on gets a reasoned refusal instead of silence.
    if !search.aborted
        && let Some(wait) = wait_value
    {
        let my_open = view
            .offers
            .iter()
            .any(|offer| offer.from == seat && offer.status == OfferStatus::Open);
        let mut declined_any = false;
        for offer in &view.offers {
            if offer.status != OfferStatus::Open
                || !offer.to.contains(&seat)
                || offer.from == seat
                || offer.declined.contains(&seat)
                || my_open
            {
                continue;
            }
            let wait_total = wait.total();
            let accept = accept_totals.get(&offer.id).copied();
            let counter = counter_best.get(&offer.id).copied();
            if accept.is_some_and(|v| v > wait_total) || counter.is_some_and(|v| v > wait_total) {
                continue;
            }
            let reason = decline_reason(
                root, seat, knowledge, &threats, offer, accept, counter, &weights,
            );
            let mut value = wait.clone();
            value.0.insert("decline", 1e-6);
            rows.push(Row {
                command: Some(settlers_protocol::Command::RejectTrade {
                    offer_id: offer.id.clone(),
                }),
                action: Action::EndTurn,
                root: RootValue { value, trade: None },
                note: Some(format!("decline: {reason:?}")),
                speech: Some(bargain::decline_text(
                    view,
                    offer,
                    &reason,
                    o.turn,
                    o.settings.players,
                )),
            });
            declined_any = true;
        }
        if !declined_any && actions_empty && rows.iter().all(|r| r.command.is_some()) {
            // Waiting is the alternative to accepting an off-turn offer.
            rows.push(Row::plain(
                None,
                Action::EndTurn,
                RootValue {
                    value: wait.clone(),
                    trade: None,
                },
            ));
        }
    }
    Ok(())
}
type PartnerOffer = (Resources, Resources, u8, f64, f64, f64, Vec<Action>);
/// Whether one of the unlocked builds takes or touches the contested site.
fn blocks(unlocked: &[Action], contested: Option<u8>) -> bool {
    let Some(v) = contested else {
        return false;
    };
    unlocked.iter().any(|a| match a {
        Action::BuildSettlement { vertex } => *vertex == v,
        Action::BuildRoad { edge } => topology().edges[*edge as usize].contains(&v),
        _ => false,
    })
}
/// Why this seat declines `offer`, from the most to the least specific ground.
#[allow(clippy::too_many_arguments)]
fn decline_reason(
    s: &State,
    seat: u8,
    k: &ResourceKnowledge,
    threats: &[bargain::Threat; 4],
    offer: &settlers_protocol::TradeOffer,
    accept: Option<f64>,
    counter: Option<f64>,
    w: &Weights,
) -> bargain::Reason {
    let threat = threats[offer.from as usize];
    if threat.race >= 0.6 {
        return bargain::Reason::Leader {
            points: public_points(s, offer.from),
        };
    }
    if threat.total >= 0.3
        && let Some(build) = bargain::public_unlock(k, offer.from, offer.receive, offer.give)
    {
        return bargain::Reason::Unlock { build };
    }
    if threat.space >= 0.5 {
        return bargain::Reason::Space;
    }
    if let (Some(a), Some(c)) = (accept, counter)
        && c > a
    {
        return bargain::Reason::Thin;
    }
    // Keeping the asked cards for the seat's own next build.
    let own = s.players[seat as usize].resources;
    let rates = leaf::rates(s, seat);
    let _ = w;
    let asked = Resource::ALL
        .into_iter()
        .filter(|&r| offer.receive.0[r as usize] > 0)
        .max_by_key(|&r| offer.receive.0[r as usize]);
    if let Some(r) = asked {
        for (cost, build) in [
            (Resources::CITY, "a city"),
            (Resources::SETTLEMENT, "a settlement"),
            (Resources::ROAD, "a road"),
            (Resources::DEVELOPMENT, "a development card"),
        ] {
            let after = own.0[r as usize].saturating_sub(offer.receive.0[r as usize]);
            if cost.0[r as usize] > 0
                && own.0[r as usize] >= cost.0[r as usize]
                && after < cost.0[r as usize]
                && rates[r as usize] < 0.12
            {
                return bargain::Reason::Need {
                    resource: r,
                    build: Some(build),
                };
            }
        }
    }
    bargain::Reason::Plain
}
