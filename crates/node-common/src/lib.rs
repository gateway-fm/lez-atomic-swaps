//! Role-neutral, least-authority runtime boundaries shared by Maker and Taker Nodes.

mod chat_contracts;
pub mod local_rpc;
pub mod logos_chat_gateway;
pub mod node_config;
pub mod owner_rpc_server;
pub mod run_local_delivery;
pub mod secure_file;
pub mod service_control;

mod maker_identity;

pub use chat_contracts::{
    BtcChatCompleteRequestV1, BtcChatCompleteRequestV2, BtcChatCompleteResponseV1,
    BtcChatCompleteResponseV2, BtcChatProposalV1, BtcChatProposalV2, BtcChatProposeRequestV1,
    BtcChatProposeRequestV2, XmrChatActivateRequestV1, XmrChatActivateResponseV1,
    XmrChatStageARequestV1, XmrChatStageAResponseV1, ZecChatCompleteRequestV1,
    ZecChatCompleteResponseV1, ZecChatProposalV1, ZecChatProposeRequestV1,
};
pub use local_rpc::{call_local_chat_gateway_rpc, call_local_chat_rpc, call_local_rpc};
pub use logos_chat_gateway::{
    LOGOS_CHAT_GATEWAY_METHODS_V1, LogosChatGateway, LogosChatGatewayAckV1,
    LogosChatGatewayBindRequestV1, LogosChatGatewayError, LogosChatGatewayIngestRequestV1,
    LogosChatGatewayOutboxAckRequestV1, LogosChatGatewayOutboxItemV1,
    LogosChatGatewayOutboxRequestV1, LogosChatGatewayResetRequestV1, LogosChatGatewayRoleV1,
    LogosChatGatewayStatusRequestV1, LogosChatGatewayStatusV1, LogosOfferIngestRequestV1,
    LogosOfferListRequestV1, LogosOfferListV1, LogosOfferSelectRequestV1, LogosOfferSelectionV1,
    LogosOfferViewV1, logos_chat_gateway_control_rpc_module, logos_chat_gateway_proxy_rpc_module,
};
pub use maker_identity::TakerMakerIdentityV1;
pub use run_local_delivery::{
    AuthenticatedLogosOfferAnnouncementV1, AuthenticatedOfferRefV1, DeliveryOfferQueryV1,
    DeliveryPublicationV1, LOGOS_OFFER_ANNOUNCEMENT_TTL_SECONDS_V1, LOGOS_OFFER_CONTENT_TOPIC_V1,
    LOGOS_OFFER_REBROADCAST_SECONDS_V1, RunLocalDelivery, RunLocalDeliveryError,
    verify_logos_offer_announcement,
};
pub use service_control::{
    NodeServiceAction, NodeServiceControlError, NodeServiceControlV1, control_maker_service,
    control_taker_service, shutdown_signal,
};
