//! Natural ordering of names: case-insensitive, numbers compared by value ("Car 2" < "Car 10").

use std::cmp::Ordering;

/// Compares two names the way a person sorts them: ignoring case, and digit runs by their numeric
/// value, so `Car 2` comes before `Car 10`. Digit runs that only differ in leading zeros
/// (`01` / `1`) compare equal: break such ties with a plain comparison where a total order matters.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a, b);
    loop {
        match (a.chars().next(), b.chars().next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (number_a, rest_a) = split_digits(a);
                let (number_b, rest_b) = split_digits(b);
                match compare_numbers(number_a, number_b) {
                    Ordering::Equal => {}
                    other => return other,
                }
                (a, b) = (rest_a, rest_b);
            }
            (Some(x), Some(y)) => {
                match fold(x).cmp(&fold(y)) {
                    Ordering::Equal => {}
                    other => return other,
                }
                (a, b) = (&a[x.len_utf8()..], &b[y.len_utf8()..]);
            }
        }
    }
}

/// Lowercase, for comparing without regard to case.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Splits off the leading digits.
fn split_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

/// Compares two digit strings by value (any length, no overflow).
fn compare_numbers(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(natural_cmp("Car 2", "Car 10"), Ordering::Less);
        assert_eq!(natural_cmp("Car 10", "Car 2"), Ordering::Greater);
        assert_eq!(
            sorted(&["Car 10", "Car 9", "Car 100", "Car 1"]),
            ["Car 1", "Car 9", "Car 10", "Car 100"]
        );
    }

    #[test]
    fn ignores_case() {
        assert_eq!(natural_cmp("apple", "BANANA"), Ordering::Less);
        assert_eq!(natural_cmp("Jets", "jets"), Ordering::Equal);
        assert_eq!(sorted(&["b", "A", "c", "B"]), ["A", "b", "B", "c"]);
    }

    #[test]
    fn prefix_sorts_first() {
        assert_eq!(natural_cmp("Car", "Car 2"), Ordering::Less);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
    }

    #[test]
    fn digits_sort_before_letters() {
        assert_eq!(natural_cmp("1a", "ab"), Ordering::Less);
        assert_eq!(natural_cmp("a1", "ab"), Ordering::Less);
    }

    #[test]
    fn leading_zeros_are_equal_in_value() {
        assert_eq!(natural_cmp("a01", "a1"), Ordering::Equal);
        assert_eq!(natural_cmp("a002", "a10"), Ordering::Less);
    }

    #[test]
    fn huge_numbers_dont_overflow() {
        let big = "9".repeat(60);
        let bigger = format!("1{}", "0".repeat(60));
        assert_eq!(natural_cmp(&big, &bigger), Ordering::Less);
    }

    #[test]
    fn several_numbers_in_one_name() {
        assert_eq!(natural_cmp("v1.10", "v1.9"), Ordering::Greater);
        assert_eq!(natural_cmp("a2b10", "a2b9"), Ordering::Greater);
    }

    #[test]
    fn non_ascii_text_is_handled() {
        assert_eq!(natural_cmp("Äpfel", "äpfel"), Ordering::Equal);
        assert_eq!(natural_cmp("é1", "é2"), Ordering::Less);
    }
}
