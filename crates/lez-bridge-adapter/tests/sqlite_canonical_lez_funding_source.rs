//! Store-derived canonical LEZ funding authority.

#![forbid(unsafe_code)]

use lez_bridge_adapter::{SqliteCanonicalLezFundingSource, SqliteCanonicalLezFundingSourceError};

#[test]
fn production_store_derived_source_is_public() {
    fn type_is_public<T>() {}

    type_is_public::<SqliteCanonicalLezFundingSource>();
    type_is_public::<SqliteCanonicalLezFundingSourceError>();
}
