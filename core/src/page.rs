//! The paginated list envelope shared by every list endpoint (`GET /maps`,
//! `GET /monsters`, …). One generic struct instead of a per-endpoint copy, so
//! the pagination rule (`page * size >= total`) lives in one place.

/// One page of a paginated API response.
#[derive(Debug, serde::Deserialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub total: u32,
    pub page: u32,
    pub size: u32,
}

impl<T> Page<T> {
    /// True when this is the final page of the listing.
    ///
    /// Pagination rule: `page * size >= total`. The live OpenAPI contract
    /// declares `size` with `minimum: 1` on every `StaticDataPage_*` schema, so
    /// a `size == 0` page is a contract violation. The degenerate case is
    /// handled explicitly anyway: with `size == 0` there is no per-page
    /// capacity, so the listing is "complete" only when it is also empty
    /// (`total == 0`). For `size == 0` with `total > 0`, `is_last` stays false
    /// on purpose — that forces the HTTP driver's `fetch_paginated` loop to
    /// run into its safety cap and surface a loud `Err` rather than silently
    /// truncating the result.
    pub fn is_last(&self) -> bool {
        if self.size == 0 {
            // Degenerate page (contract violation); complete only if empty.
            return self.total == 0;
        }
        self.page * self.size >= self.total
    }
}

#[cfg(test)]
mod tests {
    use super::Page;

    fn page(total: u32, page: u32, size: u32) -> Page<()> {
        Page {
            data: Vec::new(),
            total,
            page,
            size,
        }
    }

    #[test]
    fn is_last_true_on_final_page() {
        // 250 items, 100/page: page 3 (201..250) is the last page.
        assert!(page(250, 3, 100).is_last());
    }

    #[test]
    fn is_last_false_before_final_page() {
        // 250 items, 100/page: page 2 (101..200) is not yet last.
        assert!(!page(250, 2, 100).is_last());
    }

    #[test]
    fn is_last_true_when_total_is_exact_multiple() {
        // 200 items, 100/page: page 2 lands exactly on total (200 >= 200).
        assert!(page(200, 2, 100).is_last());
    }

    #[test]
    fn is_last_true_for_empty_listing() {
        // No items at all: page 1 of 0 is trivially last.
        assert!(page(0, 1, 100).is_last());
    }

    #[test]
    fn is_last_size_zero_with_total_zero_is_last() {
        // Degenerate size==0 but empty listing: complete.
        assert!(page(0, 1, 0).is_last());
    }

    #[test]
    fn is_last_size_zero_with_total_nonzero_is_not_last() {
        // Contract violation (size==0, total>0): must NOT report last, so the
        // HTTP layer's safety cap turns this into a loud Err instead of silent
        // truncation.
        assert!(!page(50, 1, 0).is_last());
        assert!(!page(50, 1_001, 0).is_last());
    }
}
