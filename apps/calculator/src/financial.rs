//! Financial functions. Rates are fractions per period (5% = 0.05).

use crate::engine::CalcError;

/// Describes a financial function for the UI's input forms.
pub struct FinancialFn {
    pub name: &'static str,
    pub title: &'static str,
    pub params: &'static [&'static str],
}

pub const FUNCTIONS: &[FinancialFn] = &[
    FinancialFn {
        name: "ctrm",
        title: "Compounding Term",
        params: &["Periodic interest rate", "Future value", "Present value"],
    },
    FinancialFn {
        name: "ddb",
        title: "Double-Declining Depreciation",
        params: &["Cost", "Life", "Period"],
    },
    FinancialFn {
        name: "fv",
        title: "Future Value",
        params: &[
            "Periodic payment",
            "Periodic interest rate",
            "Number of periods",
        ],
    },
    FinancialFn {
        name: "gpm",
        title: "Gross Profit Margin",
        params: &["Cost", "Margin"],
    },
    FinancialFn {
        name: "pmt",
        title: "Periodic Payment",
        params: &["Principal", "Periodic interest rate", "Term"],
    },
    FinancialFn {
        name: "pv",
        title: "Present Value",
        params: &[
            "Periodic payment",
            "Periodic interest rate",
            "Number of periods",
        ],
    },
    FinancialFn {
        name: "rate",
        title: "Periodic Interest Rate",
        params: &["Future value", "Present value", "Term"],
    },
    FinancialFn {
        name: "sln",
        title: "Straight-Line Depreciation",
        params: &["Cost", "Salvage", "Life"],
    },
    FinancialFn {
        name: "syd",
        title: "Sum-of-the-Years'-Digits Depreciation",
        params: &["Cost", "Salvage", "Life", "Period"],
    },
    FinancialFn {
        name: "term",
        title: "Payment Period",
        params: &["Periodic payment", "Future value", "Periodic interest rate"],
    },
];

/// Evaluates financial function `name`, or returns `None` if `name` is not
/// a financial function.
pub fn call(name: &str, a: &[f64]) -> Option<Result<f64, CalcError>> {
    let f = FUNCTIONS.iter().find(|f| f.name == name)?;
    if a.len() != f.params.len() {
        return Some(Err(CalcError::Arity {
            func: name.into(),
            expected: f.params.len(),
        }));
    }
    let undefined = || {
        Err(CalcError::Domain(format!(
            "{} is undefined for these values",
            f.title
        )))
    };
    let r = match name {
        "ctrm" => {
            let (rate, fv, pv) = (a[0], a[1], a[2]);
            if rate <= -1.0 || rate == 0.0 || pv == 0.0 || fv / pv <= 0.0 {
                return Some(undefined());
            }
            (fv / pv).ln() / (1.0 + rate).ln()
        }
        "ddb" => {
            let (cost, life, period) = (a[0], a[1], a[2]);
            if life <= 0.0 || period < 1.0 || period > life || period.fract() != 0.0 {
                return Some(undefined());
            }
            let mut book = cost;
            let mut dep = 0.0;
            for _ in 0..period as u64 {
                dep = book * 2.0 / life;
                book -= dep;
            }
            dep
        }
        "fv" => {
            let (pmt, rate, n) = (a[0], a[1], a[2]);
            if rate == 0.0 {
                pmt * n
            } else {
                pmt * ((1.0 + rate).powf(n) - 1.0) / rate
            }
        }
        "gpm" => {
            let (cost, margin) = (a[0], a[1]);
            if margin >= 1.0 {
                return Some(undefined());
            }
            cost / (1.0 - margin)
        }
        "pmt" => {
            let (principal, rate, n) = (a[0], a[1], a[2]);
            if n == 0.0 {
                return Some(undefined());
            }
            if rate == 0.0 {
                principal / n
            } else {
                principal * rate / (1.0 - (1.0 + rate).powf(-n))
            }
        }
        "pv" => {
            let (pmt, rate, n) = (a[0], a[1], a[2]);
            if rate == 0.0 {
                pmt * n
            } else {
                pmt * (1.0 - (1.0 + rate).powf(-n)) / rate
            }
        }
        "rate" => {
            let (fv, pv, n) = (a[0], a[1], a[2]);
            if pv == 0.0 || n == 0.0 || fv / pv < 0.0 {
                return Some(undefined());
            }
            (fv / pv).powf(1.0 / n) - 1.0
        }
        "sln" => {
            let (cost, salvage, life) = (a[0], a[1], a[2]);
            if life == 0.0 {
                return Some(Err(CalcError::DivisionByZero));
            }
            (cost - salvage) / life
        }
        "syd" => {
            let (cost, salvage, life, period) = (a[0], a[1], a[2], a[3]);
            if life <= 0.0 || period < 1.0 || period > life {
                return Some(undefined());
            }
            (cost - salvage) * (life - period + 1.0) / (life * (life + 1.0) / 2.0)
        }
        "term" => {
            let (pmt, fv, rate) = (a[0], a[1], a[2]);
            if pmt == 0.0 || rate <= -1.0 {
                return Some(undefined());
            }
            if rate == 0.0 {
                fv / pmt
            } else {
                let x = 1.0 + fv * rate / pmt;
                if x <= 0.0 {
                    return Some(undefined());
                }
                x.ln() / (1.0 + rate).ln()
            }
        }
        _ => unreachable!(),
    };
    Some(Ok(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, a: &[f64]) -> f64 {
        call(name, a).unwrap().unwrap()
    }
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * b.abs().max(1.0)
    }

    #[test]
    fn formulas() {
        assert!(close(f("ctrm", &[0.08, 16000.0, 8000.0]), 9.006_468_342));
        assert!(close(f("ddb", &[8000.0, 6.0, 1.0]), 2_666.666_666_667));
        assert!(close(f("ddb", &[8000.0, 6.0, 2.0]), 1_777.777_777_778));
        assert!(close(f("gpm", &[20.0, 0.6]), 50.0));
        assert!(close(f("pv", &[100.0, 0.05, 10.0]), 772.173_492_918_482));
        assert!(close(f("rate", &[2000.0, 1000.0, 10.0]), 0.071_773_462_536));
        assert!(close(f("sln", &[8000.0, 0.0, 4.0]), 2000.0));
        assert!(close(f("syd", &[8000.0, 0.0, 4.0, 1.0]), 3200.0));
        assert!(close(f("term", &[1000.0, 12577.89, 0.05]), 10.0));
        assert!(matches!(
            call("pv", &[1.0]),
            Some(Err(CalcError::Arity { .. }))
        ));
        assert!(call("sin", &[1.0]).is_none());
    }
}
