//! Plain HTTPS GETs (rule-sets, sing-box releases; later subscriptions and app updates).

use std::time::Duration;

/// GET `url` with the ProxyDock user agent; `accept` sets the Accept header ("" = none). At most `limit` bytes.
pub fn get(url: &str, accept: &str, timeout: Duration, limit: u64) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).user_agent("ProxyDock").build().into();
    let mut req = agent.get(url);
    if !accept.is_empty() {
        req = req.header("Accept", accept);
    }
    let mut r = req.call().map_err(|e| e.to_string())?;
    r.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())
}
