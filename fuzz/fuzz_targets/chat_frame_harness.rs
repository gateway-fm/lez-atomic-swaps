use lez_node_common::{
    LogosChatGateway, LogosChatGatewayBindRequestV1, LogosChatGatewayIngestRequestV1,
    LogosChatGatewayOutboxRequestV1, LogosChatGatewayRoleV1, LogosChatGatewayStatusRequestV1,
    LogosChatGatewayStatusV1,
};
use std::{path::PathBuf, sync::Arc};

const CONVERSATION: &str = "fuzz-conversation";
const MAKER_ADDRESS: &str = "logos://maker-fuzz";
const TAKER_ADDRESS: &str = "logos://taker-fuzz";

/// Replays one fuzz input as the text of a Chat message from the bound peer, at both ends: the
/// Maker gateway (which accepts requests) and the Taker gateway (which accepts responses).
pub fn run(data: &[u8]) {
    let Ok(content) = std::str::from_utf8(data) else {
        return;
    };
    // An accepted Maker request is handed to a task that would call the Maker's own Chat
    // socket; the runtime is never driven, so that task never runs and no I/O happens.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fuzz runtime must build");
    let _entered = runtime.enter();
    for (role, local, peer) in [
        (LogosChatGatewayRoleV1::Maker, MAKER_ADDRESS, TAKER_ADDRESS),
        (LogosChatGatewayRoleV1::Taker, TAKER_ADDRESS, MAKER_ADDRESS),
    ] {
        let socket = (role == LogosChatGatewayRoleV1::Maker)
            .then(|| PathBuf::from("/nonexistent/maker-chat.sock"));
        let gateway = Arc::new(
            LogosChatGateway::new_with_clock(role, socket, || Ok(1))
                .expect("fixed gateway must construct"),
        );
        gateway
            .bind_session(&LogosChatGatewayBindRequestV1 {
                schema_version: 1,
                conversation_id: CONVERSATION.into(),
                local_address: local.into(),
                peer_address: peer.into(),
            })
            .expect("fixed session must bind");
        let request = LogosChatGatewayIngestRequestV1 {
            schema_version: 1,
            conversation_id: CONVERSATION.into(),
            sender_address: peer.into(),
            content: content.into(),
        };
        let before = status(&gateway);
        if gateway.ingest(&request).is_ok() {
            let again = gateway
                .ingest(&request)
                .expect("an accepted frame must be accepted again");
            assert!(again.was_replay, "the same frame was processed twice");
        } else {
            assert_eq!(status(&gateway), before, "rejected frame left state");
            assert!(
                gateway
                    .outbox_peek(LogosChatGatewayOutboxRequestV1 { schema_version: 1 })
                    .expect("outbox must be readable")
                    .is_none(),
                "rejected frame queued a reply"
            );
        }
    }
}

fn status(gateway: &LogosChatGateway) -> LogosChatGatewayStatusV1 {
    gateway
        .status(LogosChatGatewayStatusRequestV1 { schema_version: 1 })
        .expect("gateway status must be readable")
}
