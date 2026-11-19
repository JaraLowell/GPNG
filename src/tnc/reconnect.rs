//! Lost COM and KISS TCP links try again once a minute.
//! Ten failures in a row leave that modem closed until GPNG starts it again.

use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::{TncCommand, TncError};

pub const RETRY_EVERY: Duration = Duration::from_secs(60);
pub const RETRY_LIMIT: u8 = 10;

pub enum Attempt {
    /// Shutdown, or the operator closed the driver.
    Stop,
    /// The port or socket dropped. The caller counts this and may try again.
    Again(TncError),
}

/// Sleep for one minute. `false` means shutdown arrived and the caller must stop.
pub fn wait_retry(commands: &mut mpsc::Receiver<TncCommand>) -> bool {
    let end = Instant::now() + RETRY_EVERY;
    while Instant::now() < end {
        match commands.try_recv() {
            Ok(TncCommand::Shutdown) | Err(mpsc::error::TryRecvError::Disconnected) => {
                return false;
            }
            Ok(_) => {}
            Err(mpsc::error::TryRecvError::Empty) => {
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
    true
}

/// Stay up, red and idle, until shutdown. The other TFPCX modems keep running.
pub fn hold_until_shutdown(commands: &mut mpsc::Receiver<TncCommand>) {
    loop {
        match commands.blocking_recv() {
            None | Some(TncCommand::Shutdown) => return,
            Some(_) => {}
        }
    }
}
