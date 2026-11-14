//! Cutting a list of rows into pages.

use std::ops::Range;

/// The rows of one page, and how many pages there are.
///
/// There is always at least one page, even for no rows. A `page` past the end shows the last
/// page. A `page_size` of 0 counts as 1.
pub fn page_range(total_rows: usize, page: usize, page_size: usize) -> (Range<usize>, usize) {
    let page_size = page_size.max(1);
    let pages = total_rows.div_ceil(page_size).max(1);
    let page = page.min(pages - 1);
    let start = page * page_size;
    let end = start.saturating_add(page_size).min(total_rows);
    (start..end, pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_has_one_empty_page() {
        assert_eq!(page_range(0, 0, 10), (0..0, 1));
        assert_eq!(page_range(0, 5, 10), (0..0, 1));
    }

    #[test]
    fn exact_multiple() {
        assert_eq!(page_range(20, 0, 10), (0..10, 2));
        assert_eq!(page_range(20, 1, 10), (10..20, 2));
    }

    #[test]
    fn partial_last_page() {
        assert_eq!(page_range(25, 2, 10), (20..25, 3));
        assert_eq!(page_range(21, 2, 10), (20..21, 3));
    }

    #[test]
    fn page_past_the_end_is_clamped() {
        assert_eq!(page_range(25, 99, 10), (20..25, 3));
        assert_eq!(page_range(5, 1, 10), (0..5, 1));
    }

    #[test]
    fn fewer_rows_than_a_page() {
        assert_eq!(page_range(3, 0, 10), (0..3, 1));
    }

    #[test]
    fn zero_page_size_counts_as_one() {
        assert_eq!(page_range(3, 1, 0), (1..2, 3));
    }

    #[test]
    fn huge_values_dont_overflow() {
        assert_eq!(page_range(5, usize::MAX, usize::MAX), (0..5, 1));
        assert_eq!(page_range(usize::MAX, usize::MAX, 1).1, usize::MAX);
    }

    #[test]
    fn pages_cover_every_row_exactly_once() {
        let total = 47;
        let (_, pages) = page_range(total, 0, 10);
        let mut seen = Vec::new();
        for page in 0..pages {
            seen.extend(page_range(total, page, 10).0);
        }
        assert_eq!(seen, (0..total).collect::<Vec<_>>());
    }
}
