pub fn short_decimal(value: u64) -> String {
    let text = value.to_string();
    if text.len() <= 18 {
        text
    } else {
        format!("{}...{}", &text[..10], &text[text.len() - 6..])
    }
}

pub fn hash_rate(value: f64) -> String {
    if !value.is_finite() || value <= 0.0 {
        return "0 H/s".to_string();
    }

    let units = ["H/s", "KH/s", "MH/s", "GH/s", "TH/s"];
    let mut n = value;
    let mut unit = 0;
    while n >= 1000.0 && unit < units.len() - 1 {
        n /= 1000.0;
        unit += 1;
    }

    let number = if n >= 100.0 {
        format!("{n:.0}")
    } else if n >= 10.0 {
        format!("{n:.1}")
    } else {
        format!("{n:.2}")
    };
    format!("{number} {}", units[unit])
}
