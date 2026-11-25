//! Serial KISS COM ports are closed and brought up again after 12 quiet hours.
//! A live AX.25 circuit postpones that by one hour, three times.
//! The next check then resets the port anyway. TCP KISS is not scheduled.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::config::{HostProtocol, TncConfig, TncLink};

use super::LinkPhase;

const QUIET_FOR: Duration = Duration::from_secs(12 * 60 * 60);
const BUSY_WAIT: Duration = Duration::from_secs(60 * 60);
const BUSY_WAITS: u8 = 3;

/// One serial KISS modem. `ports` empty means every circuit on that COM port.
struct Refresh {
    index: u8,
    ports: BTreeSet<u8>,
    label: String,
    open: bool,
    next: Option<Instant>,
    waits: u8,
}

pub(super) struct Job {
    pub port: u8,
    pub ports: BTreeSet<u8>,
    pub forced: bool,
}

#[derive(Default)]
pub(super) struct Due {
    pub lines: Vec<String>,
    pub jobs: Vec<Job>,
}

pub(super) struct Plan {
    links: Vec<Refresh>,
}

impl Plan {
    pub(super) fn from_config(config: &TncConfig) -> Self {
        let mut links = Vec::new();
        match config.protocol_kind() {
            HostProtocol::Kiss if !config.kiss_use_tcp() => {
                links.push(Refresh::new(0, BTreeSet::new()));
            }
            HostProtocol::Tfpcx => {
                if let Ok(validated) = config.validated_links() {
                    for (index, link) in validated.iter().enumerate() {
                        if serial_kiss(link) {
                            links.push(Refresh::new(
                                index as u8,
                                link.ports.iter().copied().collect(),
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
        Self { links }
    }

    /// Arm on open. A dropped port waits until the driver opens it again.
    pub(super) fn note(&mut self, index: u8, label: &str, phase: LinkPhase, now: Instant) {
        let Some(link) = self.links.iter_mut().find(|link| link.index == index) else {
            return;
        };
        if !label.is_empty() {
            link.label = label.to_string();
        }
        match phase {
            LinkPhase::Open => {
                link.open = true;
                link.waits = 0;
                link.next = Some(now + QUIET_FOR);
            }
            LinkPhase::Pending | LinkPhase::Retry | LinkPhase::Closed => {
                link.open = false;
                link.waits = 0;
                link.next = None;
            }
        }
    }

    pub(super) fn poll(&mut self, now: Instant, busy: impl Fn(&BTreeSet<u8>) -> bool) -> Due {
        let mut due = Due::default();
        for link in &mut self.links {
            let Some(at) = link.next else {
                continue;
            };
            if !link.open || now < at {
                continue;
            }
            let occupied = busy(&link.ports);
            if !occupied || link.waits >= BUSY_WAITS {
                let forced = occupied;
                link.waits = 0;
                link.next = Some(now + QUIET_FOR);
                due.lines.push(if forced {
                    link.say("serial refresh, still connected after 3 waits")
                } else {
                    link.say("serial refresh, idle")
                });
                due.jobs.push(Job {
                    port: link.ports.iter().copied().next().unwrap_or(0),
                    ports: link.ports.clone(),
                    forced,
                });
            } else {
                link.waits += 1;
                link.next = Some(now + BUSY_WAIT);
                due.lines.push(link.say(&format!(
                    "serial refresh in 1h, still connected ({} of {BUSY_WAITS})",
                    link.waits
                )));
            }
        }
        due
    }
}

impl Refresh {
    fn new(index: u8, ports: BTreeSet<u8>) -> Self {
        Self {
            index,
            ports,
            label: String::new(),
            open: false,
            next: None,
            waits: 0,
        }
    }

    fn say(&self, rest: &str) -> String {
        let name = if self.label.is_empty() {
            "serial"
        } else {
            self.label.as_str()
        };
        format!("{name}: {rest}")
    }
}

fn serial_kiss(link: &TncLink) -> bool {
    link.is_kiss() && !link.uses_tcp()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, Instant};

    use crate::config::{TncConfig, TncLink};

    use super::super::LinkPhase;
    use super::{Plan, BUSY_WAIT, BUSY_WAITS, QUIET_FOR};

    fn open(plan: &mut Plan, index: u8, at: Instant) {
        plan.note(index, "COM6", LinkPhase::Open, at);
    }

    #[test]
    fn idle_port_resets_at_12_hours_and_not_before() {
        let start = Instant::now();
        let mut plan = Plan::from_config(&TncConfig {
            protocol: "kiss".into(),
            kiss_transport: "serial".into(),
            ..TncConfig::default()
        });
        open(&mut plan, 0, start);
        assert!(plan
            .poll(start + QUIET_FOR - Duration::from_secs(1), |_| false)
            .jobs
            .is_empty());
        let due = plan.poll(start + QUIET_FOR, |_| false);
        assert_eq!(due.jobs.len(), 1);
        assert!(!due.jobs[0].forced);
        assert!(due.lines[0].contains("serial refresh, idle"));
        assert!(plan
            .poll(
                start + QUIET_FOR + QUIET_FOR - Duration::from_secs(1),
                |_| false
            )
            .jobs
            .is_empty());
    }

    #[test]
    fn busy_port_waits_three_hours_then_resets() {
        let start = Instant::now();
        let mut plan = Plan::from_config(&TncConfig {
            protocol: "kiss".into(),
            ..TncConfig::default()
        });
        open(&mut plan, 0, start);
        let mut now = start + QUIET_FOR;
        for n in 1..=3 {
            let due = plan.poll(now, |_| true);
            assert!(due.jobs.is_empty(), "wait {n} should not reset yet");
            assert!(
                due.lines[0].contains(&format!("{n} of {BUSY_WAITS}")),
                "{}",
                due.lines[0]
            );
            now += BUSY_WAIT;
        }
        let due = plan.poll(now, |_| true);
        assert_eq!(due.jobs.len(), 1);
        assert!(due.jobs[0].forced);
        assert!(due.lines[0].contains("after 3 waits"));
    }

    #[test]
    fn an_idle_hour_resets_without_using_the_remaining_waits() {
        let start = Instant::now();
        let mut plan = Plan::from_config(&TncConfig {
            protocol: "kiss".into(),
            ..TncConfig::default()
        });
        open(&mut plan, 0, start);
        let first = start + QUIET_FOR;
        assert!(plan.poll(first, |_| true).jobs.is_empty());
        let due = plan.poll(first + BUSY_WAIT, |_| false);
        assert_eq!(due.jobs.len(), 1);
        assert!(!due.jobs[0].forced);
    }

    #[test]
    fn a_dropped_port_does_not_reset_until_it_opens_again() {
        let start = Instant::now();
        let mut plan = Plan::from_config(&TncConfig {
            protocol: "kiss".into(),
            ..TncConfig::default()
        });
        open(&mut plan, 0, start);
        plan.note(0, "COM6", LinkPhase::Retry, start + QUIET_FOR);
        assert!(plan
            .poll(start + QUIET_FOR + QUIET_FOR, |_| false)
            .jobs
            .is_empty());
        let again = start + QUIET_FOR + QUIET_FOR;
        open(&mut plan, 0, again);
        assert!(plan
            .poll(again + QUIET_FOR - Duration::from_secs(1), |_| false)
            .jobs
            .is_empty());
        assert_eq!(plan.poll(again + QUIET_FOR, |_| false).jobs.len(), 1);
    }

    #[test]
    fn tcp_and_hostmode_are_not_scheduled() {
        assert!(Plan::from_config(&TncConfig {
            protocol: "kiss".into(),
            kiss_transport: "tcp".into(),
            ..TncConfig::default()
        })
        .links
        .is_empty());
        assert!(Plan::from_config(&TncConfig::default()).links.is_empty());
        assert!(Plan::from_config(&TncConfig {
            protocol: "baycom".into(),
            ..TncConfig::default()
        })
        .links
        .is_empty());
    }

    #[test]
    fn each_serial_tfpcx_modem_keeps_its_own_clock() {
        let mut config = TncConfig {
            protocol: "tfpcx".into(),
            ..TncConfig::default()
        };
        config.links = vec![
            kiss_link(&[0], false),
            kiss_link(&[1], true),
            kiss_link(&[2], false),
        ];
        let start = Instant::now();
        let mut plan = Plan::from_config(&config);
        assert_eq!(
            plan.links
                .iter()
                .map(|link| (link.index, link.ports.clone()))
                .collect::<Vec<_>>(),
            vec![(0, BTreeSet::from([0])), (2, BTreeSet::from([2]))]
        );
        open(&mut plan, 0, start);
        open(&mut plan, 2, start);
        let due = plan.poll(start + QUIET_FOR, |ports| ports.contains(&2));
        assert_eq!(due.jobs.len(), 1);
        assert_eq!(due.jobs[0].port, 0);
        assert!(!due.jobs[0].forced);
        let mut now = start + QUIET_FOR;
        for n in 1..=3 {
            let due = plan.poll(now, |ports| ports.contains(&2));
            assert!(
                due.jobs.iter().all(|job| job.port != 2),
                "port 2 should still be waiting ({n})"
            );
            now += BUSY_WAIT;
        }
        let due = plan.poll(now, |ports| ports.contains(&2));
        assert_eq!(due.jobs.len(), 1);
        assert!(due.jobs[0].forced);
        assert_eq!(due.jobs[0].port, 2);
    }

    fn kiss_link(ports: &[u8], tcp: bool) -> TncLink {
        TncLink {
            protocol: "kiss".into(),
            ports: ports.to_vec(),
            transport: if tcp { "tcp".into() } else { "serial".into() },
            serial_port: "COM6".into(),
            baud: 9600,
            tcp_host: "127.0.0.1".into(),
            tcp_port: 8001,
        }
    }
}
