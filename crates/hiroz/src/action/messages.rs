use std::marker::PhantomData;

use hiroz_cdr::{CdrBuffer, CdrDeserialize, CdrReader, CdrSerialize, CdrSerializedSize, CdrWriter};
use serde::{Deserialize, Serialize};

use super::{GoalId, GoalInfo, GoalStatus, ZAction};

// Standard ROS 2 cancel messages

/// Request to cancel one or more goals.
///
/// Used to request cancellation of specific goals or all goals.
/// A zero UUID in `goal_info.goal_id` cancels all goals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelGoalRequest {
    /// Information about the goal(s) to cancel.
    pub goal_info: GoalInfo,
}

/// Response to a cancel goal request.
///
/// Contains the result code and list of goals that are being canceled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelGoalResponse {
    /// Return code indicating success or failure.
    pub return_code: i8,
    /// List of goals that are being canceled.
    pub goals_canceling: Vec<GoalInfo>,
}

// Internal service/topic message types

/// Request to send a goal to an action server.
///
/// Contains the goal ID and the actual goal data.
#[derive(Debug, Clone)]
pub struct GoalRequest<A: ZAction> {
    /// Unique identifier for this goal.
    pub goal_id: GoalId,
    /// The goal data to be executed.
    pub goal: A::Goal,
}

impl<A: ZAction> serde::Serialize for GoalRequest<A>
where
    A: 'static,
    A::Goal: serde::Serialize + 'static,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("GoalRequest", 2)?;
        state.serialize_field("goal_id", &self.goal_id)?;
        state.serialize_field("goal", &self.goal)?;
        state.end()
    }
}

impl<'de, A: ZAction> serde::Deserialize<'de> for GoalRequest<A>
where
    A: 'static,
    A::Goal: serde::Deserialize<'de> + 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct GoalRequestHelper<B> {
            goal_id: GoalId,
            goal: B,
        }
        let helper = GoalRequestHelper::<A::Goal>::deserialize(deserializer)?;
        Ok(GoalRequest {
            goal_id: helper.goal_id,
            goal: helper.goal,
        })
    }
}

/// Response to a goal request.
///
/// Indicates whether the goal was accepted and includes a timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalResponse {
    /// Whether the goal was accepted by the server.
    pub accepted: bool,
    /// Timestamp seconds (corresponds to builtin_interfaces/Time.sec)
    pub stamp_sec: i32,
    /// Timestamp nanoseconds (corresponds to builtin_interfaces/Time.nanosec)
    pub stamp_nanosec: u32,
}

/// Request to get the result of a completed goal.
///
/// Contains the goal ID for which to retrieve the result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRequest {
    /// The ID of the goal whose result is requested.
    pub goal_id: GoalId,
}

/// Response containing the result of a completed goal.
///
/// Includes the final status and the result data.
#[derive(Debug, Clone)]
pub struct ResultResponse<A: ZAction> {
    /// The final status of the goal.
    pub status: GoalStatus,
    /// The result data returned by the action server.
    pub result: A::Result,
}

impl<A: ZAction> serde::Serialize for ResultResponse<A>
where
    A: 'static,
    A::Result: serde::Serialize + 'static,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ResultResponse", 2)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("result", &self.result)?;
        state.end()
    }
}

impl<'de, A: ZAction> serde::Deserialize<'de> for ResultResponse<A>
where
    A: 'static,
    A::Result: serde::Deserialize<'de> + 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct ResultResponseHelper<B> {
            status: GoalStatus,
            result: B,
        }
        let helper = ResultResponseHelper::<A::Result>::deserialize(deserializer)?;
        Ok(ResultResponse {
            status: helper.status,
            result: helper.result,
        })
    }
}

/// Message containing feedback from an executing goal.
///
/// Feedback messages are published periodically during goal execution
/// to provide progress updates to clients.
///
/// Note: This type does NOT implement WithTypeInfo because the type hash
/// is action-specific and must be provided via A::feedback_type_info()
#[derive(Debug, Clone)]
pub struct FeedbackMessage<A: ZAction> {
    /// The ID of the goal providing feedback.
    pub goal_id: GoalId,
    /// The feedback data from the executing goal.
    pub feedback: A::Feedback,
}

impl<A: ZAction> serde::Serialize for FeedbackMessage<A>
where
    A: 'static,
    A::Feedback: serde::Serialize + 'static,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("FeedbackMessage", 2)?;
        state.serialize_field("goal_id", &self.goal_id)?;
        state.serialize_field("feedback", &self.feedback)?;
        state.end()
    }
}

impl<'de, A: ZAction> serde::Deserialize<'de> for FeedbackMessage<A>
where
    A: 'static,
    A::Feedback: serde::Deserialize<'de> + 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct FeedbackMessageHelper<B> {
            goal_id: GoalId,
            feedback: B,
        }
        let helper = FeedbackMessageHelper::<A::Feedback>::deserialize(deserializer)?;
        Ok(FeedbackMessage {
            goal_id: helper.goal_id,
            feedback: helper.feedback,
        })
    }
}

/// Message containing status updates for multiple goals.
///
/// Published periodically to inform clients about the current status
/// of all goals known to the action server.
///
/// Note: This type does NOT implement WithTypeInfo because the type hash
/// is action-specific and must be provided via A::status_type_info()
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusMessage {
    /// List of status information for all goals.
    pub status_list: Vec<GoalStatusInfo>,
}

/// Status information for a single goal.
///
/// Contains the goal info and current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalStatusInfo {
    /// Information about the goal (ID and timestamp).
    pub goal_info: GoalInfo,
    /// Current status of the goal.
    pub status: GoalStatus,
}

// Internal service type wrappers
/// SendGoal service request message matching ROS2 IDL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendGoalRequest<A: ZAction> {
    pub goal_id: GoalId,
    pub goal: A::Goal,
}

/// SendGoal service response message matching ROS2 IDL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendGoalResponse {
    pub accepted: bool,
    pub stamp_sec: i32,
    pub stamp_nanosec: u32,
}

pub struct GoalService<A: ZAction>(PhantomData<A>);
impl<A: ZAction> crate::msg::ZService for GoalService<A> {
    type Request = SendGoalRequest<A>;
    type Response = SendGoalResponse;
}

impl<A: ZAction> crate::ServiceTypeInfo for GoalService<A> {
    fn service_type_info() -> crate::entity::TypeInfo {
        // Delegate to the action's send_goal_type_info method for proper interop
        A::send_goal_type_info()
    }
}

/// GetResult service request message matching ROS2 IDL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetResultRequest {
    pub goal_id: GoalId,
}

/// GetResult service response message matching ROS2 IDL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetResultResponse<A: ZAction> {
    pub status: i8,
    pub result: A::Result,
}

pub struct ResultService<A: ZAction>(PhantomData<A>);
impl<A: ZAction> crate::msg::ZService for ResultService<A> {
    type Request = GetResultRequest;
    type Response = GetResultResponse<A>;
}

impl<A: ZAction> crate::ServiceTypeInfo for ResultService<A> {
    fn service_type_info() -> crate::entity::TypeInfo {
        // Delegate to the action's get_result_type_info method for proper interop
        A::get_result_type_info()
    }
}

/// CancelGoal service request message matching ROS2 IDL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelGoalServiceRequest {
    pub goal_info: GoalInfo,
}

/// CancelGoal service response message matching ROS2 IDL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelGoalServiceResponse {
    pub return_code: i8,
    pub goals_canceling: Vec<GoalInfo>,
}

pub struct CancelService<A: ZAction>(PhantomData<A>);
impl<A: ZAction> crate::msg::ZService for CancelService<A> {
    type Request = CancelGoalServiceRequest;
    type Response = CancelGoalServiceResponse;
}

impl<A: ZAction> crate::ServiceTypeInfo for CancelService<A> {
    fn service_type_info() -> crate::entity::TypeInfo {
        // Delegate to the action's cancel_goal_type_info method for proper interop
        A::cancel_goal_type_info()
    }
}

// ── CDR serialization impls ───────────────────────────────────────────────────
// Concrete (non-generic) action message types now implement CdrSerialize +
// CdrDeserialize + CdrSerializedSize directly, so the blanket
// `impl<T: CdrSerialize + ...> ZMessage for T` covers them automatically.
//
// Generic types (GoalRequest<A>, etc.) still use the serde path via explicit
// ZMessage impls below until ZAction's associated type bounds are updated.

impl CdrSerialize for GoalStatusInfo {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.goal_info.cdr_serialize(w);
        self.status.cdr_serialize(w);
    }
}
impl CdrDeserialize for GoalStatusInfo {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(GoalStatusInfo {
            goal_info: GoalInfo::cdr_deserialize(r)?,
            status: GoalStatus::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for GoalStatusInfo {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.goal_info.cdr_serialized_size(pos);
        self.status.cdr_serialized_size(p)
    }
}

impl CdrSerialize for CancelGoalRequest {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.goal_info.cdr_serialize(w);
    }
}
impl CdrDeserialize for CancelGoalRequest {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(CancelGoalRequest {
            goal_info: GoalInfo::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for CancelGoalRequest {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        self.goal_info.cdr_serialized_size(pos)
    }
}

impl CdrSerialize for CancelGoalResponse {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.return_code.cdr_serialize(w);
        self.goals_canceling.cdr_serialize(w);
    }
}
impl CdrDeserialize for CancelGoalResponse {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(CancelGoalResponse {
            return_code: i8::cdr_deserialize(r)?,
            goals_canceling: Vec::<GoalInfo>::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for CancelGoalResponse {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.return_code.cdr_serialized_size(pos);
        self.goals_canceling.cdr_serialized_size(p)
    }
}

impl CdrSerialize for GoalResponse {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.accepted.cdr_serialize(w);
        self.stamp_sec.cdr_serialize(w);
        self.stamp_nanosec.cdr_serialize(w);
    }
}
impl CdrDeserialize for GoalResponse {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(GoalResponse {
            accepted: bool::cdr_deserialize(r)?,
            stamp_sec: i32::cdr_deserialize(r)?,
            stamp_nanosec: u32::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for GoalResponse {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.accepted.cdr_serialized_size(pos);
        let p = self.stamp_sec.cdr_serialized_size(p);
        self.stamp_nanosec.cdr_serialized_size(p)
    }
}

impl CdrSerialize for ResultRequest {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.goal_id.cdr_serialize(w);
    }
}
impl CdrDeserialize for ResultRequest {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(ResultRequest {
            goal_id: GoalId::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for ResultRequest {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        self.goal_id.cdr_serialized_size(pos)
    }
}

impl CdrSerialize for StatusMessage {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.status_list.cdr_serialize(w);
    }
}
impl CdrDeserialize for StatusMessage {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(StatusMessage {
            status_list: Vec::<GoalStatusInfo>::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for StatusMessage {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        self.status_list.cdr_serialized_size(pos)
    }
}

impl CdrSerialize for SendGoalResponse {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.accepted.cdr_serialize(w);
        self.stamp_sec.cdr_serialize(w);
        self.stamp_nanosec.cdr_serialize(w);
    }
}
impl CdrDeserialize for SendGoalResponse {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(SendGoalResponse {
            accepted: bool::cdr_deserialize(r)?,
            stamp_sec: i32::cdr_deserialize(r)?,
            stamp_nanosec: u32::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for SendGoalResponse {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.accepted.cdr_serialized_size(pos);
        let p = self.stamp_sec.cdr_serialized_size(p);
        self.stamp_nanosec.cdr_serialized_size(p)
    }
}

impl CdrSerialize for GetResultRequest {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.goal_id.cdr_serialize(w);
    }
}
impl CdrDeserialize for GetResultRequest {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(GetResultRequest {
            goal_id: GoalId::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for GetResultRequest {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        self.goal_id.cdr_serialized_size(pos)
    }
}

impl CdrSerialize for CancelGoalServiceRequest {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.goal_info.cdr_serialize(w);
    }
}
impl CdrDeserialize for CancelGoalServiceRequest {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(CancelGoalServiceRequest {
            goal_info: GoalInfo::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for CancelGoalServiceRequest {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        self.goal_info.cdr_serialized_size(pos)
    }
}

impl CdrSerialize for CancelGoalServiceResponse {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.return_code.cdr_serialize(w);
        self.goals_canceling.cdr_serialize(w);
    }
}
impl CdrDeserialize for CancelGoalServiceResponse {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(CancelGoalServiceResponse {
            return_code: i8::cdr_deserialize(r)?,
            goals_canceling: Vec::<GoalInfo>::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for CancelGoalServiceResponse {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.return_code.cdr_serialized_size(pos);
        self.goals_canceling.cdr_serialized_size(p)
    }
}

// ── Generic types: still use serde path until ZAction gains CDR bounds ────────

impl<A: ZAction + 'static> crate::msg::ZMessage for GoalRequest<A>
where
    A::Goal: Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de> + 'static,
{
    type Serdes = crate::msg::SerdeCdrSerdes<GoalRequest<A>>;
}

impl<A: ZAction + 'static> crate::msg::ZMessage for ResultResponse<A>
where
    A::Result: Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de> + 'static,
{
    type Serdes = crate::msg::SerdeCdrSerdes<ResultResponse<A>>;
}

impl<A: ZAction + 'static> crate::msg::ZMessage for FeedbackMessage<A>
where
    A::Feedback: Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de> + 'static,
{
    type Serdes = crate::msg::SerdeCdrSerdes<FeedbackMessage<A>>;
}

impl<A: ZAction + 'static> crate::msg::ZMessage for SendGoalRequest<A>
where
    A::Goal: Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de> + 'static,
{
    type Serdes = crate::msg::SerdeCdrSerdes<SendGoalRequest<A>>;
}

impl<A: ZAction + 'static> crate::msg::ZMessage for GetResultResponse<A>
where
    A::Result: Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de> + 'static,
{
    type Serdes = crate::msg::SerdeCdrSerdes<GetResultResponse<A>>;
}
