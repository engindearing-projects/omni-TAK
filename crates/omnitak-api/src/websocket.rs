//! WebSocket API for real-time CoT message streaming and system events

use crate::auth::AuthService;
use crate::types::{WsClientMessage, WsServerMessage};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{
    sink::SinkExt,
    stream::{SplitSink, SplitStream, StreamExt},
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ============================================================================
// WebSocket State
// ============================================================================

#[derive(Clone)]
pub struct WsState {
    /// Broadcast channel for CoT messages
    cot_tx: broadcast::Sender<WsServerMessage>,
    /// Broadcast channel for system events
    event_tx: broadcast::Sender<WsServerMessage>,
    /// Authentication service (kept for future auth integration)
    #[allow(dead_code)]
    auth_service: Arc<AuthService>,
}

impl WsState {
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        let (cot_tx, _) = broadcast::channel(1000);
        let (event_tx, _) = broadcast::channel(100);

        Self {
            cot_tx,
            event_tx,
            auth_service,
        }
    }

    /// Broadcast a CoT message to all subscribers
    pub fn broadcast_cot_message(&self, message: WsServerMessage) {
        if let Err(e) = self.cot_tx.send(message) {
            debug!("No CoT subscribers: {}", e);
        }
    }

    /// Broadcast a system event to all subscribers
    pub fn broadcast_event(&self, message: WsServerMessage) {
        if let Err(e) = self.event_tx.send(message) {
            debug!("No event subscribers: {}", e);
        }
    }
}

// ============================================================================
// Router Setup
// ============================================================================

pub fn create_ws_router(state: WsState) -> Router {
    Router::new()
        .route("/api/v1/stream", get(ws_stream_handler))
        .route("/api/v1/events", get(ws_events_handler))
        .with_state(state)
}

// ============================================================================
// WebSocket Handlers
// ============================================================================

/// WS /api/v1/stream - Real-time CoT message stream
async fn ws_stream_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_stream_socket(socket, state))
}

/// WS /api/v1/events - System events stream
async fn ws_events_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_events_socket(socket, state))
}

// ============================================================================
// Stream Socket Handler
// ============================================================================

async fn handle_stream_socket(socket: WebSocket, state: WsState) {
    let client_id = Uuid::new_v4();
    info!(client_id = %client_id, "New WebSocket stream connection");

    let (sender, receiver) = socket.split();

    // Create channels for communication
    let (client_tx, client_rx) = mpsc::unbounded_channel();

    // Spawn sender task
    let send_task = tokio::spawn(handle_send_messages(sender, client_rx, client_id));

    // Spawn receiver task
    let recv_task = tokio::spawn(handle_receive_stream_messages(
        receiver,
        client_tx.clone(),
        state.clone(),
        client_id,
    ));

    // Wait for either task to complete
    tokio::select! {
        _ = send_task => {
            debug!(client_id = %client_id, "Send task completed");
        }
        _ = recv_task => {
            debug!(client_id = %client_id, "Receive task completed");
        }
    }

    info!(client_id = %client_id, "WebSocket stream connection closed");
}

// ============================================================================
// Events Socket Handler
// ============================================================================

async fn handle_events_socket(socket: WebSocket, state: WsState) {
    let client_id = Uuid::new_v4();
    info!(client_id = %client_id, "New WebSocket events connection");

    let (sender, receiver) = socket.split();

    // Create channels for communication
    let (client_tx, client_rx) = mpsc::unbounded_channel();

    // Subscribe to events immediately
    let mut event_rx = state.event_tx.subscribe();

    // Spawn event forwarding task
    tokio::spawn({
        let client_tx = client_tx.clone();
        async move {
            while let Ok(message) = event_rx.recv().await {
                if client_tx.send(message).is_err() {
                    break;
                }
            }
        }
    });

    // Spawn sender task
    let send_task = tokio::spawn(handle_send_messages(sender, client_rx, client_id));

    // Spawn receiver task (for ping/pong)
    let recv_task = tokio::spawn(handle_receive_events_messages(
        receiver, client_tx, client_id,
    ));

    // Wait for either task to complete
    tokio::select! {
        _ = send_task => {
            debug!(client_id = %client_id, "Send task completed");
        }
        _ = recv_task => {
            debug!(client_id = %client_id, "Receive task completed");
        }
    }

    info!(client_id = %client_id, "WebSocket events connection closed");
}

// ============================================================================
// Message Handling
// ============================================================================

async fn handle_send_messages(
    mut sender: SplitSink<WebSocket, Message>,
    mut client_rx: mpsc::UnboundedReceiver<WsServerMessage>,
    client_id: Uuid,
) {
    while let Some(message) = client_rx.recv().await {
        match serde_json::to_string(&message) {
            Ok(json) => {
                if let Err(e) = sender.send(Message::Text(json.into())).await {
                    error!(client_id = %client_id, error = %e, "Failed to send message");
                    break;
                }
            }
            Err(e) => {
                error!(client_id = %client_id, error = %e, "Failed to serialize message");
            }
        }
    }
}

// TODO: the per-client CoT broadcast receiver (`cot_rx`) is tracked here but not
// yet polled/forwarded to the client — server-push CoT streaming is unfinished.
#[allow(unused_assignments)]
async fn handle_receive_stream_messages(
    mut receiver: SplitStream<WebSocket>,
    client_tx: mpsc::UnboundedSender<WsServerMessage>,
    state: WsState,
    client_id: Uuid,
) {
    // Track subscription state
    let mut subscribed_cot = false;
    let mut cot_rx: Option<broadcast::Receiver<WsServerMessage>> = None;

    while let Some(result) = receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<WsClientMessage>(&text) {
                    Ok(client_msg) => {
                        match client_msg {
                            WsClientMessage::Subscribe {
                                event_types,
                                uids,
                                geo_bounds,
                                binary,
                            } => {
                                info!(
                                    client_id = %client_id,
                                    event_types = ?event_types,
                                    uids = ?uids,
                                    geo_bounds = ?geo_bounds,
                                    binary = binary,
                                    "Client subscribed to CoT stream"
                                );

                                // Subscribe to broadcast channel
                                if !subscribed_cot {
                                    cot_rx = Some(state.cot_tx.subscribe());
                                    subscribed_cot = true;

                                    // Spawn task to forward messages
                                    if let Some(mut rx) = cot_rx.take() {
                                        let client_tx = client_tx.clone();
                                        tokio::spawn(async move {
                                            while let Ok(message) = rx.recv().await {
                                                // TODO: Apply filters based on subscription
                                                if client_tx.send(message).is_err() {
                                                    break;
                                                }
                                            }
                                        });
                                    }
                                }

                                // Send acknowledgement
                                let _ = client_tx.send(WsServerMessage::Ack {
                                    message_type: "subscribe".to_string(),
                                });
                            }
                            WsClientMessage::Unsubscribe => {
                                info!(client_id = %client_id, "Client unsubscribed from CoT stream");
                                subscribed_cot = false;
                                cot_rx = None;

                                let _ = client_tx.send(WsServerMessage::Ack {
                                    message_type: "unsubscribe".to_string(),
                                });
                            }
                            WsClientMessage::SubscribeEvents => {
                                info!(client_id = %client_id, "Subscribe to events not supported on stream endpoint");
                                let _ = client_tx.send(WsServerMessage::Error {
                                    code: "invalid_endpoint".to_string(),
                                    message: "Use /api/v1/events for system events".to_string(),
                                });
                            }
                            WsClientMessage::UnsubscribeEvents => {
                                // No-op
                            }
                            WsClientMessage::Ping => {
                                let _ = client_tx.send(WsServerMessage::Pong);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(client_id = %client_id, error = %e, "Failed to parse client message");
                        let _ = client_tx.send(WsServerMessage::Error {
                            code: "parse_error".to_string(),
                            message: format!("Failed to parse message: {}", e),
                        });
                    }
                }
            }
            Ok(Message::Binary(_)) => {
                // TODO: Handle binary messages (protobuf/msgpack)
                warn!(client_id = %client_id, "Binary messages not yet implemented");
            }
            Ok(Message::Ping(_data)) => {
                debug!(client_id = %client_id, "Received ping");
                // Axum handles pong automatically
            }
            Ok(Message::Pong(_)) => {
                debug!(client_id = %client_id, "Received pong");
            }
            Ok(Message::Close(reason)) => {
                info!(client_id = %client_id, reason = ?reason, "Client closed connection");
                break;
            }
            Err(e) => {
                error!(client_id = %client_id, error = %e, "WebSocket error");
                break;
            }
        }
    }
}

async fn handle_receive_events_messages(
    mut receiver: SplitStream<WebSocket>,
    client_tx: mpsc::UnboundedSender<WsServerMessage>,
    client_id: Uuid,
) {
    while let Some(result) = receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<WsClientMessage>(&text) {
                    Ok(client_msg) => {
                        match client_msg {
                            WsClientMessage::Ping => {
                                let _ = client_tx.send(WsServerMessage::Pong);
                            }
                            WsClientMessage::Subscribe { .. } => {
                                warn!(client_id = %client_id, "Subscribe to CoT not supported on events endpoint");
                                let _ = client_tx.send(WsServerMessage::Error {
                                    code: "invalid_endpoint".to_string(),
                                    message: "Use /api/v1/stream for CoT messages".to_string(),
                                });
                            }
                            _ => {
                                // Ignore other messages on events endpoint
                            }
                        }
                    }
                    Err(e) => {
                        warn!(client_id = %client_id, error = %e, "Failed to parse client message");
                    }
                }
            }
            Ok(Message::Ping(_)) => {
                debug!(client_id = %client_id, "Received ping");
            }
            Ok(Message::Pong(_)) => {
                debug!(client_id = %client_id, "Received pong");
            }
            Ok(Message::Close(reason)) => {
                info!(client_id = %client_id, reason = ?reason, "Client closed connection");
                break;
            }
            Err(e) => {
                error!(client_id = %client_id, error = %e, "WebSocket error");
                break;
            }
            _ => {}
        }
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

impl WsState {
    /// Create a test CoT message for testing
    pub fn create_test_cot_message() -> WsServerMessage {
        WsServerMessage::CotMessage {
            id: Uuid::new_v4(),
            source_connection: Uuid::new_v4(),
            data: r#"<?xml version="1.0"?><event version="2.0" uid="test-123" type="a-f-G" time="2025-10-27T12:00:00Z" start="2025-10-27T12:00:00Z" stale="2025-10-27T12:05:00Z" how="m-g"><point lat="34.1234" lon="-118.5678" hae="100.0" ce="10.0" le="5.0"/><detail><contact callsign="TEST1"/></detail></event>"#.to_string(),
            event_type: "a-f-G".to_string(),
            uid: "test-123".to_string(),
            timestamp: chrono::Utc::now(),
        }
    }

    /// Create a test system event for testing
    pub fn create_test_system_event(
        event_type: &str,
        details: serde_json::Value,
    ) -> WsServerMessage {
        WsServerMessage::SystemEvent {
            event: event_type.to_string(),
            details,
            timestamp: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthConfig;

    #[test]
    fn test_ws_state_creation() {
        let auth_service = Arc::new(AuthService::new(AuthConfig::default()));
        let _state = WsState::new(auth_service);

        // Test message creation
        let cot_msg = WsState::create_test_cot_message();
        assert!(matches!(cot_msg, WsServerMessage::CotMessage { .. }));

        let event_msg = WsState::create_test_system_event(
            "connection_added",
            serde_json::json!({"connection_id": "123"}),
        );
        assert!(matches!(event_msg, WsServerMessage::SystemEvent { .. }));
    }

    #[tokio::test]
    async fn test_broadcast_channels() {
        let auth_service = Arc::new(AuthService::new(AuthConfig::default()));
        let state = WsState::new(auth_service);

        // Subscribe to CoT messages
        let mut rx = state.cot_tx.subscribe();

        // Broadcast a message
        let test_msg = WsState::create_test_cot_message();
        state.broadcast_cot_message(test_msg.clone());

        // Verify message received
        let received = rx.recv().await.unwrap();
        assert!(matches!(received, WsServerMessage::CotMessage { .. }));
    }
}
