use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

/// doc coverage for a full create.
///
/// Sums up the file-coverages.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DocCoverage {
    /// The total items that could be documented in the current crate, used to calculate
    /// documentation coverage.
    pub total_items: i32,
    /// The items of the crate that are documented, used to calculate documentation coverage.
    pub documented_items: i32,
    /// The total items that could have code examples in the current crate, used to calculate
    /// documentation coverage.
    pub total_items_needing_examples: i32,
    /// The items of the crate that have a code example, used to calculate documentation coverage.
    pub items_with_examples: i32,
}

impl<'a> Extend<FileCoverage<'a>> for DocCoverage {
    fn extend<T: IntoIterator<Item = FileCoverage<'a>>>(&mut self, iter: T) {
        for fc in iter {
            self.total_items += fc.total;
            self.documented_items += fc.with_docs;
            self.total_items_needing_examples += fc.total_examples;
            self.items_with_examples += fc.with_examples;
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct FileCoverage<'a> {
    path: &'a str,
    total: i32,
    with_docs: i32,
    total_examples: i32,
    with_examples: i32,
}

#[derive(Deserialize)]
pub struct RawFileCoverage {
    total: i32,
    with_docs: i32,
    total_examples: i32,
    with_examples: i32,
}

type CoverageLine<'a> = HashMap<&'a str, RawFileCoverage>;

pub fn parse_line<'a>(line: &'a str) -> Result<impl Iterator<Item = FileCoverage<'a>> + 'a> {
    Ok(serde_json::from_str::<CoverageLine>(line)
        .map(|file_coverages| file_coverages.into_iter())?
        .map(|(path, raw)| FileCoverage {
            path,
            total: raw.total,
            with_docs: raw.with_docs,
            total_examples: raw.total_examples,
            with_examples: raw.with_examples,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_parse_line() {
        let data = serde_json::json!({
            "src/build.rs": {
                "total": 3,
                "with_docs": 3,
                "total_examples": 0,
                "with_examples": 0
            },
            "src/cmd/mod.rs": {
                "total": 41,
                "with_docs": 41,
                "total_examples": 25,
                "with_examples": 1
            },
        })
        .to_string();

        let mut result: Vec<_> = parse_line(&data).unwrap().collect();
        result.sort_unstable_by_key(|cov| cov.path);

        assert_eq!(
            result,
            vec![
                FileCoverage {
                    path: "src/build.rs",
                    total: 3,
                    with_docs: 3,
                    total_examples: 0,
                    with_examples: 0,
                },
                FileCoverage {
                    path: "src/cmd/mod.rs",
                    total: 41,
                    with_docs: 41,
                    total_examples: 25,
                    with_examples: 1,
                },
            ]
        );

        let mut sum = DocCoverage::default();
        sum.extend(result);

        assert_eq!(
            sum,
            DocCoverage {
                total_items: 44,
                documented_items: 44,
                total_items_needing_examples: 25,
                items_with_examples: 1,
            }
        );
    }

    #[test]
    fn test_parse_and_directly_sum_up() {
        let data = serde_json::json!({
            "src/build.rs": {
                "total": 3,
                "with_docs": 3,
                "total_examples": 0,
                "with_examples": 0
            },
            "src/cmd/mod.rs": {
                "total": 41,
                "with_docs": 41,
                "total_examples": 25,
                "with_examples": 1
            },
        })
        .to_string();

        let mut sum = DocCoverage::default();
        sum.extend(parse_line(&data).unwrap());

        assert_eq!(
            sum,
            DocCoverage {
                total_items: 44,
                documented_items: 44,
                total_items_needing_examples: 25,
                items_with_examples: 1,
            }
        );
    }
}
