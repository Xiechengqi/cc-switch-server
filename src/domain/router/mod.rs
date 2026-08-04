pub mod namespace;
pub mod server_logs;

pub use namespace::{
    BaseDomain, ClientSubdomain, FlatHostCatalog, MarketSlug, NamespaceError, PublicHost,
    PublicHostClaim, PublicHostKind, ShareSlug, MARKET_SLUG_MAX_LEN, MARKET_SLUG_MIN_LEN,
    PROTOCOL_EPOCH, PUBLIC_SLUG_MAX_LEN, PUBLIC_SLUG_MIN_LEN, SHARE_SLUG_MAX_LEN,
    SHARE_SLUG_MIN_LEN,
};
pub use server_logs::{
    InstallationLogBatchPayload, InstallationLogBatchResponse, InstallationLogEvent,
    INSTALLATION_LOG_BATCH_ACTION, INSTALLATION_LOG_PROTOCOL_VERSION,
};
