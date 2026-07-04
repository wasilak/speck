use speck_net::resolver_table::ResolverTable;
use std::net::IpAddr;

#[test]
fn empty_table_returns_none() {
    let table = ResolverTable::default();
    assert!(table.find_resolver("example.com").is_none());
}

#[test]
fn exact_match_returns_servers() {
    let mut table = ResolverTable::default();
    let servers: Vec<IpAddr> = vec!["127.0.0.1".parse().unwrap()];
    table.add_entry("corp".to_string(), servers.clone());
    let result = table.find_resolver("corp");
    assert_eq!(result, Some(servers.as_slice()));
}

#[test]
fn suffix_match_single_label() {
    let mut table = ResolverTable::default();
    let servers: Vec<IpAddr> = vec!["10.0.0.1".parse().unwrap()];
    table.add_entry("corp".to_string(), servers.clone());
    let result = table.find_resolver("internal.corp");
    assert_eq!(result, Some(servers.as_slice()));
}

#[test]
fn longest_suffix_wins() {
    let mut table = ResolverTable::default();
    let broad: Vec<IpAddr> = vec!["1.1.1.1".parse().unwrap()];
    let narrow: Vec<IpAddr> = vec!["10.0.0.1".parse().unwrap()];
    table.add_entry("example.com".to_string(), broad);
    table.add_entry("corp.example.com".to_string(), narrow.clone());
    let result = table.find_resolver("host.corp.example.com");
    assert_eq!(result, Some(narrow.as_slice()));
}

#[test]
fn no_match_returns_none() {
    let mut table = ResolverTable::default();
    let servers: Vec<IpAddr> = vec!["10.0.0.1".parse().unwrap()];
    table.add_entry("corp".to_string(), servers);
    assert!(table.find_resolver("example.com").is_none());
}
