pub(crate) fn args_contain_unstable_feature<S>(
    cargo_args: impl IntoIterator<Item = S>,
    feature: &str,
) -> bool
where
    S: AsRef<str>,
{
    let mut cargo_args = cargo_args.into_iter().peekable();

    while let Some(arg) = cargo_args.next() {
        let arg = arg.as_ref();
        if arg
            .strip_prefix("-Z")
            .is_some_and(|value| unstable_feature_matches(value, feature))
        {
            return true;
        }

        if arg == "-Z"
            && cargo_args
                .peek()
                .is_some_and(|next| unstable_feature_matches(next.as_ref(), feature))
        {
            return true;
        }
    }

    false
}

fn unstable_feature_matches(value: &str, feature: &str) -> bool {
    value == feature
        || value
            .strip_prefix(feature)
            .is_some_and(|suffix| suffix.starts_with('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_build_std_spellings() {
        assert!(args_contain_unstable_feature(
            ["-Zbuild-std=core"],
            "build-std"
        ));
        assert!(args_contain_unstable_feature(
            ["-Z", "build-std"],
            "build-std"
        ));
        assert!(args_contain_unstable_feature(
            ["-Z", "build-std=core,alloc"],
            "build-std"
        ));
        assert!(!args_contain_unstable_feature(
            ["-Zunstable-options"],
            "build-std"
        ));
        assert!(!args_contain_unstable_feature(
            ["-Zbuild-stdlib"],
            "build-std"
        ));

        let owned = vec!["-Z".to_string(), "build-std=core".to_string()];
        assert!(args_contain_unstable_feature(&owned, "build-std"));
        assert!(args_contain_unstable_feature(owned, "build-std"));
    }
}
