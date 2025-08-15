use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ServerStatus {
    Offline,
    WakingUp,
    Online,
}

pub type SharedServerState = Arc<Mutex<ServerStatus>>;

pub fn new_shared_state() -> SharedServerState {
    Arc::new(Mutex::new(ServerStatus::Offline))
}
