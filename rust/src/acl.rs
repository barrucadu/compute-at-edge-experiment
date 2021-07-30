use ipnet::Ipv4Net;
use iprange::IpRange;
use std::net::Ipv4Addr;

lazy_static! {
    static ref PURGE_IP_ALLOWLIST: IpRange<Ipv4Net> = [
        "37.26.93.252",     // Skyscape mirrors
        "31.210.241.100",   // Carrenza mirrors
        "23.235.32.0/20",   // Fastly cache node
        "43.249.72.0/22",   // Fastly cache node
        "103.244.50.0/24",  // Fastly cache node
        "103.245.222.0/23", // Fastly cache node
        "103.245.224.0/24", // Fastly cache node
        "104.156.80.0/20",  // Fastly cache node
        "151.101.0.0/16",   // Fastly cache node
        "157.52.64.0/18",   // Fastly cache node
        "172.111.64.0/18",  // Fastly cache node
        "185.31.16.0/22",   // Fastly cache node
        "199.27.72.0/21",   // Fastly cache node
        "199.232.0.0/16",   // Fastly cache node
        "202.21.128.0/24",  // Fastly cache node
        "203.57.145.0/24",  // Fastly cache node
        "167.82.0.0/17",    // Fastly cache node
        "167.82.128.0/20",  // Fastly cache node
        "167.82.160.0/20",  // Fastly cache node
        "167.82.224.0/20",  // Fastly cache node
    ].iter().map(|s| s.parse().unwrap()).collect();
}

#[cfg(govuk_environment = "integration")]
lazy_static! {
    static ref ENVIRONMENT_PURGE_IP_ALLOWLIST: IpRange<Ipv4Net> = [
        "34.248.229.46",    // AWS Integration NAT gateway
        "34.248.44.175",    // AWS Integration NAT gateway
        "52.51.97.232",     // AWS Integration NAT gateway
    ].iter().map(|s| s.parse().unwrap()).collect();
}

#[cfg(govuk_environment = "staging")]
lazy_static! {
    static ref ENVIRONMENT_PURGE_IP_ALLOWLIST: IpRange<Ipv4Net> = [
        "31.210.245.70",    // Carrenza Staging
        "18.202.183.143",   // AWS NAT GW1
        "18.203.90.80",     // AWS NAT GW2
        "18.203.108.248",   // AWS NAT GW3
    ].iter().map(|s| s.parse().unwrap()).collect();
}

#[cfg(govuk_environment = "production")]
lazy_static! {
    static ref ENVIRONMENT_PURGE_IP_ALLOWLIST: IpRange<Ipv4Net> = [
        "31.210.245.86",    // Carrenza Production
        "34.246.209.74",    // AWS NAT GW1
        "34.253.57.8",      // AWS NAT GW2
        "18.202.136.43",    // AWS NAT GW3
    ].iter().map(|s| s.parse().unwrap()).collect();
}

#[cfg(not(any(
    govuk_environment = "integration",
    govuk_environment = "staging",
    govuk_environment = "production"
)))]
compile_error!(
    "Set 'govuk_environment' to one of 'integration', 'staging', or 'production' in RUSTFLAGS."
);

/// Check if an IP can purge
pub fn ip_is_on_purge_allowlist(ip: Ipv4Addr) -> bool {
    ENVIRONMENT_PURGE_IP_ALLOWLIST.contains(&ip) || PURGE_IP_ALLOWLIST.contains(&ip)
}
