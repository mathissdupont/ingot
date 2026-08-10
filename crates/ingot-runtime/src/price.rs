//! What a model call costs, so a `cost` budget can be charged.
//!
//! [Runtime 0.1 §8](../../../specs/runtime/v0.1.md) says a backend that can
//! price a request should enforce `budget.cost`, and one that cannot **must not
//! pretend to**. This module is the first half; the interpreter is the second.
//!
//! # Why the operator supplies the prices
//!
//! A price table is provider- and time-dependent, so it cannot live in an
//! artifact: an artifact carrying one would be stale the moment it was
//! published, and a reproducible artifact whose meaning changed with the
//! vendor's price list would not be reproducible at all. It cannot live in this
//! binary either, for the same reason with a slower clock.
//!
//! So it lives where the API keys and the tool servers already live: in the
//! project manifest, which is deployment configuration the operator owns and
//! updates. A run with no prices configured charges nothing and says so.
//!
//! # Why integers
//!
//! Money is decimal and binary floats are not, which is why
//! [`ingot_ir::Cost`] stores an amount as a decimal string with six fractional
//! digits. Accumulation here uses the same unit as an integer — millionths of
//! one currency unit — so a total is exact and identical on every platform.
//! Nothing in a cost calculation touches an `f64`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::provider::Usage;

/// One millionth of a currency unit, which is the precision
/// [`ingot_ir::format_amount`] already renders.
pub type Micros = u128;

/// Millionths per whole unit.
pub const MICROS: Micros = 1_000_000;

/// Prices are quoted per this many tokens, which is how every vendor quotes
/// them.
pub const TOKENS_PER_QUOTE: Micros = 1_000_000;

/// What one model costs, as the operator wrote it in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ModelPrice {
    /// The model string **as the provider reports it**, matched exactly.
    ///
    /// Not a pattern: a prefix rule would silently price `claude-opus-5-mini`
    /// at `claude-opus-5`'s rate, and a wrong price is worse than none. The run
    /// names the unpriced model, so configuring it is a copy and a paste.
    pub model: String,
    /// Cost of a million input tokens, as a decimal string.
    pub input: String,
    /// Cost of a million output tokens, as a decimal string.
    pub output: String,
    /// Cost of a million cached input tokens, when the vendor discounts them.
    /// Absent means cached input is charged at the `input` rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<String>,
    /// The currency these amounts are in, e.g. `usd`.
    pub currency: String,
}

/// Every price a run was given.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pricing {
    models: Vec<ModelPrice>,
}

/// What a priced call cost, or why it could not be priced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Charge {
    /// Millionths of `currency`.
    Priced { micros: Micros, currency: String },
    /// No price is configured for the model that answered.
    Unpriced,
    /// A price exists and is in a different currency than the budget states.
    /// Converting would need a rate, which is a second time-dependent input the
    /// toolchain does not have.
    WrongCurrency { priced_in: String },
}

impl Pricing {
    pub fn new(models: Vec<ModelPrice>) -> Pricing {
        Pricing { models }
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Every model this run can price, for a readiness report.
    pub fn models(&self) -> impl Iterator<Item = &str> {
        self.models.iter().map(|price| price.model.as_str())
    }

    /// What `usage` cost on `model`, in the currency the budget is stated in.
    pub fn charge(&self, model: &str, usage: Usage, budget_currency: &str) -> Charge {
        let Some(price) = self.models.iter().find(|price| price.model == model) else {
            return Charge::Unpriced;
        };
        if !price.currency.eq_ignore_ascii_case(budget_currency) {
            return Charge::WrongCurrency {
                priced_in: price.currency.clone(),
            };
        }

        // Cached input is charged at its own rate when the operator gave one,
        // and at the input rate otherwise — the conservative reading, since a
        // vendor that does not discount cache reads bills them as input.
        let uncached = usage.input_tokens.saturating_sub(usage.cache_read_tokens);
        let cache_rate = price.cache_read.as_deref().unwrap_or(&price.input);

        let mut micros: Micros = 0;
        for (tokens, rate) in [
            (uncached, price.input.as_str()),
            (usage.cache_read_tokens, cache_rate),
            (usage.output_tokens, price.output.as_str()),
        ] {
            let Some(per_quote) = parse_micros(rate) else {
                return Charge::Unpriced;
            };
            micros += Micros::from(tokens) * per_quote / TOKENS_PER_QUOTE;
        }

        Charge::Priced {
            micros,
            currency: price.currency.clone(),
        }
    }
}

/// A decimal string as millionths, or `None` when it is not one.
///
/// Integer-only: `"3.5"` is 3_500_000, not a float that might be 3.4999999.
/// More than six fractional digits is a price this format cannot represent
/// exactly, so it is refused rather than rounded behind the operator's back.
pub fn parse_micros(amount: &str) -> Option<Micros> {
    let amount = amount.trim();
    if amount.is_empty() || amount.starts_with('-') {
        return None;
    }
    let (whole, fraction) = match amount.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (amount, ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.chars().all(|ch| ch.is_ascii_digit())
        || !fraction.chars().all(|ch| ch.is_ascii_digit())
        || fraction.len() > 6
    {
        return None;
    }

    let whole: Micros = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut padded = fraction.to_string();
    while padded.len() < 6 {
        padded.push('0');
    }
    let fraction: Micros = if padded.is_empty() {
        0
    } else {
        padded.parse().ok()?
    };
    whole.checked_mul(MICROS)?.checked_add(fraction)
}

/// Millionths as the decimal string the IR uses.
///
/// The inverse of [`parse_micros`], to six digits with trailing zeros trimmed,
/// so `4_200` renders as `0.0042` and `5_000_000` as `5`.
pub fn render_micros(micros: Micros) -> String {
    let whole = micros / MICROS;
    let fraction = micros % MICROS;
    if fraction == 0 {
        return whole.to_string();
    }
    let rendered = format!("{whole}.{fraction:06}");
    rendered.trim_end_matches('0').to_string()
}

/// What a run spent, and what it could not price.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spend {
    micros: Micros,
    currency: Option<String>,
    /// Models that answered and had no usable price. Sorted and deduplicated,
    /// because the same model answering forty times is one thing to configure.
    unpriced: BTreeMap<String, String>,
}

impl Spend {
    /// Add a charge, remembering an unpriced call rather than skipping it.
    pub fn add(&mut self, model: &str, charge: Charge) {
        match charge {
            Charge::Priced { micros, currency } => {
                self.micros += micros;
                self.currency = Some(currency);
            }
            Charge::Unpriced => {
                self.unpriced
                    .insert(model.to_string(), "no price is configured".to_string());
            }
            Charge::WrongCurrency { priced_in } => {
                self.unpriced.insert(
                    model.to_string(),
                    format!("priced in {priced_in}, which the budget is not"),
                );
            }
        }
    }

    pub fn micros(&self) -> Micros {
        self.micros
    }

    /// Whether every call that happened could be priced.
    ///
    /// The question a `cost` budget's enforcement depends on: a total that
    /// missed calls is not a total, and reporting it as one would be the
    /// pretending [Runtime 0.1 §8](../../../specs/runtime/v0.1.md) forbids.
    pub fn is_complete(&self) -> bool {
        self.unpriced.is_empty()
    }

    /// Model name and why it could not be priced.
    pub fn unpriced(&self) -> impl Iterator<Item = (&str, &str)> {
        self.unpriced
            .iter()
            .map(|(model, reason)| (model.as_str(), reason.as_str()))
    }

    /// What was spent, when anything could be priced at all.
    pub fn rendered(&self) -> Option<String> {
        let currency = self.currency.as_ref()?;
        Some(format!(
            "{} {}",
            render_micros(self.micros),
            currency.to_ascii_uppercase()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cached: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cached,
        }
    }

    fn opus() -> Pricing {
        Pricing::new(vec![ModelPrice {
            model: "claude-opus-5".into(),
            input: "3".into(),
            output: "15".into(),
            cache_read: Some("0.3".into()),
            currency: "usd".into(),
        }])
    }

    #[test]
    fn a_decimal_string_becomes_exact_millionths() {
        assert_eq!(parse_micros("3"), Some(3_000_000));
        assert_eq!(parse_micros("0.3"), Some(300_000));
        assert_eq!(parse_micros("3.5"), Some(3_500_000));
        assert_eq!(parse_micros("0.000001"), Some(1));
        assert_eq!(parse_micros(".5"), Some(500_000));
    }

    #[test]
    fn an_amount_this_format_cannot_hold_is_refused_rather_than_rounded() {
        // Rounding behind the operator's back is how a budget quietly stops
        // meaning what it says.
        assert_eq!(parse_micros("0.0000001"), None);
        assert_eq!(parse_micros("-1"), None);
        assert_eq!(parse_micros("free"), None);
        assert_eq!(parse_micros(""), None);
        assert_eq!(parse_micros("1.2.3"), None);
    }

    #[test]
    fn rendering_round_trips_through_the_ir_encoding() {
        for amount in ["0", "5", "0.25", "0.0042", "1234.567891"] {
            let micros = parse_micros(amount).expect("a valid amount");
            assert_eq!(render_micros(micros), amount);
        }
    }

    #[test]
    fn a_call_is_charged_at_the_quoted_rate() {
        // 1000 input at $3/M and 500 output at $15/M is 0.003 + 0.0075.
        let charge = opus().charge("claude-opus-5", usage(1000, 500, 0), "usd");
        assert_eq!(
            charge,
            Charge::Priced {
                micros: 3_000 + 7_500,
                currency: "usd".into()
            }
        );
        assert_eq!(render_micros(10_500), "0.0105");
    }

    #[test]
    fn cached_input_is_charged_at_its_own_rate_when_one_is_given() {
        // 1000 input of which 800 cached: 200 at $3/M, 800 at $0.3/M.
        let charge = opus().charge("claude-opus-5", usage(1000, 0, 800), "usd");
        assert_eq!(
            charge,
            Charge::Priced {
                micros: 600 + 240,
                currency: "usd".into()
            }
        );
    }

    #[test]
    fn cached_input_falls_back_to_the_input_rate() {
        // A vendor that does not discount cache reads bills them as input, so
        // an absent rate must not become a free one.
        let pricing = Pricing::new(vec![ModelPrice {
            model: "m".into(),
            input: "3".into(),
            output: "15".into(),
            cache_read: None,
            currency: "usd".into(),
        }]);
        let charge = pricing.charge("m", usage(1000, 0, 1000), "usd");
        assert_eq!(
            charge,
            Charge::Priced {
                micros: 3_000,
                currency: "usd".into()
            }
        );
    }

    #[test]
    fn an_unknown_model_is_unpriced_rather_than_free() {
        assert_eq!(
            opus().charge("claude-opus-5-mini", usage(1000, 500, 0), "usd"),
            Charge::Unpriced,
            "a prefix rule would price a different model at this one's rate"
        );
        assert_eq!(
            Pricing::default().charge("anything", usage(1, 1, 0), "usd"),
            Charge::Unpriced
        );
    }

    #[test]
    fn a_price_in_another_currency_does_not_get_converted() {
        let charge = opus().charge("claude-opus-5", usage(1000, 0, 0), "eur");
        assert_eq!(
            charge,
            Charge::WrongCurrency {
                priced_in: "usd".into()
            },
            "converting needs a rate, which is a second time-dependent input"
        );
    }

    #[test]
    fn a_spend_that_missed_a_call_is_not_a_total() {
        let mut spend = Spend::default();
        spend.add(
            "claude-opus-5",
            opus().charge("claude-opus-5", usage(1000, 500, 0), "usd"),
        );
        assert!(spend.is_complete());
        assert_eq!(spend.rendered().as_deref(), Some("0.0105 USD"));

        spend.add("mystery", Charge::Unpriced);
        assert!(
            !spend.is_complete(),
            "a total that missed a call is not a total"
        );
        let unpriced: Vec<&str> = spend.unpriced().map(|(model, _)| model).collect();
        assert_eq!(unpriced, vec!["mystery"]);

        // The same model answering many times is one thing to configure.
        spend.add("mystery", Charge::Unpriced);
        assert_eq!(spend.unpriced().count(), 1);
    }
}
