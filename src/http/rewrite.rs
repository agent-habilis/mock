//! Request-path rewriting applied before mock lookup and proxying.
//!
//! A rule is a `(prefix, replacement)` pair. The first rule whose prefix matches
//! a leading **path segment** of the request path wins; the matched prefix is
//! swapped for the replacement and the (possibly empty) remainder of the path is
//! preserved. Matching is segment-aware: prefix `/api` matches `/api` and
//! `/api/...` but not `/apidocs`. Any query string is split off first and
//! re-attached unchanged, so rewriting only ever touches the path portion.

/// Rewrite the path portion of `uri` using the first matching prefix rule,
/// re-attaching any query string. Returns `uri` unchanged when no rule matches
/// (and short-circuits the common case where `rules` is empty unless
/// `--rewrite-path` was given).
pub(crate) fn rewrite_path(uri: &str, rules: &[(String, String)]) -> String {
    if rules.is_empty() {
        return uri.to_string();
    }

    let (path, query) = match uri.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (uri, None),
    };

    // Segment-aware prefix match: `prefix` must equal the path or be followed by
    // a `/`. The parser guarantees `prefix` starts with `/` and has no trailing
    // `/`, so `prefix.len()` is always a valid (char-boundary) split point.
    let rewritten = rules
        .iter()
        .find(|(prefix, _)| path == prefix || path.starts_with(&format!("{prefix}/")))
        .map_or_else(
            || path.to_string(),
            |(prefix, replacement)| format!("{replacement}{}", &path[prefix.len()..]),
        );

    match query {
        Some(query) => format!("{rewritten}?{query}"),
        None => rewritten,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rules(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(prefix, replacement)| ((*prefix).to_string(), (*replacement).to_string()))
            .collect()
    }

    #[test]
    fn no_rules_is_identity() {
        assert_eq!(rewrite_path("/api/x?a=1", &[]), "/api/x?a=1");
    }

    #[test]
    fn exact_prefix_match_yields_bare_replacement() {
        let rules = rules(&[("/api/federated-gateway-public/graphql", "/graphql")]);
        assert_eq!(
            rewrite_path("/api/federated-gateway-public/graphql", &rules),
            "/graphql"
        );
    }

    #[test]
    fn matched_prefix_keeps_trailing_path_and_query() {
        let rules = rules(&[("/api/federated-gateway-public/graphql", "/graphql")]);
        assert_eq!(
            rewrite_path("/api/federated-gateway-public/graphql/x?op=foo", &rules),
            "/graphql/x?op=foo"
        );
    }

    #[test]
    fn first_match_wins() {
        // The broader `/api` rule comes second, so the more specific gateway rule
        // shadows it for the gateway path.
        let rules = rules(&[
            ("/api/federated-gateway-public/graphql", "/graphql"),
            ("/api", "/other"),
        ]);
        assert_eq!(
            rewrite_path("/api/federated-gateway-public/graphql", &rules),
            "/graphql"
        );
        // A path only the second rule matches still gets rewritten by it.
        assert_eq!(rewrite_path("/api/health", &rules), "/other/health");
    }

    #[test]
    fn no_matching_rule_returns_input_verbatim() {
        let rules = rules(&[("/api", "/other")]);
        assert_eq!(
            rewrite_path("/static/app.js?v=2", &rules),
            "/static/app.js?v=2"
        );
    }

    #[test]
    fn matching_is_segment_aware_not_substring() {
        let rules = rules(&[("/api", "/other")]);
        // A sibling path that merely shares the textual prefix is NOT rewritten…
        assert_eq!(
            rewrite_path("/apidocs/index.html", &rules),
            "/apidocs/index.html"
        );
        // …but an exact match and a real sub-segment are.
        assert_eq!(rewrite_path("/api", &rules), "/other");
        assert_eq!(rewrite_path("/api/users", &rules), "/other/users");
    }

    proptest! {
        #![proptest_config(crate::proptest_support::config())]

        /// With no rules, any uri is returned unchanged (the existing-user path).
        #[test]
        fn prop_empty_rules_is_identity(uri in ".*") {
            prop_assert_eq!(rewrite_path(&uri, &[]), uri);
        }

        /// A matching rule rewrites the path correctly AND preserves the query
        /// verbatim, even when the query itself contains a `?`. The rule's prefix
        /// is always a full leading segment of the generated path, so it fires.
        #[test]
        fn prop_matching_rule_rewrites_path_and_preserves_query(
            // `seg` is the matched first segment; `rest` is the remainder.
            seg in "[a-z0-9]{1,10}",
            rest in "(/[a-z0-9]{1,10}){0,4}",
            query in "[a-z0-9=&?]{0,40}",
            replacement in "/[a-z0-9/-]{1,20}",
        ) {
            let path = format!("/{seg}{rest}");
            let uri = format!("{path}?{query}");
            let rules = vec![(format!("/{seg}"), replacement.clone())];
            let result = rewrite_path(&uri, &rules);
            // Path: `/seg` swapped for `replacement`, `rest` kept; query verbatim.
            prop_assert_eq!(result, format!("{replacement}{rest}?{query}"));
        }

        /// The no-query branch is exercised independently and never panics.
        #[test]
        fn prop_no_query_branch_rewrites_path(
            seg in "[a-z0-9]{1,10}",
            rest in "(/[a-z0-9]{1,10}){0,4}",
            replacement in "/[a-z0-9/-]{1,20}",
        ) {
            let path = format!("/{seg}{rest}");
            let rules = vec![(format!("/{seg}"), replacement.clone())];
            prop_assert_eq!(rewrite_path(&path, &rules), format!("{replacement}{rest}"));
        }
    }
}
