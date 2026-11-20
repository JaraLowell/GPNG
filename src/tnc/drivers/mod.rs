//! Modem drivers. `spawn_tnc_task` starts only the protocol that was opened.
//! The other `run` loops stay uncalled, so they do no I/O and own no port.
//! APRS is not one of these drivers: the UI reads it from monitor frames on
//! whichever radio is open, including a pure WA8DED link.

pub mod baycom;
pub mod kiss;
pub mod tfpcx;
pub mod wa8ded;
