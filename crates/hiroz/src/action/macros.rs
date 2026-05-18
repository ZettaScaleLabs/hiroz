/// Macro to define a simple action type without ROS2 interop.
///
/// This macro is a simplified version of `impl_action!` for custom actions that don't need
/// ROS2 type hashes and interop. It only implements the basic `ZAction` trait methods.
///
/// # Syntax
///
/// ```ignore
/// define_action! {
///     ActionStruct,
///     action_name: "action_name",
///     Goal: GoalType,
///     Result: ResultType,
///     Feedback: FeedbackType,
/// }
/// ```
///
/// # Example
///
/// ```ignore
/// use hiroz::define_action;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// pub struct NavigateToPoseGoal {
///     pub target_x: f64,
///     pub target_y: f64,
/// }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// pub struct NavigateToPoseResult {
///     pub success: bool,
/// }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// pub struct NavigateToPoseFeedback {
///     pub current_x: f64,
///     pub current_y: f64,
/// }
///
/// pub struct NavigateToPose;
///
/// define_action! {
///     NavigateToPose,
///     action_name: "navigate_to_pose",
///     Goal: NavigateToPoseGoal,
///     Result: NavigateToPoseResult,
///     Feedback: NavigateToPoseFeedback,
/// }
/// ```
#[macro_export]
macro_rules! define_action {
    (
        $action_struct:ident,
        action_name: $action_name:expr,
        Goal: $goal_type:ty,
        Result: $result_type:ty,
        Feedback: $feedback_type:ty $(,)?
    ) => {
        impl $crate::action::ZAction for $action_struct {
            type Goal = $goal_type;
            type Result = $result_type;
            type Feedback = $feedback_type;

            fn name() -> &'static str {
                $action_name
            }
        }

        // Provide ZMessage impls via the serde (SerdeCdrSerdes) path for types that
        // do not implement the CDR traits. Types that do implement CdrSerialize +
        // CdrDeserialize + CdrSerializedSize get ZMessage automatically from the
        // blanket impl in hiroz::msg and should NOT use define_action!.
        impl $crate::msg::ZMessage for $goal_type
        where
            $goal_type: Send + Sync + 'static,
        {
            type Serdes = $crate::msg::SerdeCdrSerdes<$goal_type>;
        }
        impl $crate::msg::ZMessage for $result_type
        where
            $result_type: Send + Sync + 'static,
        {
            type Serdes = $crate::msg::SerdeCdrSerdes<$result_type>;
        }
        impl $crate::msg::ZMessage for $feedback_type
        where
            $feedback_type: Send + Sync + 'static,
        {
            type Serdes = $crate::msg::SerdeCdrSerdes<$feedback_type>;
        }
    };
}
