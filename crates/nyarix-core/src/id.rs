//! Identifier types used throughout Nyarix.
//!
//! All identifiers are strongly-typed wrappers around UUIDv7 to
//! prevent mixing different ID domains at the type level.

use std::fmt;

/// A unique identifier for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(uuid::Uuid);

/// A unique identifier for a flow within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowId(uuid::Uuid);

/// A unique identifier for a stream within a flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamId(uuid::Uuid);

/// A unique identifier for an individual packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PacketId(uuid::Uuid);

/// A unique identifier for a module instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(uuid::Uuid);

/// A unique identifier for a node in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(uuid::Uuid);

/// A unique identifier for a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RouteId(uuid::Uuid);

macro_rules! impl_id {
    ($name:ident) => {
        impl $name {
            /// Generate a new unique identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(uuid::Uuid::now_v7())
            }

            /// Create from an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: uuid::Uuid) -> Self {
                Self(uuid)
            }

            /// Get the underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }

            /// Get the nil (zero) identifier.
            #[must_use]
            pub const fn nil() -> Self {
                Self(uuid::Uuid::nil())
            }

            /// Check if this is the nil identifier.
            #[must_use]
            pub fn is_nil(&self) -> bool {
                self.0.is_nil()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

impl_id!(SessionId);
impl_id!(FlowId);
impl_id!(StreamId);
impl_id!(PacketId);
impl_id!(ModuleId);
impl_id!(NodeId);
impl_id!(RouteId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn nil_is_consistent() {
        assert!(SessionId::nil().is_nil());
        assert!(!SessionId::new().is_nil());
    }
}
