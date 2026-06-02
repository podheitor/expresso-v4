//! Usage-based billing add-ons: pure computation of overage line items.
//!
//! A plan bundles an allowance (included seats, included storage GB). Usage
//! beyond the allowance is billed per unit at the plan's overage price. This
//! module is pure (no DB): given the plan terms and the measured usage, it
//! produces the invoice lines. The DB read of usage and the write of lines live
//! in `billing.rs`; keeping the math here makes it trivially testable.

/// A plan's allowance + overage pricing (cents).
#[derive(Debug, Clone, Copy)]
pub struct PlanTerms {
    pub base_cents: i64,
    pub included_seats: i64,
    pub seat_overage_cents: i64,
    pub included_storage_gb: i64,
    pub storage_overage_cents_per_gb: i64,
}

/// Measured usage for the billing period.
#[derive(Debug, Clone, Copy)]
pub struct Usage {
    pub seats: i64,
    pub storage_bytes: i64,
}

/// One computed invoice line. `kind` matches the `billing_invoice_lines.kind`
/// CHECK constraint ('base' | 'seat_overage' | 'storage_overage').
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: &'static str,
    pub description: String,
    pub quantity: i64,
    pub unit_cents: i64,
    pub amount_cents: i64,
}

const BYTES_PER_GB: i64 = 1024 * 1024 * 1024;

/// Storage bytes → whole GB, rounded UP (a partial GB over the allowance is a
/// billable GB). Never negative.
pub fn bytes_to_gb_ceil(bytes: i64) -> i64 {
    if bytes <= 0 {
        return 0;
    }
    (bytes + BYTES_PER_GB - 1) / BYTES_PER_GB
}

/// Units of `used` beyond `included`, never negative.
fn over(used: i64, included: i64) -> i64 {
    (used - included).max(0)
}

/// Build the invoice lines for a period: always a `base` line, plus a
/// `seat_overage` and/or `storage_overage` line when usage exceeds the
/// allowance AND the plan charges for that dimension (overage price > 0).
pub fn compute_lines(terms: PlanTerms, usage: Usage) -> Vec<Line> {
    let mut lines = vec![Line {
        kind: "base",
        description: "Assinatura mensal".to_string(),
        quantity: 1,
        unit_cents: terms.base_cents,
        amount_cents: terms.base_cents,
    }];

    let extra_seats = over(usage.seats, terms.included_seats);
    if extra_seats > 0 && terms.seat_overage_cents > 0 {
        lines.push(Line {
            kind: "seat_overage",
            description: format!("Usuários adicionais ({extra_seats})"),
            quantity: extra_seats,
            unit_cents: terms.seat_overage_cents,
            amount_cents: extra_seats.saturating_mul(terms.seat_overage_cents),
        });
    }

    let used_gb = bytes_to_gb_ceil(usage.storage_bytes);
    let extra_gb = over(used_gb, terms.included_storage_gb);
    if extra_gb > 0 && terms.storage_overage_cents_per_gb > 0 {
        lines.push(Line {
            kind: "storage_overage",
            description: format!("Armazenamento adicional ({extra_gb} GB)"),
            quantity: extra_gb,
            unit_cents: terms.storage_overage_cents_per_gb,
            amount_cents: extra_gb.saturating_mul(terms.storage_overage_cents_per_gb),
        });
    }

    lines
}

/// Sum of the lines' amounts = the invoice total.
pub fn total_cents(lines: &[Line]) -> i64 {
    lines.iter().map(|l| l.amount_cents).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERMS: PlanTerms = PlanTerms {
        base_cents: 9900,
        included_seats: 10,
        seat_overage_cents: 500,
        included_storage_gb: 50,
        storage_overage_cents_per_gb: 100,
    };

    #[test]
    fn bytes_to_gb_ceil_rounds_up_partial() {
        assert_eq!(bytes_to_gb_ceil(0), 0);
        assert_eq!(bytes_to_gb_ceil(-5), 0);
        assert_eq!(bytes_to_gb_ceil(BYTES_PER_GB), 1);
        assert_eq!(bytes_to_gb_ceil(BYTES_PER_GB + 1), 2);
        assert_eq!(bytes_to_gb_ceil(1), 1);
    }

    #[test]
    fn no_overage_within_allowance() {
        let lines = compute_lines(
            TERMS,
            Usage {
                seats: 8,
                storage_bytes: 10 * BYTES_PER_GB,
            },
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, "base");
        assert_eq!(total_cents(&lines), 9900);
    }

    #[test]
    fn seat_overage_charged_per_extra_seat() {
        let lines = compute_lines(
            TERMS,
            Usage {
                seats: 13,
                storage_bytes: 0,
            },
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].kind, "seat_overage");
        assert_eq!(lines[1].quantity, 3);
        assert_eq!(lines[1].amount_cents, 1500);
        assert_eq!(total_cents(&lines), 9900 + 1500);
    }

    #[test]
    fn storage_overage_charged_per_extra_gb_rounded_up() {
        // 50 GB included; 51.5 GB used → ceil = 52 GB → 2 GB over → 200 cents.
        let lines = compute_lines(
            TERMS,
            Usage {
                seats: 1,
                storage_bytes: 51 * BYTES_PER_GB + BYTES_PER_GB / 2,
            },
        );
        let storage = lines.iter().find(|l| l.kind == "storage_overage").unwrap();
        assert_eq!(storage.quantity, 2);
        assert_eq!(storage.amount_cents, 200);
    }

    #[test]
    fn overage_suppressed_when_price_is_zero() {
        // Same usage, but a plan that does not charge for overage → base only.
        let free = PlanTerms {
            seat_overage_cents: 0,
            storage_overage_cents_per_gb: 0,
            ..TERMS
        };
        let lines = compute_lines(
            free,
            Usage {
                seats: 99,
                storage_bytes: 999 * BYTES_PER_GB,
            },
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(total_cents(&lines), 9900);
    }
}
