// Copyright 2025 ZettaScale Technology
//
// Topic name qualification and expansion for ROS 2 compatibility

/// Errors that can occur during topic name qualification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicNameError {
    /// Topic name is empty
    Empty,
    /// Topic name ends with a forward slash
    EndsWithSlash,
    /// Topic name contains invalid characters
    InvalidCharacters(String),
    /// Namespace is invalid
    InvalidNamespace(String),
    /// Node name is invalid
    InvalidNodeName(String),
}

impl std::fmt::Display for TopicNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "Topic name is empty"),
            Self::EndsWithSlash => write!(f, "Topic name ends with forward slash"),
            Self::InvalidCharacters(s) => {
                write!(f, "Topic name contains invalid characters: {}", s)
            }
            Self::InvalidNamespace(s) => write!(f, "Invalid namespace: {}", s),
            Self::InvalidNodeName(s) => write!(f, "Invalid node name: {}", s),
        }
    }
}

impl std::error::Error for TopicNameError {}

/// Validate that a topic name component (between slashes) is valid
/// Components must start with a letter or underscore, followed by alphanumeric or underscores
fn is_valid_topic_component(component: &str) -> bool {
    if component.is_empty() {
        return false;
    }
    let bytes = component.as_bytes();
    if !bytes[0].is_ascii_alphabetic() && bytes[0] != b'_' {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Validate a namespace string
/// Namespaces can be empty, "/", or a series of valid components separated by "/"
fn validate_namespace(namespace: &str) -> Result<(), TopicNameError> {
    if namespace.is_empty() || namespace == "/" {
        return Ok(());
    }

    if namespace.ends_with('/') {
        return Err(TopicNameError::InvalidNamespace(
            "namespace cannot end with '/'".to_string(),
        ));
    }

    for part in namespace.split('/') {
        if part.is_empty() {
            continue; // Leading slash creates empty first component
        }
        if !is_valid_topic_component(part) {
            return Err(TopicNameError::InvalidNamespace(format!(
                "invalid component '{}'",
                part
            )));
        }
    }
    Ok(())
}

/// Validate a node name
fn validate_node_name(node_name: &str) -> Result<(), TopicNameError> {
    if node_name.is_empty() {
        return Err(TopicNameError::InvalidNodeName(
            "node name is empty".to_string(),
        ));
    }
    if !is_valid_topic_component(node_name) {
        return Err(TopicNameError::InvalidNodeName(format!(
            "invalid node name '{}'",
            node_name
        )));
    }
    Ok(())
}

/// Qualify a topic name according to ROS 2 naming rules
///
/// This function takes a topic name and qualifies it based on the node's namespace and name.
///
/// Rules:
/// - Absolute topics (starting with '/') are returned as-is (with trailing slash removed if present)
/// - Private topics (starting with '~') are expanded to /<namespace>/<node_name>/<topic>
/// - Relative topics are expanded to /<namespace>/<topic>
/// - Empty namespace is treated as "/"
///
/// # Arguments
/// * `topic` - The input topic name (can be absolute, relative, or private)
/// * `namespace` - The node's namespace (can be "" or "/")
/// * `node_name` - The node's name
///
/// # Returns
/// * `Ok(String)` - The fully qualified topic name
/// * `Err(TopicNameError)` - If validation fails
///
/// # Examples
/// ```
/// use hiroz::topic_name::qualify_topic_name;
///
/// // Absolute topic
/// assert_eq!(qualify_topic_name("/chatter", "/ns", "node").unwrap(), "/chatter");
///
/// // Relative topic in root namespace
/// assert_eq!(qualify_topic_name("chatter", "/", "node").unwrap(), "/chatter");
///
/// // Relative topic in named namespace
/// assert_eq!(qualify_topic_name("chatter", "/ns", "node").unwrap(), "/ns/chatter");
///
/// // Private topic
/// assert_eq!(qualify_topic_name("~my_topic", "/ns", "node").unwrap(), "/ns/node/my_topic");
/// ```
pub fn qualify_topic_name(
    topic: &str,
    namespace: &str,
    node_name: &str,
) -> Result<String, TopicNameError> {
    // Validate inputs
    if topic.is_empty() {
        return Err(TopicNameError::Empty);
    }

    validate_namespace(namespace)?;
    validate_node_name(node_name)?;

    // Normalize namespace: ensure it starts with "/" if not empty
    let namespace = if namespace.is_empty() {
        "".to_string()
    } else if namespace.starts_with('/') {
        namespace.to_string()
    } else {
        format!("/{}", namespace)
    };

    // Handle different topic name types
    let qualified = if topic.starts_with('/') {
        // Absolute topic - use as-is, but remove trailing slash if present
        let topic = topic.strip_suffix('/').unwrap_or(topic);
        if topic.is_empty() || topic == "/" {
            return Err(TopicNameError::InvalidCharacters(
                "topic cannot be just '/'".to_string(),
            ));
        }
        topic.to_string()
    } else if topic.starts_with('~') {
        // Private topic - expand with namespace and node name
        let topic_suffix = topic.strip_prefix('~').unwrap();
        let topic_suffix = topic_suffix.strip_prefix('/').unwrap_or(topic_suffix);

        // Validate the topic suffix
        if !topic_suffix.is_empty() {
            for part in topic_suffix.split('/') {
                if !part.is_empty() && !is_valid_topic_component(part) {
                    return Err(TopicNameError::InvalidCharacters(format!(
                        "invalid component '{}' in private topic",
                        part
                    )));
                }
            }
        }

        if namespace.is_empty() || namespace == "/" {
            if topic_suffix.is_empty() {
                format!("/{}", node_name)
            } else {
                format!("/{}/{}", node_name, topic_suffix)
            }
        } else if topic_suffix.is_empty() {
            format!("{}/{}", namespace, node_name)
        } else {
            format!("{}/{}/{}", namespace, node_name, topic_suffix)
        }
    } else {
        // Relative topic - expand with namespace only
        let topic = topic.strip_suffix('/').unwrap_or(topic);

        // Validate topic components
        for part in topic.split('/') {
            if !part.is_empty() && !is_valid_topic_component(part) {
                return Err(TopicNameError::InvalidCharacters(format!(
                    "invalid component '{}'",
                    part
                )));
            }
        }

        if namespace.is_empty() || namespace == "/" {
            format!("/{}", topic)
        } else {
            format!("{}/{}", namespace, topic)
        }
    };

    Ok(qualified)
}

/// Qualify a service name according to ROS 2 naming rules
///
/// Service names follow the same rules as topic names
pub fn qualify_service_name(
    service: &str,
    namespace: &str,
    node_name: &str,
) -> Result<String, TopicNameError> {
    qualify_topic_name(service, namespace, node_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_absolute_topics() {
        assert_eq!(
            qualify_topic_name("/chatter", "/", "node").unwrap(),
            "/chatter"
        );
        assert_eq!(
            qualify_topic_name("/chatter", "/ns", "node").unwrap(),
            "/chatter"
        );
        assert_eq!(
            qualify_topic_name("/foo/bar", "/ns", "node").unwrap(),
            "/foo/bar"
        );
    }

    #[test]
    fn test_absolute_topics_trailing_slash() {
        assert_eq!(
            qualify_topic_name("/chatter/", "/ns", "node").unwrap(),
            "/chatter"
        );
    }

    #[test]
    fn test_relative_topics_root_namespace() {
        assert_eq!(
            qualify_topic_name("chatter", "/", "node").unwrap(),
            "/chatter"
        );
        assert_eq!(
            qualify_topic_name("chatter", "", "node").unwrap(),
            "/chatter"
        );
    }

    #[test]
    fn test_relative_topics_named_namespace() {
        assert_eq!(
            qualify_topic_name("chatter", "/ns", "node").unwrap(),
            "/ns/chatter"
        );
        assert_eq!(
            qualify_topic_name("foo/bar", "/ns", "node").unwrap(),
            "/ns/foo/bar"
        );
        assert_eq!(
            qualify_topic_name("chatter", "/my/nested/ns", "node").unwrap(),
            "/my/nested/ns/chatter"
        );
    }

    #[test]
    fn test_private_topics() {
        assert_eq!(
            qualify_topic_name("~my_topic", "/", "node").unwrap(),
            "/node/my_topic"
        );
        assert_eq!(
            qualify_topic_name("~my_topic", "/ns", "node").unwrap(),
            "/ns/node/my_topic"
        );
        assert_eq!(qualify_topic_name("~", "/ns", "node").unwrap(), "/ns/node");
        assert_eq!(
            qualify_topic_name("~/my_topic", "/ns", "node").unwrap(),
            "/ns/node/my_topic"
        );
    }

    #[test]
    fn test_private_topics_nested() {
        assert_eq!(
            qualify_topic_name("~foo/bar", "/ns", "node").unwrap(),
            "/ns/node/foo/bar"
        );
    }

    #[test]
    fn test_empty_topic() {
        assert!(matches!(
            qualify_topic_name("", "/", "node"),
            Err(TopicNameError::Empty)
        ));
    }

    #[test]
    fn test_invalid_namespace() {
        assert!(matches!(
            qualify_topic_name("chatter", "/ns/", "node"),
            Err(TopicNameError::InvalidNamespace(_))
        ));
    }

    #[test]
    fn test_invalid_node_name() {
        assert!(matches!(
            qualify_topic_name("chatter", "/ns", ""),
            Err(TopicNameError::InvalidNodeName(_))
        ));
        assert!(matches!(
            qualify_topic_name("chatter", "/ns", "123node"),
            Err(TopicNameError::InvalidNodeName(_))
        ));
    }

    #[test]
    fn test_valid_topic_components() {
        assert!(is_valid_topic_component("foo"));
        assert!(is_valid_topic_component("_foo"));
        assert!(is_valid_topic_component("foo123"));
        assert!(is_valid_topic_component("foo_bar"));
        assert!(is_valid_topic_component("FooBar"));

        assert!(!is_valid_topic_component(""));
        assert!(!is_valid_topic_component("123"));
        assert!(!is_valid_topic_component("foo-bar"));
        assert!(!is_valid_topic_component("foo bar"));
    }

    #[test]
    fn test_service_names() {
        assert_eq!(
            qualify_service_name("/add_two_ints", "/", "node").unwrap(),
            "/add_two_ints"
        );
        assert_eq!(
            qualify_service_name("add_two_ints", "/ns", "node").unwrap(),
            "/ns/add_two_ints"
        );
        assert_eq!(
            qualify_service_name("~my_service", "/ns", "node").unwrap(),
            "/ns/node/my_service"
        );
    }
}
