use craft_anything_js::{Limits, ParseMode, analyze};

/// Reading or assigning a hook property does not invoke a procedure.
#[test]
fn property_reads_are_not_client_calls() {
    for source in ["const hook = trpc.users.list.useQuery;", "trpc.users.list.useMutation = replacement;", "inspect(trpc.users.list.useQuery);"] {
        let a = analyze("app.js", source, &Limits { max_parse_bytes: 0, ..Limits::default() });
        assert_eq!(a.facts.mode, ParseMode::Lexer);
        assert!(a.facts.api_operations.is_empty(), "{source}: {:?}", a.facts.api_operations);
    }
}

/// Ordinary hook calls still produce the procedure identity through comments.
#[test]
fn calls_keep_the_procedure_name() {
    for source in ["trpc.users.list.useQuery();", "trpc.users.list.useQuery /* note */ ();", "trpc.users.list.useQuery?.();"] {
        let a = analyze("app.js", source, &Limits { max_parse_bytes: 0, ..Limits::default() });
        assert!(a.facts.api_operations.iter().any(|o| o.name.as_deref() == Some("users.list")), "{source}");
    }
}
