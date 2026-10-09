//! Human rendering of 6-field cron expressions.

use super::format::{format_clock_time, pad2};

const DOW_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn is_star(field: &str) -> bool {
    field == "*"
}

fn step_of(field: &str) -> Option<i64> {
    let digits = field.strip_prefix("*/")?;
    num_of(digits)
}

fn num_of(field: &str) -> Option<i64> {
    if !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()) {
        field.parse().ok()
    } else {
        None
    }
}

fn dow_index(token: &str) -> Option<usize> {
    if let Some(n) = num_of(token) {
        return match n {
            7 => Some(0),
            0..=6 => usize::try_from(n).ok(),
            _ => None,
        };
    }
    let prefix: String = token.chars().take(3).collect::<String>().to_lowercase();
    match prefix.as_str() {
        "sun" => Some(0),
        "mon" => Some(1),
        "tue" => Some(2),
        "wed" => Some(3),
        "thu" => Some(4),
        "fri" => Some(5),
        "sat" => Some(6),
        _ => None,
    }
}

fn describe_dow(field: &str) -> Option<String> {
    let mut names = Vec::new();
    for part in field.split(',') {
        let range: Vec<&str> = part.split('-').collect();
        if range.len() == 2 && !range[0].is_empty() && !range[1].is_empty() {
            let from = dow_index(range[0])?;
            let to = dow_index(range[1])?;
            names.push(format!("{}-{}", DOW_NAMES[from], DOW_NAMES[to]));
        } else {
            names.push(DOW_NAMES[dow_index(part)?].to_owned());
        }
    }
    Some(names.join(", "))
}

/// Best-effort description of "sec min hour dom mon dow"; `None` when unsure.
pub fn describe_cron(expr: &str) -> Option<String> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    let [sec, min, hour, dom, mon, dow] = parts.as_slice() else {
        return None;
    };
    let rest_is_star = is_star(hour) && is_star(dom) && is_star(mon) && is_star(dow);

    if let Some(step) = step_of(sec)
        && is_star(min)
        && rest_is_star
    {
        return Some(if step == 1 {
            "every second".to_owned()
        } else {
            format!("every {step} seconds")
        });
    }
    num_of(sec)?;

    if let Some(step) = step_of(min)
        && rest_is_star
    {
        return Some(if step == 1 {
            "every minute".to_owned()
        } else {
            format!("every {step} minutes")
        });
    }
    if is_star(min) && rest_is_star {
        return Some("every minute".to_owned());
    }
    let min_num = num_of(min)?;
    let date_is_star = is_star(dom) && is_star(mon) && is_star(dow);

    if let Some(step) = step_of(hour)
        && date_is_star
    {
        return Some(if step == 1 {
            format!("hourly at :{}", pad2(min_num))
        } else {
            format!("every {step} hours at :{}", pad2(min_num))
        });
    }
    if is_star(hour) && date_is_star {
        return Some(format!("hourly at :{}", pad2(min_num)));
    }
    let hour_num = num_of(hour)?;
    let time = format_clock_time(hour_num, min_num);
    if date_is_star {
        return Some(format!("daily at {time}"));
    }
    if is_star(dom) && is_star(mon) {
        return describe_dow(dow).map(|days| format!("{days} at {time}"));
    }
    if let Some(dom_num) = num_of(dom)
        && is_star(mon)
        && is_star(dow)
    {
        return Some(format!("monthly on day {dom_num} at {time}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::describe_cron;

    #[test]
    fn describes_common_schedules() {
        assert_eq!(
            describe_cron("*/20 * * * * *").as_deref(),
            Some("every 20 seconds")
        );
        assert_eq!(
            describe_cron("0 */5 * * * *").as_deref(),
            Some("every 5 minutes")
        );
        assert_eq!(
            describe_cron("0 17 * * * *").as_deref(),
            Some("hourly at :17")
        );
        assert_eq!(
            describe_cron("0 0 */6 * * *").as_deref(),
            Some("every 6 hours at :00")
        );
        assert_eq!(
            describe_cron("0 0 17 * * 1,3,5").as_deref(),
            Some("Mon, Wed, Fri at 5:00 PM")
        );
        assert_eq!(
            describe_cron("0 0 4 * * 0").as_deref(),
            Some("Sun at 4:00 AM")
        );
        assert_eq!(
            describe_cron("0 0 9 1 * *").as_deref(),
            Some("monthly on day 1 at 9:00 AM")
        );
        assert_eq!(describe_cron("*/5 * * * *"), None);
        assert_eq!(
            describe_cron("0 0 9 * * mon-fri").as_deref(),
            Some("Mon-Fri at 9:00 AM")
        );
    }
}
