//! cache-aware breakeven（DESIGN §5）。整数交叉相乘，无除法精度损失；
//! v0.1 用于 failover 与 quota spill 的切换判定，不用于自动选模。

use serde::Serialize;

use crate::cost::{NanoUsd, Price};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakevenParams {
    pub enabled: bool,
    pub min_remaining_turns: u32,
    /// safety_factor × 100（1.2 → 120）。config 解析时整数化（§12.5）。
    pub safety_factor_pct: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchCandidate {
    /// 换模型要按 miss 价重算的前缀量。
    pub prefix_tokens: u64,
    /// 预估后续每轮输入 token。
    pub tokens_per_turn: u64,
    /// 预估剩余轮次（无历史 → 0）。
    pub remaining_turns: u32,
    /// 保持现状的下一轮单价 = 现模型 input_hit（GAP-Q7 默认）。
    pub p_stay_hit: Price,
    /// 新模型 input_miss（换过去必然 miss 整段前缀）。
    pub p_new_miss: Price,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StayReason {
    Disabled,
    RemainingTurnsZero,
    BelowMinRemainingTurns,
    NotPaying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchVerdict {
    Switch {
        gain: NanoUsd,
        cost: NanoUsd,
    },
    Stay {
        reason: StayReason,
        gain: NanoUsd,
        cost: NanoUsd,
    },
}

const fn tokens_nano(tokens: u128, price: Price) -> u64 {
    let q = tokens * price.0 as u128 / 1000;
    if q > u64::MAX as u128 {
        u64::MAX
    } else {
        q as u64
    }
}

/// `gain = remaining_turns × tokens_per_turn × (p_stay − p_new)/1000`（i128 中间量，负值截 0）；
/// `cost = prefix × p_new/1000`；
/// `Switch ⟺ gain × 100 > safety_factor_pct × cost`（严格大于，交叉相乘）。
pub fn decide_switch(p: &BreakevenParams, c: &SwitchCandidate) -> SwitchVerdict {
    if !p.enabled {
        return SwitchVerdict::Stay {
            reason: StayReason::Disabled,
            gain: NanoUsd::ZERO,
            cost: NanoUsd::ZERO,
        };
    }
    let cost = NanoUsd(tokens_nano(c.prefix_tokens as u128, c.p_new_miss));
    if c.remaining_turns == 0 {
        return SwitchVerdict::Stay {
            reason: StayReason::RemainingTurnsZero,
            gain: NanoUsd::ZERO,
            cost,
        };
    }
    if c.remaining_turns < p.min_remaining_turns {
        return SwitchVerdict::Stay {
            reason: StayReason::BelowMinRemainingTurns,
            gain: NanoUsd::ZERO,
            cost,
        };
    }
    let diff = c.p_stay_hit.0 as i128 - c.p_new_miss.0 as i128;
    if diff <= 0 {
        return SwitchVerdict::Stay {
            reason: StayReason::NotPaying,
            gain: NanoUsd::ZERO,
            cost,
        };
    }
    let gain = NanoUsd(tokens_nano(
        c.remaining_turns as u128 * c.tokens_per_turn as u128 * diff as u128,
        Price(1),
    ));
    let lhs = gain.0 as u128 * 100;
    let rhs = p.safety_factor_pct as u128 * cost.0 as u128;
    if lhs > rhs {
        SwitchVerdict::Switch { gain, cost }
    } else {
        SwitchVerdict::Stay {
            reason: StayReason::NotPaying,
            gain,
            cost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BreakevenParams {
        BreakevenParams {
            enabled: true,
            min_remaining_turns: 3,
            safety_factor_pct: 120,
        }
    }

    fn candidate() -> SwitchCandidate {
        SwitchCandidate {
            prefix_tokens: 10_000,
            tokens_per_turn: 1_000,
            remaining_turns: 10,
            p_stay_hit: Price(220_000),
            p_new_miss: Price(100_000),
        }
    }

    #[test]
    fn disabled_stays() {
        let mut p = params();
        p.enabled = false;
        assert_eq!(
            decide_switch(&p, &candidate()),
            SwitchVerdict::Stay {
                reason: StayReason::Disabled,
                gain: NanoUsd::ZERO,
                cost: NanoUsd::ZERO
            }
        );
    }

    #[test]
    fn zero_remaining_turns_stays() {
        let mut c = candidate();
        c.remaining_turns = 0;
        assert_eq!(
            decide_switch(&params(), &c),
            SwitchVerdict::Stay {
                reason: StayReason::RemainingTurnsZero,
                gain: NanoUsd::ZERO,
                cost: NanoUsd(1_000_000)
            }
        );
    }

    #[test]
    fn below_min_remaining_turns_stays_even_if_formula_pays() {
        let mut c = candidate();
        c.remaining_turns = 2; // < min 3
        assert_eq!(
            decide_switch(&params(), &c),
            SwitchVerdict::Stay {
                reason: StayReason::BelowMinRemainingTurns,
                gain: NanoUsd::ZERO,
                cost: NanoUsd(1_000_000)
            }
        );
    }

    #[test]
    fn zero_prefix_switches() {
        let mut c = candidate();
        c.prefix_tokens = 0;
        assert_eq!(
            decide_switch(&params(), &c),
            SwitchVerdict::Switch {
                gain: NanoUsd(1_200_000),
                cost: NanoUsd::ZERO
            }
        );
    }

    #[test]
    fn exact_equality_stays_strictly_greater_required() {
        // gain = 10×1000×(220_000−100_000)/1000 = 1_200_000
        // cost = 10_000×100_000/1000 = 1_000_000；gain×100 == 120×cost == 120_000_000
        assert_eq!(
            decide_switch(&params(), &candidate()),
            SwitchVerdict::Stay {
                reason: StayReason::NotPaying,
                gain: NanoUsd(1_200_000),
                cost: NanoUsd(1_000_000)
            }
        );
    }

    #[test]
    fn strictly_above_threshold_switches() {
        let mut c = candidate();
        c.prefix_tokens = 9_999; // cost 999_900 → gain×100 = 120_000_000 > 119_988_000
        assert_eq!(
            decide_switch(&params(), &c),
            SwitchVerdict::Switch {
                gain: NanoUsd(1_200_000),
                cost: NanoUsd(999_900)
            }
        );
    }

    #[test]
    fn stay_price_not_above_new_price_stays() {
        let mut c = candidate();
        c.p_stay_hit = c.p_new_miss; // 不赚
        assert_eq!(
            decide_switch(&params(), &c),
            SwitchVerdict::Stay {
                reason: StayReason::NotPaying,
                gain: NanoUsd::ZERO,
                cost: NanoUsd(1_000_000)
            }
        );
        c.p_stay_hit = Price(1);
        assert_eq!(
            decide_switch(&params(), &c),
            SwitchVerdict::Stay {
                reason: StayReason::NotPaying,
                gain: NanoUsd::ZERO,
                cost: NanoUsd(1_000_000)
            }
        );
    }

    #[test]
    fn at_min_remaining_turns_is_eligible() {
        let mut c = candidate();
        c.remaining_turns = 3; // == min：可参与判定
        match decide_switch(&params(), &c) {
            SwitchVerdict::Stay {
                reason: StayReason::BelowMinRemainingTurns,
                ..
            } => {
                panic!("remaining_turns == min 应参与算式判定")
            }
            _ => {}
        }
    }
}
