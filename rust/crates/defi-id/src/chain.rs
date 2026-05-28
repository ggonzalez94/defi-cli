//! Chain parsing: CAIP-2, numeric chain IDs, and the alias set. Scaffold stub.

/// A canonical chain reference.
///
/// Scaffold stub — populated in Phase 2 with namespace/reference and the alias
/// resolution logic from `internal/id`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chain {
    pub caip2: String,
}
