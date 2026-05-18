//! Key expression helpers for Humble ↔ Jazzy translation.
//!
//! Humble topic KE:  `{domain}/{topic}/TYPE_NAME/TypeHashNotSupported`
//! Jazzy  topic KE:  `{domain}/{topic}/TYPE_NAME/RIHS01_<hex>`
//!
//! The bridge rewrites the hash segment so each side receives what it expects.

use hiroz_protocol::TypeHash;

/// Humble's sentinel value for the type hash segment of a key expression.
pub const HUMBLE_HASH_SENTINEL: &str = "TypeHashNotSupported";

/// Classify how a topic key expression encodes the type hash.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashVariant {
    /// ROS 2 Humble: hash segment is `TypeHashNotSupported`
    Humble,
    /// ROS 2 Jazzy (and later): hash segment is `RIHS01_<hex>`
    Jazzy(TypeHash),
}

/// Parsed representation of an rmw_zenoh topic key expression.
///
/// Format: `{domain_id}/{topic_path}/{type_name}/{type_hash}`
///
/// The `topic_path` may contain multiple `/`-separated segments (no mangling
/// for topic KEs — only liveliness tokens use `%` mangling).
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct ParsedTopicKe {
    pub domain_id: u32,
    /// Full topic path (may contain slashes), e.g. `chatter` or `ns/topic`
    pub topic_path: String,
    /// Type name segment, e.g. `std_msgs::msg::dds_::String_`
    pub type_name: String,
    /// Hash variant parsed from the trailing segment
    pub hash: HashVariant,
}

#[cfg(test)]
impl ParsedTopicKe {
    /// Parse a raw key expression string into a `ParsedTopicKe`.
    ///
    /// The format is `{domain_id}/{topic...}/{type_name}/{hash}` where
    /// `{topic...}` may span multiple segments. We identify the type name
    /// and hash as the **last two** segments, domain_id as the **first**.
    pub fn parse(ke: &str) -> anyhow::Result<Self> {
        let segments: Vec<&str> = ke.split('/').collect();
        if segments.len() < 4 {
            anyhow::bail!("topic KE too short: {ke}");
        }

        let domain_id: u32 = segments[0]
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid domain_id in KE: {ke}"))?;

        let hash_str = segments[segments.len() - 1];
        let type_name = segments[segments.len() - 2].to_string();
        let topic_path = segments[1..segments.len() - 2].join("/");

        let hash = parse_hash_segment(hash_str)?;

        Ok(Self {
            domain_id,
            topic_path,
            type_name,
            hash,
        })
    }

    /// Rewrite this KE replacing the hash with a Humble-style hash.
    pub fn as_humble_ke(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.domain_id, self.topic_path, self.type_name, HUMBLE_HASH_SENTINEL,
        )
    }

    /// Rewrite this KE replacing the hash with the given RIHS01 hash.
    pub fn as_jazzy_ke(&self, hash: &TypeHash) -> String {
        format!(
            "{}/{}/{}/{}",
            self.domain_id,
            self.topic_path,
            self.type_name,
            hash.to_rihs_string(),
        )
    }

    /// Returns the subscription wildcard that matches both Humble and Jazzy
    /// variants of this topic/type combination.
    pub fn wildcard_ke(&self) -> String {
        format!(
            "{}/{}/{}/**",
            self.domain_id, self.topic_path, self.type_name
        )
    }
}

/// Parse the hash segment of a topic KE.
#[cfg(test)]
fn parse_hash_segment(hash_str: &str) -> anyhow::Result<HashVariant> {
    if hash_str == HUMBLE_HASH_SENTINEL {
        return Ok(HashVariant::Humble);
    }
    if let Some(hash) = TypeHash::from_rihs_string(hash_str) {
        return Ok(HashVariant::Jazzy(hash));
    }
    anyhow::bail!("unrecognised hash segment: {hash_str}")
}

/// Return `true` if `hash` represents the Humble sentinel (all-zero value).
pub fn is_humble_hash(hash: &TypeHash) -> bool {
    *hash == TypeHash::zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAZZY_KE: &str = "0/chatter/std_msgs::msg::dds_::String_/RIHS01_9c0c1c7e2bf9d65b2f5a6b62bab0ece9b4ed68c701234567890abcdef1234567";
    const HUMBLE_KE: &str = "0/chatter/std_msgs::msg::dds_::String_/TypeHashNotSupported";
    const NS_KE: &str = "0/ns/sub/chatter/std_msgs::msg::dds_::String_/TypeHashNotSupported";

    #[test]
    fn parse_jazzy_ke() {
        let parsed = ParsedTopicKe::parse(JAZZY_KE).unwrap();
        assert_eq!(parsed.domain_id, 0);
        assert_eq!(parsed.topic_path, "chatter");
        assert_eq!(parsed.type_name, "std_msgs::msg::dds_::String_");
        assert!(matches!(parsed.hash, HashVariant::Jazzy(_)));
    }

    #[test]
    fn parse_humble_ke() {
        let parsed = ParsedTopicKe::parse(HUMBLE_KE).unwrap();
        assert_eq!(parsed.domain_id, 0);
        assert_eq!(parsed.topic_path, "chatter");
        assert!(matches!(parsed.hash, HashVariant::Humble));
    }

    #[test]
    fn parse_namespaced_ke() {
        let parsed = ParsedTopicKe::parse(NS_KE).unwrap();
        assert_eq!(parsed.topic_path, "ns/sub/chatter");
    }

    #[test]
    fn rewrite_humble_to_jazzy() {
        let parsed = ParsedTopicKe::parse(HUMBLE_KE).unwrap();
        let hash = TypeHash::new(1, [0xabu8; 32]);
        let jazzy = parsed.as_jazzy_ke(&hash);
        assert!(jazzy.contains("RIHS01_"));
        assert!(jazzy.ends_with(&"ab".repeat(32)));
    }

    #[test]
    fn rewrite_jazzy_to_humble() {
        let parsed = ParsedTopicKe::parse(JAZZY_KE).unwrap();
        let humble = parsed.as_humble_ke();
        assert!(humble.ends_with("TypeHashNotSupported"));
    }

    #[test]
    fn wildcard_ke() {
        let parsed = ParsedTopicKe::parse(HUMBLE_KE).unwrap();
        let wc = parsed.wildcard_ke();
        assert_eq!(wc, "0/chatter/std_msgs::msg::dds_::String_/**");
    }

    #[test]
    fn too_short_ke_errors() {
        assert!(ParsedTopicKe::parse("0/chatter").is_err());
        assert!(ParsedTopicKe::parse("0/chatter/type").is_err());
    }

    #[test]
    fn is_humble_hash_sentinel() {
        let zero = TypeHash::zero();
        assert!(is_humble_hash(&zero));
        let non_zero = TypeHash::new(1, [1u8; 32]);
        assert!(!is_humble_hash(&non_zero));
    }

    #[test]
    fn rihs_string_format() {
        let hash = TypeHash::new(1, [0u8; 32]);
        let s = hash.to_rihs_string();
        assert!(s.starts_with("RIHS01_"));
        assert_eq!(s.len(), 7 + 64);
    }
}
