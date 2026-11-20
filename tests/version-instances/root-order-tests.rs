// These tests are compiled with verbatim functions extracted from production instances.rs.
use std::path::Path;

fn values(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn root_normalization_never_sorts_names() {
    let initial = values(&["X:/order/z-old", "X:/order/default", "X:/order/a-new"]);
    let expected = values(&["X:/order/default", "X:/order/z-old", "X:/order/a-new"]);
    assert_eq!(ordered_roots(&initial, &[], Some("X:/order/default")), expected);
    assert_eq!(ordered_roots(&expected, &[], Some("X:/order/default")), expected);
    assert_eq!(ordered_roots(&expected, &expected, Some("X:/order/default")), expected);
}

#[test]
fn previous_addition_order_wins_over_incoming_activation_order() {
    let previous = values(&["X:/order/default", "X:/order/z-old", "X:/order/a-new"]);
    let requested = values(&["X:/order/b-last", "X:/order/a-new", "X:/order/default", "X:/order/z-old"]);
    let expected = values(&["X:/order/default", "X:/order/z-old", "X:/order/a-new", "X:/order/b-last"]);
    assert_eq!(ordered_roots(&requested, &previous, Some("X:/order/default")), expected);
    for _ in 0..20 {
        assert_eq!(ordered_roots(&requested, &expected, Some("X:/order/default")), expected);
    }
}

#[test]
fn removal_then_readdition_is_a_new_append() {
    let previous = values(&["X:/order/default", "X:/order/z-old", "X:/order/a-new"]);
    let removed = ordered_roots(&values(&["X:/order/a-new", "X:/order/default"]), &previous, Some("X:/order/default"));
    assert_eq!(removed, values(&["X:/order/default", "X:/order/a-new"]));
    let requested = values(&["X:/order/z-old", "X:/order/default", "X:/order/a-new"]);
    assert_eq!(ordered_roots(&requested, &removed, Some("X:/order/default")), values(&["X:/order/default", "X:/order/a-new", "X:/order/z-old"]));
    assert!(ordered_roots(&[], &previous, Some("X:/order/default")).is_empty());
}

#[test]
fn windows_aliases_deduplicate_and_preserve_earliest_identity() {
    let incoming = values(&["X:/order/z-old", "x:\\ORDER\\Z-OLD\\", r"\\?\X:\order\z-old", "X:/order/default", "X:/order/a-new"]);
    assert_eq!(ordered_roots(&incoming, &[], Some("x:\\ORDER\\DEFAULT\\")), values(&["X:/order/default", "X:/order/z-old", "X:/order/a-new"]));
    assert_eq!(root_identity(" X:/ORDER/z-old/// "), root_identity(r"\\?\x:\order\Z-OLD"));
    let unc = values(&["//server/share/z-old", "\\\\SERVER\\SHARE\\Z-OLD\\", "//server/share/a-new"]);
    assert_eq!(ordered_roots(&unc, &[], None), values(&["//server/share/z-old", "//server/share/a-new"]));
}


#[cfg(windows)]
#[test]
fn extended_unc_aliases_preserve_default_and_previous_order() {
    let aliases = values(&[
        "//server/share/z-old",
        "//server/share/default",
        "//server/share/a-new",
        r"\\?\UNC\SERVER\SHARE\Z-OLD\",
        r"\\?\unc\server\share\default\",
    ]);
    let expected = values(&["//server/share/default", "//server/share/z-old", "//server/share/a-new"]);
    assert_eq!(root_identity(r"\\?\UNC\SERVER\SHARE\Z-OLD\"), root_identity("//server/share/z-old"));
    assert_eq!(root_identity(r"\\?\unc\server\share\default\"), root_identity("//server/share/default"));
    assert_eq!(ordered_roots(&aliases, &[], Some(r"\\?\unc\SERVER\SHARE\DEFAULT\")), expected);
    let incoming = values(&[
        r"\\?\UNC\server\share\a-new",
        r"\\?\UNC\server\share\b-last",
        r"\\?\UNC\server\share\z-old",
        r"\\?\UNC\server\share\default",
    ]);
    assert_eq!(
        ordered_roots(&incoming, &expected, Some("//server/share/default")),
        vec![incoming[3].clone(), incoming[2].clone(), incoming[0].clone(), incoming[1].clone()]
    );
}

#[test]
fn previous_order_matches_aliases_even_when_spelling_changes() {
    let previous = values(&["X:/order/default", "X:/order/z-old", "X:/order/a-new"]);
    let incoming = values(&["x:\\order\\a-new", "x:\\order\\b-last", "x:\\order\\z-old\\", "x:\\order\\default"]);
    assert_eq!(ordered_roots(&incoming, &previous, Some("X:/order/default")), values(&["x:\\order\\default", "x:\\order\\z-old\\", "x:\\order\\a-new", "x:\\order\\b-last"]));
}

#[test]
fn portable_effective_roots_use_real_temporary_directories() {
    let fixture = std::env::var("NICKEL_ROOT_ORDER_TEMP").expect("isolated directory required");
    let root = Path::new(&fixture);
    let portable = root.join("portable");
    let oldest = root.join("z-old");
    let younger = root.join("a-new");
    let latest = root.join("b-last");
    for directory in [&portable, &oldest, &younger, &latest] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let portable = portable.to_string_lossy().into_owned();
    let oldest = oldest.to_string_lossy().into_owned();
    let younger = younger.to_string_lossy().into_owned();
    let latest = latest.to_string_lossy().into_owned();
    let missing = root.join("missing").to_string_lossy().into_owned();
    let original = vec![oldest.clone(), missing.clone(), younger.clone(), portable.to_uppercase()];
    let mut effective = original.clone();
    effective.retain(|path| Path::new(path).is_dir() && path != &portable);
    effective.insert(0, portable.clone());
    let expected = vec![portable.clone(), oldest.clone(), younger.clone()];
    let ordered = ordered_roots(&effective, &[], Some(&portable));
    assert_eq!(ordered, expected);
    assert!(!ordered.iter().any(|path| path == &missing));
    let requested = vec![latest.clone(), younger.clone(), portable.clone(), oldest.clone()];
    let appended = ordered_roots(&requested, &original, Some(&portable));
    assert_eq!(appended, vec![portable.clone(), oldest, younger, latest]);
    assert_eq!(ordered_roots(&appended, &[], Some(&portable)), appended);
}

#[test]
fn default_position_is_the_only_exception_to_previous_order() {
    let previous = values(&["X:/order/z-old", "X:/order/a-new"]);
    let requested = values(&["X:/order/a-new", "X:/order/default", "X:/order/z-old"]);
    assert_eq!(ordered_roots(&requested, &previous, Some("X:/order/default")), values(&["X:/order/default", "X:/order/z-old", "X:/order/a-new"]));
    assert_eq!(ordered_roots(&previous, &[], None), previous);
}
