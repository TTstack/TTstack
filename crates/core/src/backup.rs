//! One published disk recovery point, with durable operations and bounded retirement.
use serde::{Deserialize, Serialize};

pub const MAX_RETIRED: usize = 2;

pub fn token() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Zvol,
    Reflink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Create,
    Restore,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation_id: String,
    pub expected_revision: String,
    pub action: Action,
    pub generation: Option<String>,
}

impl Request {
    pub fn from_headers(
        key: Option<&str>,
        revision: Option<&str>,
        action: Action,
        generation: Option<String>,
    ) -> Result<Self, String> {
        let key = key.ok_or("missing precondition: Idempotency-Key required")?;
        let revision = revision.ok_or("missing precondition: If-Match required")?;
        let revision = revision
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or("invalid If-Match; use the quoted backup revision")?;
        let request = Self {
            operation_id: key.into(),
            expected_revision: revision.into(),
            action,
            generation,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), String> {
        for value in [&self.operation_id, &self.expected_revision] {
            uuid::Uuid::parse_str(value).map_err(|_| "invalid backup operation ID or revision")?;
        }
        match (&self.action, &self.generation) {
            (Action::Restore, Some(id)) => {
                uuid::Uuid::parse_str(id).map_err(|_| "invalid backup generation")?;
            }
            (Action::Restore, None) => return Err("restore requires a generation".into()),
            (_, Some(_)) => return Err("generation is only valid for restore".into()),
            _ => (),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    pub id: String,
    pub backend: Backend,
    pub root_bytes: u64,
    pub created_at: u64,
    /// ZFS GUID or file device/inode, never a caller-supplied path.
    pub identity: String,
    pub dependencies: String,
    /// A restore's temporary file may already have moved into the active path.
    pub restore_staging: bool,
}

impl Generation {
    pub fn reserved_mib(&self) -> u32 {
        u32::try_from(self.root_bytes.div_ceil(1024 * 1024)).unwrap_or(u32::MAX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub request: Request,
    pub artifact: Generation,
    pub disk_identity: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub request: Request,
    pub completed_at: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub revision: String,
    pub sequence: u64,
    pub current: Option<Generation>,
    pub pending: Option<Pending>,
    pub retired: Vec<Generation>,
    pub last_result: Option<Receipt>,
    pub cleanup_error: Option<String>,
    pub cleanup_retry_at: u64,
    pub cleanup_failures: u32,
    /// Controller-only forwarding intent, retained across unknown outcomes.
    pub forwarding: Option<Request>,
    pub forwarding_reserved_mib: u32,
    #[serde(default)]
    pub forwarding_settled_sequence: Option<u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            revision: token(),
            sequence: 0,
            current: None,
            pending: None,
            retired: vec![],
            last_result: None,
            cleanup_error: None,
            cleanup_retry_at: 0,
            cleanup_failures: 0,
            forwarding: None,
            forwarding_reserved_mib: 0,
            forwarding_settled_sequence: None,
        }
    }
}

impl State {
    pub fn reserved_mib(&self) -> u32 {
        let committed = self
            .current
            .iter()
            .chain(&self.retired)
            .fold(0u32, |sum, g| sum.saturating_add(g.reserved_mib()));
        let pending = self.pending.as_ref().map_or(0, |p| {
            if p.request.action == Action::Delete {
                0
            } else {
                p.artifact.reserved_mib()
            }
        });
        committed.saturating_add(pending.max(self.forwarding_reserved_mib))
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.forwarding.is_some()
    }

    pub fn has_artifacts(&self) -> bool {
        self.current.is_some() || !self.retired.is_empty() || self.busy()
    }

    /// Check replay before the precondition. A completed restore never rewinds again.
    pub fn check(&self, request: &Request) -> Result<bool, String> {
        request.validate()?;
        if let Some(receipt) = &self.last_result
            && receipt.request.operation_id == request.operation_id
        {
            return if receipt.request == *request {
                Ok(true)
            } else {
                Err("conflict: backup operation ID reused with different parameters".into())
            };
        }
        if let Some(pending) = &self.pending {
            return if pending.request == *request {
                Ok(false)
            } else {
                Err(
                    "conflict: backup operation unfinished; inspect and retry its exact request"
                        .into(),
                )
            };
        }
        if request.expected_revision != self.revision {
            return Err("precondition: stale backup revision".into());
        }
        Ok(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub vm: crate::model::Vm,
    pub enabled: bool,
    pub supported: bool,
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBody {
    pub generation: String,
}

/// Transport-independent mapping, also used when forwarding agent errors.
pub fn error_status(message: &str) -> u16 {
    if message.starts_with("precondition:") {
        412
    } else if message.starts_with("missing precondition:") {
        428
    } else if message.starts_with("not found:") {
        404
    } else if message.starts_with("conflict:") {
        409
    } else if message.starts_with("backup unsupported:")
        || message.starts_with("invalid ")
        || message == "restore requires a generation"
        || message == "generation is only valid for restore"
        || message == "backup admission disabled"
    {
        400
    } else {
        500
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_restore_and_stale_replays_never_repeat_a_write() {
        let mut state = State::default();
        let request = Request {
            operation_id: token(),
            expected_revision: state.revision.clone(),
            action: Action::Restore,
            generation: Some(token()),
        };
        state.revision = token();
        state.last_result = Some(Receipt {
            request: request.clone(),
            completed_at: 1,
            error: None,
        });
        assert_eq!(state.check(&request), Ok(true));
        let mut changed = request.clone();
        changed.generation = Some(token());
        assert!(state.check(&changed).unwrap_err().starts_with("conflict:"));
        state.last_result = None;
        assert!(
            state
                .check(&request)
                .unwrap_err()
                .starts_with("precondition:")
        );
    }
}
