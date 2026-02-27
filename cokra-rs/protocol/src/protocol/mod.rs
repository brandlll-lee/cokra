// Cokra Protocol Layer
// Core protocol definitions

/// Events emitted during Cokra operation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Event {
    /// Turn started
    TurnStarted(TurnStartedEvent),
    /// Item started
    ItemStarted(ItemStartedEvent),
    /// Item completed
    ItemCompleted(ItemCompletedEvent),
    /// Turn completed
    TurnCompleted(TurnCompletedEvent),
}

/// Operations submitted to Cokra
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Op {
    /// User turn
    UserTurn(UserTurn),
    /// Steer active turn
    Steer(Steer),
    /// Interrupt active turn
    Interrupt(Interrupt),
}

// Event types
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnStartedEvent {
    pub turn_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ItemStartedEvent {
    pub item_id: String,
    pub item_type: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ItemCompletedEvent {
    pub item_id: String,
    pub result: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnCompletedEvent {
    pub turn_id: String,
}

// Operation types
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserTurn {
    pub input: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Steer {
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Interrupt {
    pub reason: String,
}
