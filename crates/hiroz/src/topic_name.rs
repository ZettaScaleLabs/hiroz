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

/// Validate every `/`-separated component of `path`.
///
/// `path` must already have any leading `/` stripped, so that an empty
/// component always means a genuine `//` or a trailing `/` rather than the
/// leading separator of an absolute name.
///
/// Empty components are **rejected**, not skipped. Skipping them let `//a//b`
/// pass ROS validation and fail later inside zenoh's key-expression parser,
/// with an error that cites a dependency path and never names the offending
/// topic.
fn validate_topic_components(path: &str, context: &str) -> Result<(), TopicNameError> {
    for part in path.split('/') {
        if part.is_empty() {
            return Err(TopicNameError::InvalidCharacters(format!(
                "empty component in {context}: '//' and a trailing '/' are not valid ROS 2 names"
            )));
        }
        if !is_valid_topic_component(part) {
            return Err(TopicNameError::InvalidCharacters(format!(
                "invalid component '{part}' in {context}"
            )));
        }
    }
    Ok(())
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

    // Strip exactly one leading slash -- that one is the separator. Every
    // remaining component must be non-empty: skipping them (as this used to)
    // let `//ns` validate, and the namespace is concatenated verbatim into the
    // qualified name below, so `//ns` + `chatter` produced `//ns/chatter` --
    // the very `//` form topic validation rejects, reaching zenoh's
    // key-expression parser with the same opaque error this module exists to
    // prevent. Namespaces are user-supplied, so that path is reachable.
    let body = namespace.strip_prefix('/').unwrap_or(namespace);
    for part in body.split('/') {
        if part.is_empty() {
            return Err(TopicNameError::InvalidNamespace(
                "empty component: '//' is not a valid namespace".to_string(),
            ));
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
/// - Absolute topics (starting with '/') are validated and returned (with trailing slash removed if present)
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
        // Absolute topic - validated, with a trailing slash removed if present
        let topic = topic.strip_suffix('/').unwrap_or(topic);
        if topic.is_empty() || topic == "/" {
            return Err(TopicNameError::InvalidCharacters(
                "topic cannot be just '/'".to_string(),
            ));
        }
        // This branch used to return unchecked, so an absolute name reached
        // zenoh with no component validation at all -- `create_client("/bad
        // name", ..)` was accepted and failed later, or silently never matched.
        // `strip_prefix`, not `trim_start_matches`: exactly one leading slash is
        // the separator. Trimming all of them would turn `//a` into `a` and
        // accept it, which is the very form this is meant to reject.
        let body = topic.strip_prefix('/').unwrap_or(topic);
        validate_topic_components(body, "absolute topic")?;
        topic.to_string()
    } else if topic.starts_with('~') {
        // Private topic - expand with namespace and node name
        let topic_suffix = topic.strip_prefix('~').unwrap();
        let topic_suffix = topic_suffix.strip_prefix('/').unwrap_or(topic_suffix);
        // Strip a trailing slash here too. The absolute and relative branches
        // both normalize `a/` -> `a`; without this, `~/a/` alone was rejected,
        // an asymmetry with no justification.
        let topic_suffix = topic_suffix.strip_suffix('/').unwrap_or(topic_suffix);

        // Validate the topic suffix
        if !topic_suffix.is_empty() {
            validate_topic_components(topic_suffix, "private topic")?;
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
        validate_topic_components(topic, "topic")?;

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

    /// The absolute branch used to return unchecked, so these all succeeded.
    ///
    /// `create_client("/bad name", ..)` was accepted at construction and then
    /// failed inside zenoh's key-expression parser -- or worse, resolved to a
    /// name that could never match a peer. Table-driven so the rejected forms
    /// are visible as a set rather than buried in assertions.
    #[test]
    fn absolute_names_reject_invalid_components() {
        for bad in [
            "/bad name", // space
            "/a/b c",    // space in a later component
            "/1abc",     // must not start with a digit
            "/a/2b",     // ditto, later component
            "/a-b",      // hyphen is not a valid component character
            "/a.b",      // nor is a dot
            "/a//b",     // empty component
            "//a",       // empty leading component
            "/a/b//",    // empty component after trailing-slash strip
        ] {
            assert!(
                qualify_topic_name(bad, "/ns", "node").is_err(),
                "expected `{bad}` to be rejected"
            );
        }
    }

    #[test]
    fn absolute_names_still_accept_valid_components() {
        for good in [
            "/chatter",
            "/a/b/c",
            "/_private",
            "/a_1/b_2",
            "/chatter/", // trailing slash is stripped, not rejected
        ] {
            assert!(
                qualify_topic_name(good, "/ns", "node").is_ok(),
                "expected `{good}` to be accepted"
            );
        }
    }

    /// Empty components were previously *skipped* rather than rejected, on
    /// every branch, so `//a//b` passed ROS validation.
    #[test]
    fn empty_components_are_rejected_on_every_branch() {
        assert!(
            qualify_topic_name("/a//b", "/ns", "node").is_err(),
            "absolute"
        );
        assert!(
            qualify_topic_name("a//b", "/ns", "node").is_err(),
            "relative"
        );
        assert!(
            qualify_topic_name("~/a//b", "/ns", "node").is_err(),
            "private"
        );
    }

    /// The namespace is concatenated verbatim into the qualified name, so it
    /// needs the same rejection the topic gets. `//ns` used to validate --
    /// `validate_namespace` skipped empty components -- and produced
    /// `//ns/chatter`, the exact form the topic branch rejects.
    #[test]
    fn namespaces_reject_empty_components() {
        assert!(
            qualify_topic_name("chatter", "//ns", "node").is_err(),
            "//ns"
        );
        assert!(
            qualify_topic_name("chatter", "/ns//sub", "node").is_err(),
            "/ns//sub"
        );
        assert!(
            qualify_topic_name("chatter", "/ns/", "node").is_err(),
            "trailing slash"
        );
        // Still accepted: the single leading slash is the separator, not an
        // empty component, and a bare namespace needs no slash at all.
        assert!(qualify_topic_name("chatter", "/ns", "node").is_ok(), "/ns");
        assert!(qualify_topic_name("chatter", "ns", "node").is_ok(), "ns");
        assert!(qualify_topic_name("chatter", "/", "node").is_ok(), "/");
        assert!(qualify_topic_name("chatter", "", "node").is_ok(), "empty");
    }

    /// A trailing slash is normalized on all three branches, not two.
    ///
    /// The absolute and relative branches strip it before validating; the
    /// private branch did not, so `~/a/` alone was rejected while `/a/` and
    /// `a/` were accepted and normalized.
    #[test]
    fn trailing_slash_is_normalized_on_every_branch() {
        assert_eq!(qualify_topic_name("/a/", "/ns", "node").unwrap(), "/a");
        assert_eq!(qualify_topic_name("a/", "/ns", "node").unwrap(), "/ns/a");
        assert_eq!(
            qualify_topic_name("~/a/", "/ns", "node").unwrap(),
            "/ns/node/a"
        );
    }

    /// The "just slashes" family: only `//` had any assertion before.
    #[test]
    fn slash_only_names_are_rejected() {
        for bad in ["/", "//", "///"] {
            assert!(
                qualify_topic_name(bad, "/ns", "node").is_err(),
                "expected `{bad}` to be rejected"
            );
        }
    }

    /// Services and actions route through the same function, so the fix must
    /// reach them too -- the issue's reproduction is a client, not a topic.
    #[test]
    fn service_names_reject_invalid_absolute_components() {
        assert!(qualify_service_name("/bad name", "/ns", "node").is_err());
        assert!(qualify_service_name("/add_two_ints", "/ns", "node").is_ok());
    }
}
