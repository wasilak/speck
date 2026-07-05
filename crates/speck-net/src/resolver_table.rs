use std::collections::HashMap;
use std::ffi::c_void;
use std::net::IpAddr;
use std::sync::Arc;

// Use core_foundation types from system_configuration's re-export (0.9.4) to avoid
// version mismatch with the SCDynamicStore API which also uses 0.9.4 internally.
use system_configuration::core_foundation::{
    array::CFArray,
    base::{CFType, TCFType, ToVoid},
    dictionary::CFDictionary,
    string::CFString,
};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use system_configuration::{
    dynamic_store::{SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext},
    sys::{
        dynamic_store::SCDynamicStoreSetDispatchQueue,
        schema_definitions::{kSCPropNetDNSServerAddresses, kSCPropNetDNSSupplementalMatchDomains},
    },
};
use tokio::sync::watch;
use tracing::{debug, warn};

/// Per-domain resolver entries populated from SCDynamicStore VPN service entries.
/// Only services with non-empty SupplementalMatchDomains are added — default (non-VPN)
/// resolvers are skipped so they do not intercept general traffic.
#[derive(Debug, Clone, Default)]
pub struct ResolverTable {
    entries: HashMap<String, Vec<IpAddr>>,
}

impl ResolverTable {
    /// Insert a domain → nameserver mapping. No-ops when `servers` is empty.
    pub fn add_entry(&mut self, domain: String, servers: Vec<IpAddr>) {
        if !servers.is_empty() {
            self.entries.insert(domain, servers);
        }
    }

    /// Return true when the table has no entries (no VPN-scoped resolvers present).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Longest-suffix match: for "host.corp.example" tries "host.corp.example",
    /// "corp.example", "example" in order. Returns the first match.
    /// Returns `None` when no VPN-scoped entry matches — the caller falls back
    /// to `getaddrinfo`.
    pub fn find_resolver(&self, domain: &str) -> Option<&[IpAddr]> {
        let labels: Vec<&str> = domain.split('.').collect();
        for start in 0..labels.len() {
            let suffix = labels[start..].join(".");
            if let Some(servers) = self.entries.get(&suffix) {
                return Some(servers);
            }
        }
        None
    }
}

/// Perform a one-shot read of VPN-scoped DNS resolvers from SCDynamicStore.
///
/// Opens a short-lived SCDynamicStore session (no GCD queue, no Box::leak),
/// reads the current resolver table, and returns it. Used by `spk doctor` to
/// check for VPN-injected split-DNS entries without starting the live watcher.
///
/// Returns an empty `ResolverTable` if the store cannot be opened (logs a warning).
pub fn read_resolver_table_once() -> ResolverTable {
    match SCDynamicStoreBuilder::new("speck-doctor-dns-check").build() {
        None => {
            warn!("could not open SCDynamicStore for one-shot resolver read");
            ResolverTable::default()
        }
        Some(store) => read_resolver_table(&store),
    }
}

/// GCD callback — called on the `speck.dns.watcher` serial queue whenever a
/// watched SCDynamicStore key changes.
fn resolver_changed(
    store: SCDynamicStore,
    _changed_keys: CFArray<CFString>,
    tx: &mut Arc<watch::Sender<ResolverTable>>,
) {
    let table = read_resolver_table(&store);
    debug!(entries = table.entries.len(), "resolver table updated");
    let _ = tx.send(table);
}

/// Start the SCDynamicStore watcher and return a `(Sender, Receiver)` pair.
///
/// The watcher registers GCD dispatch queue callbacks for DNS service key
/// changes via `SCDynamicStoreSetDispatchQueue` — the only correct notification
/// model (not CFRunLoop). On each notification it rebuilds the `ResolverTable`
/// and sends the new value through the watch channel.
///
/// The store and GCD queue are leaked so they remain alive for the entire
/// process lifetime.
pub fn spawn_resolver_watcher() -> (
    Arc<watch::Sender<ResolverTable>>,
    watch::Receiver<ResolverTable>,
) {
    let (tx, rx) = watch::channel(ResolverTable::default());
    let tx = Arc::new(tx);

    let callback_tx = Arc::clone(&tx);
    let callback_context = SCDynamicStoreCallBackContext {
        callout: resolver_changed,
        info: callback_tx,
    };

    let store = match SCDynamicStoreBuilder::new("speck-dns-watcher")
        .callback_context(callback_context)
        .build()
    {
        Some(s) => s,
        None => {
            warn!("failed to create SCDynamicStore session; DNS live-update disabled");
            return (tx, rx);
        }
    };

    // Register patterns: per-service DNS entries and global DNS.
    let watch_keys: CFArray<CFString> = CFArray::from_CFTypes(&[]);
    let watch_patterns: CFArray<CFString> = CFArray::from_CFTypes(&[
        CFString::from("State:/Network/Service/[^/]+/DNS"),
        CFString::from("State:/Network/Global/DNS"),
    ]);
    if !store.set_notification_keys(&watch_keys, &watch_patterns) {
        warn!("SCDynamicStoreSetNotificationKeys failed; DNS live-update disabled");
        return (tx, rx);
    }

    // Create GCD serial queue and wire it as the callback delivery mechanism.
    // CRITICAL: SCDynamicStoreSetDispatchQueue must be used — CFRunLoopAddSource
    // is the wrong model and produces silent no-ops (see STATE.md D-09).
    let queue = DispatchQueue::new("speck.dns.watcher", DispatchQueueAttr::SERIAL);
    // SAFETY: DispatchQueue is #[repr(C)]; the pointer to it is the dispatch_queue_t.
    let raw_queue: *mut c_void = (&*queue as *const DispatchQueue) as *mut c_void;

    let ok = unsafe {
        SCDynamicStoreSetDispatchQueue(store.as_concrete_TypeRef(), raw_queue)
    };
    if ok == 0 {
        warn!("SCDynamicStoreSetDispatchQueue failed; DNS live-update disabled");
        return (tx, rx);
    }

    // Read initial resolver state synchronously before any callbacks can fire.
    let initial = read_resolver_table(&store);
    debug!(entries = initial.entries.len(), "initial resolver table loaded");
    let _ = tx.send(initial);

    // Leak the store and queue: they must stay alive for the process lifetime so
    // that GCD callback delivery continues and the SCDynamicStore session remains open.
    Box::leak(Box::new(store));
    Box::leak(Box::new(queue));

    (tx, rx)
}

/// Read all per-service DNS entries from SCDynamicStore and build a `ResolverTable`.
///
/// Only services that carry `SupplementalMatchDomains` are included — these are
/// VPN-injected resolvers. Services without this key are default resolvers and
/// are handled by `getaddrinfo` on the regular code path.
fn read_resolver_table(store: &SCDynamicStore) -> ResolverTable {
    let mut table = ResolverTable::default();

    let keys = match store.get_keys("State:/Network/Service/.*/DNS") {
        Some(k) => k,
        None => {
            debug!("no DNS service keys found in SCDynamicStore");
            return table;
        }
    };

    for key_ref in keys.iter() {
        let key_string = (*key_ref).to_string();

        let pl = match store.get(key_string.as_str()) {
            Some(p) => p,
            None => continue,
        };

        let dict = match pl.downcast_into::<CFDictionary>() {
            Some(d) => d,
            None => continue,
        };

        // Only VPN services carry SupplementalMatchDomains.
        let domains = extract_string_array(
            &dict,
            // SAFETY: kSCPropNetDNSSupplementalMatchDomains is a stable Apple CF constant.
            unsafe { kSCPropNetDNSSupplementalMatchDomains }.to_void(),
        );
        if domains.is_empty() {
            continue;
        }

        let server_strings = extract_string_array(
            &dict,
            // SAFETY: kSCPropNetDNSServerAddresses is a stable Apple CF constant.
            unsafe { kSCPropNetDNSServerAddresses }.to_void(),
        );
        let servers: Vec<IpAddr> = server_strings
            .iter()
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .collect();

        for domain in domains {
            table.add_entry(domain, servers.clone());
        }
    }

    debug!(count = table.entries.len(), "read resolver table from SCDynamicStore");
    table
}

/// Extract a `Vec<String>` from a value at `key` inside `dict`.
/// The value must be a `CFArray` of `CFString` items; other types are ignored.
fn extract_string_array(dict: &CFDictionary, key: *const c_void) -> Vec<String> {
    let arr = match dict
        .find(key)
        .map(|ptr| unsafe { CFType::wrap_under_get_rule(*ptr) })
        .and_then(|t| t.downcast_into::<CFArray>())
    {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut result = Vec::with_capacity(arr.len() as usize);
    for item_ptr in &arr {
        if let Some(s) =
            unsafe { CFType::wrap_under_get_rule(*item_ptr) }.downcast_into::<CFString>()
        {
            result.push(s.to_string());
        }
    }
    result
}
