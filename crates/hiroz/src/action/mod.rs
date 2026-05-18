use crate::msg::ZMessage;
use hiroz_cdr::{CdrBuffer, CdrDeserialize, CdrReader, CdrSerialize, CdrSerializedSize, CdrWriter};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

pub mod client;
pub mod driver;
pub mod macros;
pub mod messages;
pub mod server;
pub mod state;

// Re-export type-state markers for documentation and advanced usage
pub use server::{Accepted, Executing, Requested};

/// Type alias for the client-side goal handle.
///
/// Use this when you need to name the client `GoalHandle` type alongside
/// server types (e.g. in a node that is both an action client and server)
/// to avoid the name collision with [`server::GoalHandle`].
///
/// # Example
///
/// ```no_run
/// use hiroz::action::ClientGoalHandle;
/// ```
pub type ClientGoalHandle<A, S = client::goal_state::Active> = client::GoalHandle<A, S>;

/// Core trait for ROS 2 actions
pub trait ZAction: Send + Sync + 'static {
    type Goal: ZMessage + Clone + Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de>;
    type Result: ZMessage
        + Clone
        + Send
        + Sync
        + serde::Serialize
        + for<'de> serde::Deserialize<'de>;
    type Feedback: ZMessage + Clone + serde::Serialize + for<'de> serde::Deserialize<'de>;

    fn name() -> &'static str;

    /// Returns type info for the SendGoal service.
    /// Default implementation returns zero hash for hiroz to hiroz communication only.
    /// Override this to provide proper type hashes for ROS 2 interop.
    fn send_goal_type_info() -> crate::entity::TypeInfo {
        crate::entity::TypeInfo::new(
            &format!("{}/_action/SendGoal", Self::name()),
            crate::entity::TypeHash::zero(),
        )
    }

    /// Returns type info for the GetResult service.
    /// Default implementation returns zero hash for hiroz to hiroz communication only.
    /// Override this to provide proper type hashes for ROS 2 interop.
    fn get_result_type_info() -> crate::entity::TypeInfo {
        crate::entity::TypeInfo::new(
            &format!("{}/_action/GetResult", Self::name()),
            crate::entity::TypeHash::zero(),
        )
    }

    /// Returns type info for the CancelGoal service.
    /// Default implementation returns zero hash for hiroz to hiroz communication only.
    /// Override this to provide proper type hashes for ROS 2 interop.
    fn cancel_goal_type_info() -> crate::entity::TypeInfo {
        crate::entity::TypeInfo::new(
            "action_msgs/srv/CancelGoal",
            crate::entity::TypeHash::zero(),
        )
    }

    /// Returns type info for the Feedback topic.
    /// Default implementation returns zero hash for hiroz to hiroz communication only.
    /// Override this to provide proper type hashes for ROS 2 interop.
    fn feedback_type_info() -> crate::entity::TypeInfo {
        crate::entity::TypeInfo::new(
            &format!("{}/_FeedbackMessage", Self::name()),
            crate::entity::TypeHash::zero(),
        )
    }

    /// Returns type info for the Status topic.
    /// Default implementation returns zero hash for hiroz to hiroz communication only.
    /// Override this to provide proper type hashes for ROS 2 interop.
    fn status_type_info() -> crate::entity::TypeInfo {
        crate::entity::TypeInfo::new(
            "action_msgs/msg/GoalStatusArray",
            crate::entity::TypeHash::zero(),
        )
    }
}

/// Unique identifier for action goals.
///
/// A `GoalId` is a UUID that uniquely identifies an action goal.
/// It is generated when a goal is sent and used to track the goal's
/// lifecycle, feedback, and results.
///
/// # Examples
///
/// ```
/// # use hiroz::action::GoalId;
/// let goal_id = GoalId::new();
/// assert!(goal_id.is_valid());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GoalId([u8; 16]);

impl GoalId {
    /// Creates a new random GoalId.
    ///
    /// Generates a UUID v4 and uses it as the goal identifier.
    ///
    /// # Returns
    ///
    /// A new `GoalId` with a randomly generated UUID.
    pub fn new() -> Self {
        // Generate UUID v4
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..]);
        Self(uuid)
    }

    /// Creates a GoalId from raw bytes.
    ///
    /// # Arguments
    ///
    /// * `bytes` - A 16-byte array representing a UUID.
    ///
    /// # Returns
    ///
    /// A `GoalId` with the specified bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Checks if this GoalId is valid (not all zeros).
    ///
    /// # Returns
    ///
    /// `true` if the GoalId contains at least one non-zero byte, `false` otherwise.
    pub fn is_valid(&self) -> bool {
        self.0.iter().any(|&x| x != 0)
    }

    /// Returns the raw bytes of this GoalId.
    ///
    /// # Returns
    ///
    /// A reference to the 16-byte array containing the UUID.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl Default for GoalId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for GoalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let uuid = uuid::Uuid::from_bytes(self.0);
        write!(f, "{}", uuid.hyphenated())
    }
}

/// Status of an action goal.
///
/// The `GoalStatus` enum represents the current state of an action goal
/// in its lifecycle, from acceptance to completion or cancellation.
///
/// # Examples
///
/// ```
/// # use hiroz::action::GoalStatus;
/// let status = GoalStatus::Executing;
/// assert!(status.is_active());
/// assert!(!status.is_terminal());
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i8)]
#[serde(try_from = "i8", into = "i8")]
pub enum GoalStatus {
    /// Unknown status (initial state).
    Unknown = 0,
    /// Goal has been accepted by the server.
    Accepted = 1,
    /// Goal is currently being executed.
    Executing = 2,
    /// Goal is being canceled.
    Canceling = 3,
    /// Goal completed successfully.
    Succeeded = 4,
    /// Goal was canceled.
    Canceled = 5,
    /// Goal failed/aborted.
    Aborted = 6,
}

impl GoalStatus {
    /// Checks if the goal is in an active state.
    ///
    /// Active states are `Accepted`, `Executing`, and `Canceling`.
    ///
    /// # Returns
    ///
    /// `true` if the goal is active, `false` otherwise.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Accepted | Self::Executing | Self::Canceling)
    }

    /// Checks if the goal is in a terminal state.
    ///
    /// Terminal states are `Succeeded`, `Canceled`, and `Aborted`.
    ///
    /// # Returns
    ///
    /// `true` if the goal is terminal, `false` otherwise.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Canceled | Self::Aborted)
    }
}

// Conversion from i8 for serde deserialization (ROS2 uses i8 for status)
impl TryFrom<i8> for GoalStatus {
    type Error = String;

    fn try_from(value: i8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(GoalStatus::Unknown),
            1 => Ok(GoalStatus::Accepted),
            2 => Ok(GoalStatus::Executing),
            3 => Ok(GoalStatus::Canceling),
            4 => Ok(GoalStatus::Succeeded),
            5 => Ok(GoalStatus::Canceled),
            6 => Ok(GoalStatus::Aborted),
            _ => Err(format!("Invalid GoalStatus value: {}", value)),
        }
    }
}

// Conversion to i8 for serde serialization (ROS2 uses i8 for status)
impl From<GoalStatus> for i8 {
    fn from(status: GoalStatus) -> i8 {
        status as i8
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Information about an action goal including its ID and timestamp.
///
/// `GoalInfo` combines a `GoalId` with a timestamp to provide complete
/// information about when a goal was created or last updated.
///
/// This matches the ROS2 `action_msgs/msg/GoalInfo` structure which uses
/// `builtin_interfaces/Time` for the stamp field.
///
/// # Examples
///
/// ```
/// # use hiroz::action::{GoalId, GoalInfo};
/// let goal_id = GoalId::new();
/// let goal_info = GoalInfo::new(goal_id);
/// ```
pub struct GoalInfo {
    /// The unique identifier of the goal.
    pub goal_id: GoalId,
    /// Timestamp using ROS2 Time structure (sec: i32, nanosec: u32)
    pub stamp: Time,
}

/// ROS2 Time structure from builtin_interfaces/msg/Time
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Time {
    /// Seconds component of the timestamp
    pub sec: i32,
    /// Nanoseconds component of the timestamp
    pub nanosec: u32,
}

impl Time {
    /// Creates a Time from the current system time
    pub fn now() -> Self {
        let duration = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        Self {
            sec: duration.as_secs() as i32,
            nanosec: duration.subsec_nanos(),
        }
    }

    /// Creates a zero timestamp
    pub fn zero() -> Self {
        Self { sec: 0, nanosec: 0 }
    }
}

impl GoalInfo {
    /// Creates a new GoalInfo with the current timestamp.
    ///
    /// # Arguments
    ///
    /// * `goal_id` - The ID of the goal.
    ///
    /// # Returns
    ///
    /// A `GoalInfo` with the specified goal ID and current timestamp.
    pub fn new(goal_id: GoalId) -> Self {
        Self {
            goal_id,
            stamp: Time::now(),
        }
    }
}

/// Events that can trigger goal state transitions.
///
/// `GoalEvent` represents the different events that can cause an action goal
/// to transition from one state to another in the ROS 2 action state machine.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalEvent {
    /// Start executing an accepted goal.
    Execute,
    /// Request to cancel the goal.
    CancelGoal,
    /// Goal execution completed successfully.
    Succeed,
    /// Goal execution failed.
    Abort,
    /// Goal was successfully canceled.
    Canceled,
}

/// Transitions a goal status based on an event.
///
/// This function implements the ROS 2 action state machine transitions.
/// It takes the current goal status and an event, and returns the new status
/// according to the ROS 2 specification.
///
/// # Arguments
///
/// * `current` - The current status of the goal.
/// * `event` - The event that triggered the transition.
///
/// # Returns
///
/// The new goal status after applying the transition. Returns `GoalStatus::Unknown`
/// for invalid transitions.
///
/// # Examples
///
/// ```
/// # use hiroz::action::{GoalStatus, GoalEvent, transition_goal_state};
/// let new_status = transition_goal_state(GoalStatus::Accepted, GoalEvent::Execute);
/// assert_eq!(new_status, GoalStatus::Executing);
/// ```
pub fn transition_goal_state(current: GoalStatus, event: GoalEvent) -> GoalStatus {
    match (current, event) {
        // From ACCEPTED
        (GoalStatus::Accepted, GoalEvent::Execute) => GoalStatus::Executing,
        (GoalStatus::Accepted, GoalEvent::CancelGoal) => GoalStatus::Canceling,

        // From EXECUTING
        (GoalStatus::Executing, GoalEvent::CancelGoal) => GoalStatus::Canceling,
        (GoalStatus::Executing, GoalEvent::Succeed) => GoalStatus::Succeeded,
        (GoalStatus::Executing, GoalEvent::Abort) => GoalStatus::Aborted,

        // From CANCELING
        (GoalStatus::Canceling, GoalEvent::Canceled) => GoalStatus::Canceled,
        (GoalStatus::Canceling, GoalEvent::Succeed) => GoalStatus::Succeeded,
        (GoalStatus::Canceling, GoalEvent::Abort) => GoalStatus::Aborted,

        // Invalid transitions
        _ => GoalStatus::Unknown,
    }
}

// ── CDR serialization impls ───────────────────────────────────────────────────
// These allow GoalId, GoalStatus, Time, and GoalInfo to satisfy the
// CdrSerialize + CdrDeserialize + CdrSerializedSize bounds, which in turn
// lets the action message types use the NativeCdrSerdes blanket ZMessage impl.

impl CdrSerialize for GoalId {
    #[inline]
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.0.cdr_serialize(w);
    }
}

impl CdrDeserialize for GoalId {
    #[inline]
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(GoalId(<[u8; 16]>::cdr_deserialize(r)?))
    }
}

impl CdrSerializedSize for GoalId {
    #[inline]
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        self.0.cdr_serialized_size(pos)
    }
}

impl CdrSerialize for GoalStatus {
    #[inline]
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        (*self as i8).cdr_serialize(w);
    }
}

impl CdrDeserialize for GoalStatus {
    #[inline]
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        let v = i8::cdr_deserialize(r)?;
        GoalStatus::try_from(v).map_err(hiroz_cdr::error::Error::Custom)
    }
}

impl CdrSerializedSize for GoalStatus {
    #[inline]
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        pos + 1
    }
}

impl CdrSerialize for Time {
    #[inline]
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.sec.cdr_serialize(w);
        self.nanosec.cdr_serialize(w);
    }
}

impl CdrDeserialize for Time {
    #[inline]
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(Time {
            sec: i32::cdr_deserialize(r)?,
            nanosec: u32::cdr_deserialize(r)?,
        })
    }
}

impl CdrSerializedSize for Time {
    #[inline]
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.sec.cdr_serialized_size(pos);
        self.nanosec.cdr_serialized_size(p)
    }
}

impl CdrSerialize for GoalInfo {
    #[inline]
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.goal_id.cdr_serialize(w);
        self.stamp.cdr_serialize(w);
    }
}

impl CdrDeserialize for GoalInfo {
    #[inline]
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(GoalInfo {
            goal_id: GoalId::cdr_deserialize(r)?,
            stamp: Time::cdr_deserialize(r)?,
        })
    }
}

impl CdrSerializedSize for GoalInfo {
    #[inline]
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.goal_id.cdr_serialized_size(pos);
        self.stamp.cdr_serialized_size(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goal_id_display_is_hyphenated_uuid() {
        let bytes = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let id = GoalId(bytes);
        let s = format!("{}", id);
        assert_eq!(s, "00010203-0405-0607-0809-0a0b0c0d0e0f");
    }

    #[test]
    fn test_goal_status_variants_are_distinct() {
        assert_ne!(GoalStatus::Unknown, GoalStatus::Accepted);
        assert_ne!(GoalStatus::Executing, GoalStatus::Succeeded);
        assert_ne!(GoalStatus::Canceled, GoalStatus::Aborted);
    }
}
