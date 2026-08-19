pub mod namespace;

pub use namespace::{
    BaseDomain, ClientSubdomain, FlatHostCatalog, NamespaceError, PublicHost, PublicHostClaim,
    PublicHostKind, ShareSlug, PROTOCOL_EPOCH, PUBLIC_SLUG_MAX_LEN, PUBLIC_SLUG_MIN_LEN,
    SHARE_SLUG_MAX_LEN, SHARE_SLUG_MIN_LEN,
};
