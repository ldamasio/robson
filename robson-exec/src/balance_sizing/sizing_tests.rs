use robson_domain::Price;
use rust_decimal_macros::dec;

use super::*;
use crate::{
    balance_costs::{sizing_terms, CostPolicy},
    balance_sizing::{arithmetic::TRACE, test_support::fixture},
};
fn calculate(i: &SizingInputs) -> Result<SizingCalculation, SizingError> {
    calculate_balance_candidate(i)
}
#[test]
fn full_graph_and_canonical_quote_are_unqualified_and_exact() {
    TRACE.with_borrow_mut(Vec::clear);
    let i = fixture();
    let out = calculate(&i).unwrap();
    assert_eq!(out.attempts(), 2);
    assert_eq!(*out.amounts(), SizedAmounts {
        protection: dec!(0.3),
        gross_entry: dec!(0.3),
        adverse_fill: dec!(50),
        loss_per_unit: dec!(50),
        remainder: dec!(0.1),
        charge: dec!(20),
        gross_notional: dec!(30)
    });
    let trace = TRACE.with_borrow(|t| t.clone());
    let mut frames: Vec<(&str, u64)> = vec![];
    let mut calls = 0;
    let mut max = 0;
    let mut exports = 0;
    let mut divisions = 0;
    let mut imports = 0;
    for (kind, v) in trace {
        match kind {
            "prepare" | "candidate" => {
                if !frames.is_empty() {
                    frames.last_mut().unwrap().1 = calls;
                }
                frames.push((kind, 0));
                calls = 0;
            },
            "call" => calls += v,
            "bits" => max = max.max(v),
            "export_div" => exports += v,
            "integer_div" => divisions += v,
            "import" => imports += v,
            _ => panic!("unknown trace"),
        }
    }
    frames.last_mut().unwrap().1 = calls;
    assert_eq!(frames[0], ("prepare", 57));
    assert_eq!(frames.last(), Some(&("candidate", 37)));
    assert!(frames[1..].iter().all(|(_, c)| *c <= 37));
    assert!(max <= 872);
    assert!(exports <= 26 * 29);
    assert!(divisions <= 774);
    assert!(imports <= 128, "imports={imports}");
    assert_eq!(out.inputs().binding.account.symbol().quote(), "USDC");
    let mut changed = i.clone();
    changed.cash.free = D::ZERO;
    assert_eq!(out.inputs().cash.free, dec!(2000));
    assert!(calculate(&changed).is_err());
}
#[test]
fn labeled_chart_golden_priced_candidates() {
    let mut i = fixture();
    i.price.value = dec!(73284.01);
    i.chart.support = dec!(65955.60);
    i.chart.plan.basis = Price::new(i.chart.support).unwrap();
    i.chart.plan.trigger = i.chart.plan.basis;
    i.chart.plan.cap_basis_distance = Some(dec!(7328.41));
    i.chart.plan.executable_span = Some(dec!(7328.41));
    i.chart.plan.tick_size = Some(dec!(0.01));
    i.rules.price.tick = dec!(0.01);
    i.rules.price.max = dec!(1000000);
    i.rules.lot.step = dec!(0.00001);
    i.rules.lot.min = dec!(0.00001);
    i.rules.market_lot = None;
    i.rules.notional.min = dec!(5);
    i.rules.notional.apply_min = false;
    i.rules.notional.apply_max = false;
    i.policy = CostPolicy::parse("10", None, None).unwrap();
    i.terms = sizing_terms(&i.binding.account, "10", "10");
    i.basis.amount = dec!(1805);
    let mut p = prepare(&i).unwrap();
    assert_eq!(p.af, dec!(65889.6444));
    assert_eq!(p.l, dec!(7533.5392544));
    for (s, q, rh, a) in [
        (dec!(0.0001), dec!(0.00011), dec!(0.00008), dec!(6.752094405888)),
        (dec!(0.00008), dec!(0.00009), dec!(0.00008), dec!(6.6014236208)),
        (dec!(0.0002), dec!(0.00021), dec!(0.00008), dec!(7.505448331328)),
    ] {
        p.s0 = r(s).unwrap();
        let got = candidate(&i, &p, 0, &mut false).unwrap();
        assert_eq!((got.gross_entry, got.remainder, got.charge), (q, rh, a));
    }
    assert_eq!(Arithmetic::default().decimal(&p.cap).unwrap(), dec!(18.05));
}
#[test]
fn full_filter_valuation_is_not_ticker_or_adverse_fill() {
    let mut i = fixture();
    i.rules.notional.max = dec!(35);
    i.filter_price.as_mut().unwrap().value = dec!(400);
    assert_eq!(calculate(&i).unwrap_err(), SizingError::Exhausted {
        reason: CandidateRefusal::Filter,
        attempts: 4
    });
    i.filter_price.as_mut().unwrap().value = dec!(100);
    assert!(calculate(&i).is_ok());
    i.filter_price.as_mut().unwrap().value = dec!(1);
    assert_eq!(calculate(&i).unwrap_err(), SizingError::Exhausted {
        reason: CandidateRefusal::Filter,
        attempts: 4
    });
    for wrong in 0..4 {
        let mut bad = fixture();
        match wrong {
            0 => bad.filter_price = None,
            1 => bad.filter_price.as_mut().unwrap().rules_generation = 10,
            2 => bad.filter_price.as_mut().unwrap().source = FilterPriceSource::LastPrice,
            _ => bad.filter_price.as_mut().unwrap().observation.binding.accounting_version = 99,
        };
        assert_eq!(calculate(&bad).unwrap_err(), SizingError::FilterPrice);
    }
    i.rules.notional.apply_min = false;
    i.rules.notional.apply_max = false;
    i.filter_price = None;
    assert!(calculate(&i).is_ok());
}
fn support(i: &mut SizingInputs, level: D, span: D) {
    i.chart.support = level;
    i.chart.plan.basis = Price::new(level).unwrap();
    i.chart.plan.trigger = i.chart.plan.basis;
    i.chart.plan.cap_basis_distance = Some(span);
    i.chart.plan.executable_span = Some(span);
}
#[test]
fn candidate_search_is_initial_plus_three_reductions_only() {
    let mut first = fixture();
    first.basis.amount = dec!(10000);
    first.cash.free = dec!(50);
    assert_eq!(calculate(&first).unwrap().attempts(), 1);
    assert_eq!(calculate(&fixture()).unwrap().attempts(), 2);
    let mut third = fixture();
    support(&mut third, dec!(60), dec!(40));
    assert_eq!(calculate(&third).unwrap().attempts(), 3);
    let mut fourth = fixture();
    support(&mut fourth, dec!(70), dec!(30));
    fourth.basis.amount = dec!(1800);
    assert_eq!(calculate(&fourth).unwrap().attempts(), 4);
    let mut fifth = fixture();
    support(&mut fifth, dec!(80), dec!(20));
    assert_eq!(calculate(&fifth).unwrap_err(), SizingError::Exhausted {
        reason: CandidateRefusal::ChargeCap,
        attempts: 4
    });
}
// Independent reduced i128 fractions: bounded test inputs only. No production
// rational, grid, export, sizing or geometry helper is used by the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct F(i128, i128);
impl F {
    fn new(n: i128, d: i128) -> Self {
        assert!(n >= 0 && d > 0);
        let (mut a, mut b) = (n, d);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Self(n / a, d / a)
    }
    fn dec(v: D) -> Self {
        Self::new(v.mantissa(), 10_i128.pow(v.scale()))
    }
    fn add(self, b: Self) -> Self {
        Self::new(self.0 * b.1 + b.0 * self.1, self.1 * b.1)
    }
    fn sub(self, b: Self) -> Self {
        Self::new(self.0 * b.1 - b.0 * self.1, self.1 * b.1)
    }
    fn mul(self, b: Self) -> Self {
        Self::new(self.0 * b.0, self.1 * b.1)
    }
    fn div(self, b: Self) -> Self {
        Self::new(self.0 * b.1, self.1 * b.0)
    }
    fn le(self, b: Self) -> bool {
        self.0 * b.1 <= b.0 * self.1
    }
    fn min(self, b: Self) -> Self {
        if self.le(b) {
            self
        } else {
            b
        }
    }
    fn floor(self, p: u32) -> Self {
        let f = 10_i128.pow(p);
        Self::new(self.0 * f / self.1, f)
    }
    fn grid(self, s: Self, up: bool) -> Self {
        let ratio = self.div(s);
        let q = ratio.0 / ratio.1 + i128::from(up && ratio.0 % ratio.1 != 0);
        F::new(q, 1).mul(s)
    }
}
fn oracle(i: &SizingInputs) -> Option<([F; 7], u8)> {
    let one = F(1, 1);
    let p = F::dec(i.price.value);
    let b = F::dec(i.basis.amount);
    let step = F::dec(i.rules.lot.step);
    let f = F::dec(i.terms.taker_fraction());
    let t = one.sub(f);
    let u = one.sub(F::dec(i.policy.margin_bps()).div(F(10000, 1)));
    let af = F::dec(i.chart.plan.trigger.as_decimal())
        .mul(one.sub(F::dec(i.terms.gap_bps()).div(F(10000, 1))));
    let l = p.sub(af).add(f.mul(p.add(af)));
    let cap = b.div(F(100, 1));
    let physical = b
        .min(F::dec(i.cash.free))
        .mul(one.sub(F::dec(i.risk.margin_headroom_bps()).div(F(10000, 1))));
    let s0 = cap.div(l).floor(12).min(physical.div(p).floor(8)).grid(step, false);
    let min = F::dec(i.rules.notional.min);
    let r0 = min.div(af.mul(u)).grid(step, true);
    for k in 0..4 {
        let decrease = F(k, 1).mul(step);
        if !decrease.le(s0) {
            continue;
        }
        let s = s0.sub(decrease);
        if s.0 == 0 || !F::dec(i.rules.lot.min).le(s) || !s.le(F::dec(i.rules.lot.max)) {
            continue;
        }
        let q = s.div(t).grid(step, true);
        if !q.le(F::dec(i.rules.lot.max)) {
            continue;
        }
        let rh = s.min(r0);
        let a = s.mul(l).add(q.sub(s).mul(p)).add(rh.mul(af).mul(t));
        let gross = q.mul(p);
        if !a.le(cap)
            || !gross.le(physical)
            || i.policy.max_entry_notional().is_some_and(|c| !gross.le(F::dec(c)))
            || !min.le(gross.mul(u))
            || !min.le(s.mul(af).mul(u))
        {
            continue;
        }
        if let Some(v) = &i.filter_price {
            let v = F::dec(v.value);
            let n = i.rules.notional;
            if [q, s].into_iter().any(|qty| {
                (n.apply_min && !min.le(qty.mul(v)))
                    || (n.apply_max && !qty.mul(v).le(F::dec(n.max)))
            }) {
                continue;
            }
        }
        return Some(([s, q, af, l, rh, a, gross], k as u8 + 1));
    }
    None
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(96))]
    #[test]
    fn independent_i128_oracle_matches_complete_candidate_sequence(b in 200_u32..5000,free in 10_u32..2000,level in 0usize..4,grid in 0usize..3,fee in proptest::bool::ANY,gap in proptest::bool::ANY,margin in proptest::bool::ANY){
        let mut i=fixture();let (c,span)=[(dec!(50),dec!(50)),(dec!(60),dec!(40)),(dec!(70),dec!(30)),(dec!(80),dec!(20))][level];support(&mut i,c,span);i.basis.amount=D::from(b);i.cash.free=D::from(free);i.rules.lot.step=[dec!(0.01),dec!(0.1),dec!(1)][grid];i.rules.lot.min=i.rules.lot.step;i.rules.market_lot=None;
        let gs=if gap{"10"}else{"0"};i.terms=sizing_terms(&i.binding.account,if fee{"10"}else{"0"},gs);i.policy=CostPolicy::parse(gs,Some(if margin{"200"}else{"0"}),Some("1000000")).unwrap();
        let expected=oracle(&i);let actual=calculate(&i);
        match(expected,actual){(Some((e,attempt)),Ok(got))=>{let a=got.amounts();proptest::prop_assert_eq!([a.protection,a.gross_entry,a.adverse_fill,a.loss_per_unit,a.remainder,a.charge,a.gross_notional].map(F::dec),e);proptest::prop_assert_eq!(got.attempts(),attempt);},(None,Err(SizingError::Exhausted{..}))=>{},(e,a)=>proptest::prop_assert!(false,"oracle={e:?} actual={a:?}")}
    }
}

#[test]
fn complete_fraction_extremes_preserve_intermediates_and_exact_refusals() {
    let rows: serde_json::Value =
        serde_json::from_str(include_str!("testdata/sizing_oracle.json")).unwrap();
    let exact = |s: &str| s.parse::<crate::balance_costs::exact::ExactDecimal>().unwrap().value();
    for row in rows.as_array().unwrap() {
        let d = |key: &str| exact(row[key].as_str().unwrap());
        let mut i = fixture();
        i.price.value = d("price");
        i.chart.support = d("chart");
        i.chart.plan.basis = Price::new(i.chart.support).unwrap();
        i.chart.plan.trigger = i.chart.plan.basis;
        // Goldens separately prove this native span; financial oracle never calls Rust
        // helpers.
        let span = i.price.value.checked_sub(i.chart.support).unwrap();
        i.chart.plan.cap_basis_distance = Some(span);
        i.chart.plan.executable_span = Some(span);
        i.rules.price.tick = dec!(0.0000000000000000000000000001);
        i.chart.plan.tick_size = Some(i.rules.price.tick);
        i.rules.price.max = D::ZERO;
        i.basis.amount = d("basis");
        i.cash.free = d("free");
        i.rules.lot.step = d("step");
        i.rules.lot.min = i.rules.lot.step;
        i.rules.lot.max = D::MAX;
        i.rules.market_lot = None;
        i.rules.notional.min = d("minimum");
        i.rules.notional.max = D::MAX;
        i.rules.notional.apply_min = false;
        i.rules.notional.apply_max = false;
        i.filter_price = None;
        let gap = row["gap"].as_str().unwrap();
        i.terms = sizing_terms(&i.binding.account, row["fee_bps"].as_str().unwrap(), gap);
        i.policy = CostPolicy::parse(gap, Some(row["margin"].as_str().unwrap()), None).unwrap();
        // Activate nonbinding filters/cap so every late refusal exercises
        // the full candidate conversion path (formerly154 imports).
        if row["name"] == "final-unrepresentable" {
            i.rules.notional.apply_min = true;
            i.rules.notional.apply_max = true;
            i.filter_price = fixture().filter_price;
            i.policy =
                CostPolicy::parse(gap, Some(row["margin"].as_str().unwrap()), Some("1000000"))
                    .unwrap();
        }
        TRACE.with_borrow_mut(Vec::clear);
        let actual = calculate(&i);
        let trace = TRACE.with_borrow(|t| t.clone());
        let mut counts = vec![];
        let mut count = 0;
        let mut bits = 0;
        let mut divisions = 0;
        let mut imports = 0;
        for (kind, v) in trace {
            match kind {
                "prepare" | "candidate" => {
                    if !counts.is_empty() {
                        *counts.last_mut().unwrap() = count;
                    }
                    counts.push(0);
                    count = 0;
                },
                "call" => count += v,
                "bits" => bits = bits.max(v),
                "integer_div" => divisions += v,
                "import" => imports += v,
                "export_div" => {},
                _ => panic!("unknown trace"),
            }
        }
        *counts.last_mut().unwrap() = count;
        assert!(counts.len() <= 5);
        assert!(counts[0] <= 57);
        assert!(counts[1..].iter().all(|c| *c <= 37));
        assert!(counts.iter().sum::<u64>() <= 205);
        assert!(bits <= 872);
        assert!(divisions <= 774);
        assert!(imports <= 128, "imports={imports}");
        eprintln!("sizing-proof case={} frames={counts:?} imports={imports} max_bits={bits} divisions={divisions}", row["name"]);
        if let Some(amounts) = row["amounts"].as_array() {
            let result = actual.unwrap_or_else(|e| panic!("{}: {e:?}", row["name"]));
            let a = result.amounts();
            assert_eq!(
                [
                    a.protection,
                    a.gross_entry,
                    a.adverse_fill,
                    a.loss_per_unit,
                    a.remainder,
                    a.charge,
                    a.gross_notional
                ],
                amounts
                    .iter()
                    .map(|v| exact(v.as_str().unwrap()))
                    .collect::<Vec<_>>()
                    .as_slice(),
                "{}",
                row["name"]
            );
            assert_eq!(u64::from(result.attempts()), row["attempts"].as_u64().unwrap());
        } else {
            match actual.unwrap_err() {
                SizingError::Exhausted { reason, attempts } => {
                    assert_eq!(
                        format!("{reason:?}"),
                        row["reason"].as_str().unwrap(),
                        "{}",
                        row["name"]
                    );
                    assert_eq!(u64::from(attempts), row["attempts"].as_u64().unwrap());
                },
                SizingError::Unrepresentable => assert_eq!(row["error"], "Unrepresentable"),
                e => panic!("{}: {e:?}", row["name"]),
            }
        }
    }
}

#[test]
fn gross_cash_operator_cap_and_zero_free_are_separate() {
    let mut i = fixture();
    i.basis.amount = dec!(100000);
    i.cash.free = dec!(105);
    i.terms = sizing_terms(&i.binding.account, "10", "0");
    let result = calculate(&i).unwrap();
    assert_eq!(
        (result.attempts(), result.amounts().protection, result.amounts().gross_entry),
        (2, dec!(0.9), dec!(1))
    );
    i.policy = CostPolicy::parse("0", Some("0"), Some("95")).unwrap();
    let capped = calculate(&i).unwrap();
    assert_eq!((capped.attempts(), capped.amounts().gross_notional), (3, dec!(90)));
    i.cash.free = D::ZERO;
    i.cash.locked = dec!(999999);
    assert_eq!(calculate(&i).unwrap_err(), SizingError::Exhausted {
        reason: CandidateRefusal::Quantity,
        attempts: 1
    });
    assert_eq!(result.inputs().terms.taker_bps(), dec!(10));
    i.terms = sizing_terms(&i.binding.account, "20", "0");
    assert_eq!(result.inputs().terms.taker_bps(), dec!(10));
}
#[test]
fn venue_upper_checks_gross_q_and_lower_checks_protection_s() {
    let mut i = fixture();
    i.basis.amount = dec!(100000);
    i.cash.free = dec!(105);
    i.terms = sizing_terms(&i.binding.account, "10", "0");
    i.filter_price.as_mut().unwrap().value = dec!(100);
    i.rules.notional.max = dec!(95);
    assert_eq!(
        calculate(&i).unwrap().attempts(),
        3,
        "Q breaches upperfilter while S fits atcandidate1"
    );
    i.rules.notional.max = dec!(1000000);
    i.rules.notional.min = dec!(0.95);
    i.filter_price.as_mut().unwrap().value = D::ONE;
    assert_eq!(
        calculate(&i).unwrap_err(),
        SizingError::Exhausted {
            reason: CandidateRefusal::Filter,
            attempts: 4
        },
        "S fails lowerfilter while Q fits atcandidate1"
    );
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(64))]
    #[test]
    fn reducing_capital_cannot_increase_an_accepted_gross_size(b in 1000_u32..5000,extra in 1_u32..1000){
        let mut smaller=fixture();smaller.basis.amount=D::from(b);let mut larger=smaller.clone();larger.basis.amount=D::from(b+extra);
        if let (Ok(s),Ok(l))=(calculate(&smaller),calculate(&larger)){proptest::prop_assert!(s.amounts().gross_notional<=l.amounts().gross_notional);}
    }
}
