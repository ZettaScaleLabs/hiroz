// Free-tier scale caps. Compiled away in the pro profile.

#[cfg(feature = "free-tier")]
pub const MAX_TOPICS: usize = 10;
#[cfg(feature = "free-tier")]
pub const MAX_RULES: usize = 5;
#[cfg(feature = "free-tier")]
pub const MAX_DOMAIN_PAIRS: usize = 1;

#[cfg(not(feature = "free-tier"))]
pub const MAX_TOPICS: usize = usize::MAX;
#[cfg(not(feature = "free-tier"))]
pub const MAX_RULES: usize = usize::MAX;
#[cfg(not(feature = "free-tier"))]
pub const MAX_DOMAIN_PAIRS: usize = usize::MAX;

pub fn check_topic_cap(count: usize) -> anyhow::Result<()> {
    if count > MAX_TOPICS {
        anyhow::bail!(
            "Free tier limit: at most {MAX_TOPICS} bridged topics/services. \
             Upgrade to hiroz-toolkit-pro for unlimited bridging."
        );
    }
    Ok(())
}

pub fn check_rule_cap(count: usize) -> anyhow::Result<()> {
    if count > MAX_RULES {
        anyhow::bail!(
            "Free tier limit: at most {MAX_RULES} remapping rules. \
             Upgrade to hiroz-toolkit-pro for unlimited rules."
        );
    }
    Ok(())
}

pub fn check_domain_pair_cap(count: usize) -> anyhow::Result<()> {
    if count > MAX_DOMAIN_PAIRS {
        anyhow::bail!(
            "Free tier limit: at most {MAX_DOMAIN_PAIRS} domain pair(s). \
             Upgrade to hiroz-toolkit-pro for unlimited bridging."
        );
    }
    Ok(())
}
