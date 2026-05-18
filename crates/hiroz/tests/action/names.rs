// Test action name derivation functions
// In hiroz, these are computed inline, but we can test the logic

fn get_goal_service_name(action_name: &str) -> String {
    format!("{}/_action/send_goal", action_name)
}

fn get_cancel_service_name(action_name: &str) -> String {
    format!("{}/_action/cancel_goal", action_name)
}

fn get_result_service_name(action_name: &str) -> String {
    format!("{}/_action/get_result", action_name)
}

fn get_feedback_topic_name(action_name: &str) -> String {
    format!("{}/_action/feedback", action_name)
}

fn get_status_topic_name(action_name: &str) -> String {
    format!("{}/_action/status", action_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goal_service_name() {
        assert_eq!(
            get_goal_service_name("test_it"),
            "test_it/_action/send_goal"
        );
    }

    #[test]
    fn test_cancel_service_name() {
        assert_eq!(
            get_cancel_service_name("test_it"),
            "test_it/_action/cancel_goal"
        );
    }

    #[test]
    fn test_result_service_name() {
        assert_eq!(
            get_result_service_name("test_it"),
            "test_it/_action/get_result"
        );
    }

    #[test]
    fn test_feedback_topic_name() {
        assert_eq!(
            get_feedback_topic_name("test_it"),
            "test_it/_action/feedback"
        );
    }

    #[test]
    fn test_status_topic_name() {
        assert_eq!(get_status_topic_name("test_it"), "test_it/_action/status");
    }

    #[test]
    fn test_action_name_validation() {
        // Test with namespaced action
        assert_eq!(
            get_goal_service_name("/namespace/test_action"),
            "/namespace/test_action/_action/send_goal"
        );

        // Test with underscores
        assert_eq!(
            get_goal_service_name("test_action_with_underscores"),
            "test_action_with_underscores/_action/send_goal"
        );

        // Test with numbers (should work in hiroz)
        assert_eq!(
            get_goal_service_name("action123"),
            "action123/_action/send_goal"
        );
    }
}
