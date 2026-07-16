pub mod namespace;

pub use namespace::{
    BaseDomain, ClientSubdomain, FlatHostCatalog, MarketSlug, NamespaceError, PublicHost,
    PublicHostClaim, PublicHostKind, ShareSlug, MARKET_SLUG_MAX_LEN, MARKET_SLUG_MIN_LEN,
    PROTOCOL_EPOCH, PUBLIC_SLUG_MAX_LEN, PUBLIC_SLUG_MIN_LEN, SHARE_SLUG_MAX_LEN,
    SHARE_SLUG_MIN_LEN,
};
