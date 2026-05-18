#[cfg(test)]
mod tests {
    use hiroz::action::{GoalEvent, GoalStatus, transition_goal_state};
    #[test]
    fn test_valid_transitions() {
        // From ACCEPTED
        assert_eq!(
            transition_goal_state(GoalStatus::Accepted, GoalEvent::Execute),
            GoalStatus::Executing
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Accepted, GoalEvent::CancelGoal),
            GoalStatus::Canceling
        );

        // From EXECUTING
        assert_eq!(
            transition_goal_state(GoalStatus::Executing, GoalEvent::CancelGoal),
            GoalStatus::Canceling
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Executing, GoalEvent::Succeed),
            GoalStatus::Succeeded
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Executing, GoalEvent::Abort),
            GoalStatus::Aborted
        );

        // From CANCELING
        assert_eq!(
            transition_goal_state(GoalStatus::Canceling, GoalEvent::Canceled),
            GoalStatus::Canceled
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Canceling, GoalEvent::Abort),
            GoalStatus::Aborted
        );
    }

    #[test]
    fn test_invalid_transitions() {
        // Invalid from ACCEPTED
        assert_eq!(
            transition_goal_state(GoalStatus::Accepted, GoalEvent::Succeed),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Accepted, GoalEvent::Abort),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Accepted, GoalEvent::Canceled),
            GoalStatus::Unknown
        );

        // Invalid from EXECUTING
        assert_eq!(
            transition_goal_state(GoalStatus::Executing, GoalEvent::Execute),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Executing, GoalEvent::Canceled),
            GoalStatus::Unknown
        );

        // Invalid from CANCELING
        assert_eq!(
            transition_goal_state(GoalStatus::Canceling, GoalEvent::Execute),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Canceling, GoalEvent::CancelGoal),
            GoalStatus::Unknown
        );

        // Invalid from SUCCEEDED
        assert_eq!(
            transition_goal_state(GoalStatus::Succeeded, GoalEvent::Execute),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Succeeded, GoalEvent::CancelGoal),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Succeeded, GoalEvent::Succeed),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Succeeded, GoalEvent::Abort),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Succeeded, GoalEvent::Canceled),
            GoalStatus::Unknown
        );

        // Invalid from ABORTED
        assert_eq!(
            transition_goal_state(GoalStatus::Aborted, GoalEvent::Execute),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Aborted, GoalEvent::CancelGoal),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Aborted, GoalEvent::Succeed),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Aborted, GoalEvent::Abort),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Aborted, GoalEvent::Canceled),
            GoalStatus::Unknown
        );

        // Invalid from CANCELED
        assert_eq!(
            transition_goal_state(GoalStatus::Canceled, GoalEvent::Execute),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Canceled, GoalEvent::CancelGoal),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Canceled, GoalEvent::Succeed),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Canceled, GoalEvent::Abort),
            GoalStatus::Unknown
        );
        assert_eq!(
            transition_goal_state(GoalStatus::Canceled, GoalEvent::Canceled),
            GoalStatus::Unknown
        );
    }
}
