//! Schema catalog with authenticated registration (R8, R10, R11, R12).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use daemoneye_lib::detection::catalog::{
    CatalogError, SchemaCatalog, TokenRejection, VerifiedRegistration, verify_spawn_token,
};
use daemoneye_lib::detection_bounds::{
    MAX_COLUMNS_PER_TABLE, MAX_IDENTIFIER_LENGTH, MAX_TABLES_PER_DESCRIPTOR,
};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};

/// A 64-hex-character token of the shape the agent issues.
fn token(last: char) -> String {
    let mut value = "a".repeat(63);
    value.push(last);
    value
}

fn column(name: &str, ops: &[PredicateOp]) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(ColumnType::String),
        nullable: false,
        supported_ops: ops.iter().copied().map(i32::from).collect(),
    }
}

fn descriptor(collector_id: &str, columns: Vec<ColumnDescriptor>) -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: collector_id.to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns,
        }],
    }
}

#[test]
fn a_registration_with_no_token_is_rejected_by_the_no_token_gate() {
    let error = verify_spawn_token("procmond", Some(&token('a')), None).unwrap_err();
    assert_eq!(error, TokenRejection::NoTokenPresented);
}

#[test]
fn a_token_issued_to_a_different_collector_is_rejected_by_the_mismatch_gate() {
    let error = verify_spawn_token("procmond", Some(&token('a')), Some(&token('b'))).unwrap_err();
    assert_eq!(error, TokenRejection::TokenMismatch);
}

#[test]
fn a_collector_with_no_issued_token_is_rejected_by_the_unknown_collector_gate() {
    let error = verify_spawn_token("ghostmond", None, Some(&token('a'))).unwrap_err();
    assert_eq!(error, TokenRejection::UnknownCollector);
}

#[test]
fn a_token_differing_only_in_its_final_byte_is_rejected() {
    let mut presented = token('a');
    presented.pop();
    presented.push('b');
    let error = verify_spawn_token("procmond", Some(&token('a')), Some(&presented)).unwrap_err();
    assert_eq!(error, TokenRejection::TokenMismatch);
}

#[test]
fn a_correct_token_yields_a_proof_bound_to_the_collector_identity() {
    let verified: VerifiedRegistration =
        verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    assert_eq!(verified.collector_id(), "procmond");
}

#[test]
fn a_verified_descriptor_becomes_visible_to_lookup() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    catalog
        .register(
            &verified,
            descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]),
        )
        .unwrap();

    assert!(catalog.table("processes").is_some());
    assert!(catalog.column("processes", "name").is_some());
    assert!(catalog.column("processes", "absent").is_none());
}

#[test]
fn a_descriptor_claiming_another_identity_is_rejected() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    let error = catalog
        .register(
            &verified,
            descriptor("netmond", vec![column("name", &[PredicateOp::Eq])]),
        )
        .unwrap_err();
    assert!(matches!(error, CatalogError::IdentityMismatch { .. }));
    assert!(catalog.table("processes").is_none());
}

#[test]
fn a_descriptor_over_the_table_bound_is_rejected_before_storage() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    let mut over = descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]);
    over.tables = (0..=MAX_TABLES_PER_DESCRIPTOR)
        .map(|index| TableDescriptor {
            name: format!("t{index}"),
            columns: vec![column("name", &[PredicateOp::Eq])],
        })
        .collect();

    let error = catalog.register(&verified, over).unwrap_err();
    assert!(matches!(error, CatalogError::TooManyTables { .. }));
    assert!(catalog.table("t0").is_none());
}

#[test]
fn a_descriptor_over_the_column_bound_is_rejected_before_storage() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    let columns = (0..=MAX_COLUMNS_PER_TABLE)
        .map(|index| column(&format!("c{index}"), &[PredicateOp::Eq]))
        .collect();

    let error = catalog
        .register(&verified, descriptor("procmond", columns))
        .unwrap_err();
    assert!(matches!(error, CatalogError::TooManyColumns { .. }));
    assert!(catalog.table("processes").is_none());
}

#[test]
fn a_descriptor_with_an_overlong_identifier_is_rejected_before_storage() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    let long = "n".repeat(MAX_IDENTIFIER_LENGTH + 1);

    let error = catalog
        .register(
            &verified,
            descriptor("procmond", vec![column(&long, &[PredicateOp::Eq])]),
        )
        .unwrap_err();
    assert!(matches!(error, CatalogError::IdentifierTooLong { .. }));
    assert!(catalog.table("processes").is_none());
}

#[test]
fn an_unknown_reference_error_names_the_reference() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    catalog
        .register(
            &verified,
            descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]),
        )
        .unwrap();

    let table_error = catalog
        .resolve_reference("network.connections", "remote_ip")
        .unwrap_err();
    assert!(table_error.to_string().contains("network.connections"));

    let column_error = catalog
        .resolve_reference("processes", "cmdline")
        .unwrap_err();
    assert!(column_error.to_string().contains("cmdline"));
}

#[test]
fn an_advertised_operation_is_not_pushable_until_conformance_passes() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    catalog
        .register(
            &verified,
            descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]),
        )
        .unwrap();

    assert!(catalog.is_advertised("processes", "name", PredicateOp::Eq));
    assert!(!catalog.is_pushable("processes", "name", PredicateOp::Eq));

    catalog.record_conformance_pass("procmond", "processes", "name", PredicateOp::Eq);
    assert!(catalog.is_pushable("processes", "name", PredicateOp::Eq));
}

#[test]
fn re_registration_reports_the_columns_a_descriptor_dropped() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    catalog
        .register(
            &verified,
            descriptor(
                "procmond",
                vec![
                    column("name", &[PredicateOp::Eq]),
                    column("cmdline", &[PredicateOp::Eq]),
                ],
            ),
        )
        .unwrap();

    let change = catalog
        .register(
            &verified,
            descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]),
        )
        .unwrap();

    assert!(!change.is_first_registration());
    assert!(
        change
            .removed_references()
            .contains(&("processes".to_owned(), "cmdline".to_owned()))
    );
}

#[test]
fn a_first_registration_is_reported_as_such() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    let change = catalog
        .register(
            &verified,
            descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]),
        )
        .unwrap();
    assert!(change.is_first_registration());
}

#[test]
fn a_changed_descriptor_drops_the_collectors_conformance_passes() {
    // A re-registered descriptor is a new descriptor_version, and a conformance result is bound to
    // the version it was produced against — an operation whose semantics changed must not carry a
    // stale pass forward.
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    catalog
        .register(
            &verified,
            descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]),
        )
        .unwrap();
    catalog.record_conformance_pass("procmond", "processes", "name", PredicateOp::Eq);
    assert!(catalog.is_pushable("processes", "name", PredicateOp::Eq));

    let mut upgraded = descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]);
    upgraded.descriptor_version = "v2".to_owned();
    upgraded.tables[0]
        .columns
        .push(column("pid", &[PredicateOp::Eq]));
    let _change = catalog.register(&verified, upgraded).unwrap();

    assert!(catalog.is_advertised("processes", "name", PredicateOp::Eq));
    assert!(
        !catalog.is_pushable("processes", "name", PredicateOp::Eq),
        "a changed descriptor must not carry its predecessor's conformance passes"
    );
}

#[test]
fn an_unchanged_re_registration_keeps_its_conformance_passes() {
    let mut catalog = SchemaCatalog::new();
    let verified = verify_spawn_token("procmond", Some(&token('a')), Some(&token('a'))).unwrap();
    let same = || descriptor("procmond", vec![column("name", &[PredicateOp::Eq])]);
    catalog.register(&verified, same()).unwrap();
    catalog.record_conformance_pass("procmond", "processes", "name", PredicateOp::Eq);

    let _change = catalog.register(&verified, same()).unwrap();

    assert!(catalog.is_pushable("processes", "name", PredicateOp::Eq));
}
