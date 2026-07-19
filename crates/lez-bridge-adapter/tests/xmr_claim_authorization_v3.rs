//! M4 authenticated Stage-B claim-authorization capability contract.

use lez_bridge_adapter::PreparedXmrClaimAuthorizationEvidenceV3;

#[test]
fn claim_authorization_capability_is_public_but_opaque() {
    assert!(std::mem::size_of::<PreparedXmrClaimAuthorizationEvidenceV3>() > 0);
}
