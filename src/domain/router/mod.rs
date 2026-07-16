pub mod namespace;

pub use namespace::{
    BaseDomain, ClientKey, FlatHostCatalog, MarketSlug, NamespaceError, PublicHost,
    PublicHostClaim, PublicHostKind, ShareSlug, CLIENT_FINGERPRINT_LEN, CLIENT_PREFIX_MAX_LEN,
    CLIENT_PREFIX_MIN_LEN, MARKET_SLUG_MAX_LEN, MARKET_SLUG_MIN_LEN, PROTOCOL_EPOCH,
    SHARE_SLUG_MAX_LEN, SHARE_SLUG_MIN_LEN,
};
