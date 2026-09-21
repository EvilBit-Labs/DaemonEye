//! The gate every collector registration passes through (R8, R10, R11, R12).
//!
//! This is the composition point for three things that live in two sibling crates with no
//! dependency edge between them: `daemoneye-eventbus` mints the spawn token and owns the wire
//! shape of a registration, `daemoneye-lib` owns the constant-time verifier, the catalog, and rule
//! health. The agent sees both, so the conversion from the eventbus serde descriptor to the prost
//! descriptor the catalog stores happens here, once.
//!
//! The ordering is the security property. A descriptor is converted only after the token verified,
//! bounds-checked only after conversion, and stored only after both — so an unauthenticated or
//! pathological descriptor never reaches the catalog, and a refused one triggers no re-planning.

use std::collections::BTreeSet;
use std::sync::Arc;

use daemoneye_eventbus::process_manager::spawn_token::SpawnTokenStore;
use daemoneye_eventbus::rpc::{
    ColumnDescriptor as WireColumn, ConformanceResult as WireConformanceResult,
    RegistrationRequest, SchemaDescriptor as WireDescriptor,
};
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::{
    CatalogError, TokenRejection, VerifiedRegistration, verify_spawn_token,
};
use daemoneye_lib::proto::{
    ColumnDescriptor, ConformanceResult, SchemaDescriptor, TableDescriptor,
};
use daemoneye_lib::rejection_log::RegistrationGate;
use thiserror::Error;
use tokio::sync::Mutex;

/// Why a registration was not admitted.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AdmissionError {
    /// The spawn token did not authenticate the presented identity.
    #[error("{0}")]
    Token(TokenRejection),
    /// The descriptor was authenticated but refused before storage.
    #[error("{0}")]
    Descriptor(CatalogError),
}

/// Map an admission failure onto the gate that refused it.
///
/// One `const fn`, because `daemoneye-eventbus` cannot name [`RegistrationGate`] and
/// `daemoneye-lib`'s catalog does not know the registry's vocabulary. Keeping the mapping here and
/// total is what lets a test assert *which* gate fired rather than only that admission failed.
pub const fn gate_for_admission(error: &AdmissionError) -> RegistrationGate {
    match *error {
        AdmissionError::Token(rejection) => match rejection {
            TokenRejection::NoTokenPresented => RegistrationGate::NoTokenPresented,
            TokenRejection::UnknownCollector => RegistrationGate::UnknownCollector,
            TokenRejection::TokenMismatch => RegistrationGate::TokenMismatch,
            // `TokenRejection` is `#[non_exhaustive]`. A variant this build does not recognize is
            // still a refusal, and the safe thing to record is a refusal rather than to widen the
            // meaning of a more specific gate.
            _unrecognized => RegistrationGate::TokenMismatch,
        },
        // A descriptor that fails a bound or claims the wrong identity is a malformed request:
        // the token itself was fine, so none of the token gates describes it.
        AdmissionError::Descriptor(_) => RegistrationGate::MalformedRequest,
    }
}

/// Authenticates registrations and feeds admitted descriptors to the one detection engine.
///
/// The gate owns no catalog and no rule health of its own. A second copy of either would be
/// invisible: registrations would land in one and the planner would read the other, so every rule
/// would defer forever under R18 while every unit test still passed.
#[derive(Debug)]
pub struct CollectorAdmission {
    tokens: Arc<SpawnTokenStore>,
    /// The engine that owns the catalog, rule health, the compiled plans and the task ledger.
    ///
    /// A `tokio` mutex, not a `std` one: the renewal cycle that shares this engine is `async`, and
    /// a synchronous guard held across its sends would trip `clippy::await_holding_lock`.
    engine: Arc<Mutex<DetectionEngine>>,
    /// Collectors that registered since the last renewal tick and are owed their task set (R16).
    ///
    /// Recorded rather than sent here: admission has no dispatch, and keeping every IPC send in
    /// the renewal loop keeps one place responsible for the wire.
    pending_reissue: Mutex<BTreeSet<String>>,
}

impl CollectorAdmission {
    /// Build an admission gate over the token store the process manager mints into and the engine
    /// the planner reads.
    ///
    /// The *same* [`SpawnTokenStore`] must back both, or nothing this agent spawned will ever
    /// authenticate; [`Self::shares_token_store_with`] is the startup check for that. The same
    /// holds for the engine, checked by [`Self::shares_engine_with`].
    #[must_use]
    pub fn new(tokens: Arc<SpawnTokenStore>, engine: Arc<Mutex<DetectionEngine>>) -> Self {
        Self {
            tokens,
            engine,
            pending_reissue: Mutex::new(BTreeSet::new()),
        }
    }

    /// Whether this gate verifies against exactly the store `other` mints into.
    ///
    /// A composition root asserts this at startup. Two stores that merely look alike issue tokens
    /// that never verify against each other, and the failure would otherwise surface as every
    /// collector mysteriously failing to register — the same class of silent mis-wiring that made
    /// `--compute-hashes` a no-op (see
    /// `docs/solutions/security-issues/binary-hashing-authorization-and-toctou-fixes.md`).
    #[must_use]
    pub fn shares_token_store_with(&self, other: &Arc<SpawnTokenStore>) -> bool {
        Arc::ptr_eq(&self.tokens, other)
    }

    /// Whether this gate feeds exactly the engine `other` plans against.
    ///
    /// The engine equivalent of [`Self::shares_token_store_with`], and asserted at startup for the
    /// same reason: two engines fail silently, with every rule deferring forever.
    #[must_use]
    pub fn shares_engine_with(&self, other: &Arc<Mutex<DetectionEngine>>) -> bool {
        Arc::ptr_eq(&self.engine, other)
    }

    /// Take the collectors owed a task re-issue, clearing the set (R16).
    pub async fn drain_pending_reissue(&self) -> Vec<String> {
        let mut pending = self.pending_reissue.lock().await;
        std::mem::take(&mut *pending).into_iter().collect()
    }

    /// Authenticate a registration and, if it carries one, admit its descriptor into the engine.
    ///
    /// Nothing is returned beyond success: `DetectionEngine::register_collector` already drains
    /// the rules deferred under R18, re-validates rule health and re-plans everything the change
    /// unblocked. Handing a `ReplanOutcome` back here would invite a caller to re-plan a second
    /// time against a catalog that has already moved.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError`] naming the specific gate that refused the registration.
    pub async fn admit(&self, request: &RegistrationRequest) -> Result<(), AdmissionError> {
        let verified = self.verify(request)?;

        // An authenticated collector that advertises nothing widens nothing. It is not an error:
        // registration and schema advertisement are separate facts. It is still a re-registration,
        // so it is still owed its task set.
        if let Some(ref wire) = request.descriptor {
            let descriptor = to_proto_descriptor(wire);
            let mut engine = self.engine.lock().await;
            let _change = engine
                .register_collector(&verified, descriptor)
                .map_err(AdmissionError::Descriptor)?;
            drop(engine);
        }

        let mut pending = self.pending_reissue.lock().await;
        let _first = pending.insert(request.collector_id.clone());
        drop(pending);
        Ok(())
    }

    /// Verify the presented spawn token against the one issued for this identity (R8).
    fn verify(
        &self,
        request: &RegistrationRequest,
    ) -> Result<VerifiedRegistration, AdmissionError> {
        let issued = self.tokens.expected_token(&request.collector_id);
        verify_spawn_token(
            &request.collector_id,
            issued.as_deref(),
            request.spawn_token.as_deref(),
        )
        .map_err(AdmissionError::Token)
    }
}

/// Convert the eventbus serde descriptor into the prost descriptor the catalog stores.
///
/// The two are U1's deliberate mirror of one another, so this is a field-for-field copy. Enum
/// values cross as their `i32` discriminants, which are the same numbers on both sides because
/// both are generated from the same `common.proto` enum.
fn to_proto_descriptor(wire: &WireDescriptor) -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: wire.collector_id.clone(),
        descriptor_version: wire.descriptor_version.clone(),
        tables: wire
            .tables
            .iter()
            .map(|table| TableDescriptor {
                name: table.name.clone(),
                columns: table.columns.iter().map(to_proto_column).collect(),
            })
            .collect(),
        conformance_results: wire
            .conformance_results
            .iter()
            .map(to_proto_conformance_result)
            .collect(),
    }
}

/// Convert one conformance-vector result across the mirror (R22).
///
/// The results cross with the descriptor rather than in a separate call, because the catalog binds
/// a result to the `descriptor_version` it was produced against and drops every result a collector
/// held whenever its descriptor changes.
fn to_proto_conformance_result(wire: &WireConformanceResult) -> ConformanceResult {
    ConformanceResult {
        table: wire.table.clone(),
        column: wire.column.clone(),
        op: op_value(wire.op),
        passed: wire.passed,
    }
}

/// Convert one column descriptor across the mirror.
fn to_proto_column(wire: &WireColumn) -> ColumnDescriptor {
    ColumnDescriptor {
        name: wire.name.clone(),
        column_type: column_type_value(wire.column_type),
        nullable: wire.nullable,
        supported_ops: wire.supported_ops.iter().map(|op| op_value(*op)).collect(),
    }
}

/// Wire value of a mirrored column type.
fn column_type_value(column_type: daemoneye_eventbus::rpc::ColumnType) -> i32 {
    use daemoneye_eventbus::rpc::ColumnType as Wire;
    use daemoneye_lib::proto::ColumnType as Proto;

    let mapped = match column_type {
        Wire::Unspecified => Proto::Unspecified,
        Wire::String => Proto::String,
        Wire::Int => Proto::Int,
        Wire::Uint => Proto::Uint,
        Wire::Float => Proto::Float,
        Wire::Bool => Proto::Bool,
        // `ColumnType` is `#[non_exhaustive]`; an unrecognized type carries no claim.
        _unrecognized => Proto::Unspecified,
    };
    i32::from(mapped)
}

/// Wire value of a mirrored predicate operation.
fn op_value(op: daemoneye_eventbus::rpc::PredicateOp) -> i32 {
    use daemoneye_eventbus::rpc::PredicateOp as Wire;
    use daemoneye_lib::proto::PredicateOp as Proto;

    let mapped = match op {
        Wire::Unspecified => Proto::Unspecified,
        Wire::Eq => Proto::Eq,
        Wire::Ne => Proto::Ne,
        Wire::Lt => Proto::Lt,
        Wire::Le => Proto::Le,
        Wire::Gt => Proto::Gt,
        Wire::Ge => Proto::Ge,
        Wire::In => Proto::In,
        Wire::Like => Proto::Like,
        Wire::Regexp => Proto::Regexp,
        // `PredicateOp` is `#[non_exhaustive]`; an unrecognized op is not pushable.
        _unrecognized => Proto::Unspecified,
    };
    i32::from(mapped)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::to_proto_descriptor;
    use daemoneye_eventbus::rpc::{
        ColumnDescriptor as WireColumn, ColumnType as WireColumnType,
        ConformanceResult as WireResult, PredicateOp as WireOp, SchemaDescriptor as WireDescriptor,
        TableDescriptor as WireTable,
    };
    use daemoneye_lib::proto::PredicateOp;

    /// The only hop between the collector that produces a conformance result and the catalog that
    /// records it. A field dropped here is silent: every operation simply stops being pushable.
    #[test]
    fn conformance_results_cross_the_mirror_with_their_operation_and_verdict() {
        // Arrange
        let wire = WireDescriptor {
            collector_id: "procmond".to_owned(),
            descriptor_version: "processes-v1".to_owned(),
            tables: vec![WireTable {
                name: "processes".to_owned(),
                columns: vec![WireColumn {
                    name: "name".to_owned(),
                    column_type: WireColumnType::String,
                    nullable: false,
                    supported_ops: vec![WireOp::Like],
                }],
            }],
            conformance_results: vec![
                WireResult {
                    table: "processes".to_owned(),
                    column: "name".to_owned(),
                    op: WireOp::Like,
                    passed: true,
                },
                WireResult {
                    table: "processes".to_owned(),
                    column: "name".to_owned(),
                    op: WireOp::Regexp,
                    passed: false,
                },
            ],
        };

        // Act
        let proto = to_proto_descriptor(&wire);

        // Assert
        assert_eq!(proto.conformance_results.len(), 2);
        let passed = proto.conformance_results.first().unwrap();
        assert_eq!(passed.table, "processes");
        assert_eq!(passed.column, "name");
        assert_eq!(passed.op(), PredicateOp::Like);
        assert!(passed.passed);
        let failed = proto.conformance_results.last().unwrap();
        assert_eq!(failed.op(), PredicateOp::Regexp);
        assert!(!failed.passed, "a failure must cross as a failure");
    }
}
