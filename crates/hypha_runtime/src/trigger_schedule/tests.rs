use super::*;

const S: i64 = 100;

/// A host clock whose wall and monotonic readings are both `now`.
fn host(now: i64) -> TriggerClock<'static> {
    TriggerClock::Host { wall: now, steady: now }
}

/// A host clock whose wall and monotonic readings differ, as after a wall-clock step.
fn host_at(wall: i64, steady: i64) -> TriggerClock<'static> {
    TriggerClock::Host { wall, steady }
}

const FREE: Limit = Limit { at: None, blocking: None };

fn sim(now: i64) -> TriggerClock<'static> {
    TriggerClock::Sim { now, limit: &FREE }
}

/// The limit of an upstream that reported `clock`.
fn reported(clock: i64) -> Limit {
    Limit { at: Some(clock + 1), blocking: Some(("up".into(), u64::try_from(clock).ok())) }
}

/// A schedule of one port with silence limit `silence`.
fn one(silence: i64) -> TriggerSchedule {
    TriggerSchedule::new(&[silence]).unwrap()
}

fn ports(ports: &[usize]) -> TriggerPorts {
    TriggerPorts::of(ports.iter().copied()).unwrap()
}

fn trigger(time: i64, port: usize) -> TriggerEvent {
    TriggerEvent::Trigger { time, port }
}

fn late(stamp: i64, port: usize) -> TriggerEvent {
    TriggerEvent::Late { stamp, port }
}

fn fallback(time: i64, due: &[usize]) -> TriggerEvent {
    TriggerEvent::Fallback { time, due: ports(due) }
}

/// The events of one admission of the pending stamps of every port.
fn run_ports(
    s: &mut TriggerSchedule,
    pending: &[Vec<i64>],
    clock: TriggerClock<'_>,
) -> Vec<TriggerEvent> {
    let mut out = Vec::new();
    assert_eq!(s.admit(pending, clock, &mut out), Ok(TriggerAdmit::Run));
    out
}

/// The events of one admission of a one-port schedule.
fn run(s: &mut TriggerSchedule, pending: &[i64], clock: TriggerClock<'_>) -> Vec<TriggerEvent> {
    run_ports(s, &[pending.to_vec()], clock)
}

#[test]
fn new_refuses_a_negative_silence_and_takes_zero_as_no_limit() {
    assert!(TriggerSchedule::new(&[-1]).is_none());
    assert!(TriggerSchedule::new(&[S, -1]).is_none());
    assert!(TriggerSchedule::new(&[0]).is_some(), "an untimed trigger");
    assert!(TriggerSchedule::new(&[S, 0]).is_some(), "an untimed trigger beside a timed one");
}

#[test]
fn a_schedule_of_untimed_triggers_has_no_fallback() {
    let mut s = one(0);
    assert_eq!(s.shortest(), None);
    let year = 365 * 24 * 3_600 * 1_000_000_000_i64;
    assert_eq!(run(&mut s, &[], host(0)), vec![]);
    assert_eq!(run(&mut s, &[], host(year)), vec![], "no step without a trigger");
    assert_eq!(s.next_fallback(), None);
    assert_eq!(run(&mut s, &[year + 5], host(year + 5)), vec![trigger(year + 5, 0)]);
    assert_eq!(run(&mut s, &[], host(2 * year)), vec![]);
}

#[test]
fn an_untimed_trigger_is_never_late() {
    let mut s = one(0);
    assert_eq!(run(&mut s, &[10], host(1_000_000)), vec![trigger(10, 0)]);
}

#[test]
fn the_fallback_counts_only_the_timed_triggers_and_never_marks_an_untimed_one_due() {
    let mut s = TriggerSchedule::new(&[0, S]).unwrap();
    assert_eq!(s.shortest(), Some(S));
    assert_eq!(run_ports(&mut s, &[vec![], vec![]], host(0)), vec![]);
    assert_eq!(run_ports(&mut s, &[vec![], vec![]], host(S)), vec![fallback(S, &[1])]);
    let mut sim_schedule = TriggerSchedule::new(&[0, S]).unwrap();
    let events = run_ports(&mut sim_schedule, &[vec![], vec![]], sim(0));
    assert_eq!(events, vec![]);
    let events = run_ports(&mut sim_schedule, &[vec![], vec![]], sim(2 * S));
    assert_eq!(events, vec![fallback(S, &[1]), fallback(2 * S, &[1])]);
}

#[test]
fn new_refuses_no_port_and_more_ports_than_a_set_holds() {
    assert!(TriggerSchedule::new(&[]).is_none());
    assert!(TriggerSchedule::new(&[S; TriggerPorts::CAPACITY]).is_some());
    assert!(TriggerSchedule::new(&[S; TriggerPorts::CAPACITY + 1]).is_none());
}

#[test]
fn admit_refuses_pending_lists_of_other_ports() {
    let mut s = TriggerSchedule::new(&[S, S]).unwrap();
    let mut out = Vec::new();
    assert_eq!(s.admit(&[vec![]], host(0), &mut out), Err(HoldError::Ports));
}

#[test]
fn the_first_call_anchors_the_fallback() {
    let mut s = one(S);
    assert_eq!(run(&mut s, &[], host(1_000)), vec![]);
    assert_eq!(run(&mut s, &[], host(1_099)), vec![]);
    assert_eq!(run(&mut s, &[], host(1_100)), vec![fallback(1_100, &[0])]);
}

#[test]
fn a_trigger_runs_and_moves_the_fallback() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    assert_eq!(run(&mut s, &[30], host(30)), vec![trigger(30, 0)]);
    assert_eq!(run(&mut s, &[], host(129)), vec![]);
    assert_eq!(run(&mut s, &[], host(130)), vec![fallback(130, &[0])]);
}

#[test]
fn missed_host_fallbacks_run_as_one_at_the_latest_due_time() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    assert_eq!(run(&mut s, &[], host(350)), vec![fallback(300, &[0])]);
    assert_eq!(run(&mut s, &[], host(399)), vec![]);
    assert_eq!(run(&mut s, &[], host(400)), vec![fallback(400, &[0])]);
}

#[test]
fn a_host_clock_jump_runs_one_fallback() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    let year = 365 * 24 * 3_600 * 1_000_000_000_i64;
    assert_eq!(run(&mut s, &[], host(year)), vec![fallback(year, &[0])]);
}

#[test]
fn a_host_trigger_older_than_the_limit_is_late() {
    let mut s = one(S);
    assert_eq!(run(&mut s, &[10], host(1_000_000)), vec![late(10, 0)]);
    assert_eq!(run(&mut s, &[], host(1_000_099)), vec![], "no burst of fallbacks");
}

#[test]
fn host_silence_counts_from_the_send_stamp_of_a_trigger() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    // Sent at 50, drained at 90: the next fallback is due at 150, not at 190.
    assert_eq!(run(&mut s, &[50], host(90)), vec![trigger(50, 0)]);
    assert_eq!(run(&mut s, &[], host(149)), vec![]);
    assert_eq!(run(&mut s, &[], host(150)), vec![fallback(150, &[0])]);
}

#[test]
fn a_host_trigger_stamped_before_a_fallback_steps_at_the_fallback_time() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    assert_eq!(run(&mut s, &[], host(150)), vec![fallback(100, &[0])]);
    // Sent at 90, younger than the limit, drained after the fallback at 100: the port has
    // no step after it, so the sample steps, and the step time does not go back.
    assert_eq!(run(&mut s, &[90], host(160)), vec![trigger(100, 0)]);
    assert_eq!(run(&mut s, &[100], host(170)), vec![trigger(100, 0)]);
}

#[test]
fn a_sim_trigger_stamped_before_the_last_event_is_refused() {
    let mut s = one(1_000);
    let mut out = Vec::new();
    assert_eq!(
        s.admit(&[vec![200, 150]], sim(300), &mut out),
        Ok(TriggerAdmit::OutOfOrder { stamp: 150, before: 200 })
    );
    assert!(out.is_empty(), "nothing runs");
    assert_eq!(run(&mut s, &[200], sim(300)), vec![trigger(200, 0)]);
    assert_eq!(
        s.admit(&[vec![199]], sim(300), &mut out),
        Ok(TriggerAdmit::OutOfOrder { stamp: 199, before: 200 })
    );
    assert_eq!(run(&mut s, &[200], sim(300)), vec![trigger(200, 0)], "a tie runs");
}

#[test]
fn a_sim_trigger_before_the_first_evaluation_still_runs() {
    let mut s = one(S);
    assert_eq!(run(&mut s, &[990], sim(1_000)), vec![trigger(990, 0)]);
}

#[test]
fn a_sim_trigger_before_the_first_evaluation_moves_no_anchor_back() {
    let mut s = one(S);
    assert_eq!(run(&mut s, &[990], sim(1_000)), vec![trigger(990, 0)]);
    // The next fallback counts from the first evaluation, the later of the two.
    assert_eq!(run(&mut s, &[], sim(1_099)), vec![]);
    assert_eq!(run(&mut s, &[], sim(1_100)), vec![fallback(1_100, &[0])]);
}

#[test]
fn a_sim_fallback_holds_for_an_upstream_that_never_reported() {
    let never = Limit { at: Some(i64::MIN), blocking: Some(("up".into(), None)) };
    let mut s = one(S);
    let _ = run(&mut s, &[], TriggerClock::Sim { now: 0, limit: &never });
    assert_eq!(run(&mut s, &[], TriggerClock::Sim { now: 500, limit: &never }), vec![]);
}

#[test]
fn host_a_sample_after_a_raised_step_compares_with_the_sample_of_that_step() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    assert_eq!(run(&mut s, &[], host(150)), vec![fallback(100, &[0])]);
    assert_eq!(run(&mut s, &[90], host(160)), vec![trigger(100, 0)]);
    // Sent at 95, after the sample of the port's last step at 90, though before the time
    // that step ran at.
    assert_eq!(run(&mut s, &[95], host(170)), vec![trigger(100, 0)]);
}

#[test]
fn host_a_trigger_at_exactly_its_limit_steps_and_one_past_it_is_late() {
    let mut s = one(S);
    assert_eq!(run(&mut s, &[900], host(1_000)), vec![trigger(900, 0)]);
    let mut s = one(S);
    assert_eq!(run(&mut s, &[899], host(1_000)), vec![late(899, 0)]);
}

#[test]
fn host_a_tie_within_one_port_steps_each_sample() {
    let mut s = one(S);
    assert_eq!(run(&mut s, &[50, 50], host(60)), vec![trigger(50, 0), trigger(50, 0)]);
}

#[test]
fn host_fallbacks_before_a_late_trigger_run_as_one() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    assert_eq!(run(&mut s, &[350], host(400)), vec![fallback(300, &[0]), trigger(350, 0)]);
}

#[test]
fn a_long_sim_jump_is_admitted_over_capped_calls_in_order() {
    let mut s = one(S);
    let _ = run(&mut s, &[], sim(0));
    let now = S * 3000;
    let mut all = Vec::new();
    let mut calls = 0;
    loop {
        let events = run(&mut s, &[], sim(now));
        assert!(events.len() <= MAX_EVENTS_PER_ADMIT);
        calls += 1;
        all.extend(events);
        if !s.capped() {
            break;
        }
    }
    assert_eq!(calls, 3, "1024 + 1024 + 952 events");
    let expected: Vec<TriggerEvent> = (1..=3000).map(|n| fallback(S * n, &[0])).collect();
    assert_eq!(all, expected);
}

#[test]
fn a_sim_jump_runs_each_fallback_it_skips() {
    let mut s = one(S);
    let _ = run(&mut s, &[], sim(0));
    assert_eq!(
        run(&mut s, &[], sim(350)),
        vec![fallback(100, &[0]), fallback(200, &[0]), fallback(300, &[0])]
    );
}

#[test]
fn a_sim_fallback_waits_until_no_upstream_can_send_a_trigger_at_its_time() {
    let mut s = one(S);
    let _ = run(&mut s, &[], sim(100));
    // The upstream reported 199: it may still send a trigger stamped 200.
    let at_fallback = reported(199);
    let held = TriggerClock::Sim { now: 300, limit: &at_fallback };
    assert_eq!(run(&mut s, &[], held), vec![]);
    assert_eq!(s.frontier(&[vec![]], &at_fallback), Some(200));
    // It reported 200 and sent the trigger at 200, which replaces the fallback.
    let past = reported(200);
    let clock = TriggerClock::Sim { now: 300, limit: &past };
    assert_eq!(run(&mut s, &[200], clock), vec![trigger(200, 0)]);
}

#[test]
fn a_trigger_at_the_fallback_time_preempts_it() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    assert_eq!(run(&mut s, &[100], host(100)), vec![trigger(100, 0)]);
}

#[test]
fn a_late_trigger_runs_after_the_fallbacks_before_it() {
    let mut s = one(S);
    let _ = run(&mut s, &[], sim(0));
    assert_eq!(
        run(&mut s, &[250], sim(250)),
        vec![fallback(100, &[0]), fallback(200, &[0]), trigger(250, 0)]
    );
}

#[test]
fn sim_triggers_wait_for_the_clock() {
    let mut s = one(1_000);
    let _ = run(&mut s, &[], sim(0));
    assert_eq!(run(&mut s, &[100], sim(50)), vec![]);
    assert_eq!(s.frontier(&[vec![100]], &FREE), Some(100));
}

#[test]
fn sim_triggers_wait_for_the_upstream_limit() {
    let limit = reported(149);
    let mut s = one(1_000);
    let _ = run(&mut s, &[], TriggerClock::Sim { now: 0, limit: &limit });
    assert_eq!(
        run(&mut s, &[100, 200], TriggerClock::Sim { now: 300, limit: &limit }),
        vec![trigger(100, 0)]
    );
    // 200 is still pending; the limit caps the report below 150.
    assert_eq!(s.frontier(&[vec![200]], &limit), Some(150));
}

#[test]
fn a_held_trigger_waits_with_no_time_limit() {
    let limit = reported(49);
    let mut s = one(1_000);
    let mut out = Vec::new();
    let _ = s.admit(&[vec![]], TriggerClock::Sim { now: 0, limit: &limit }, &mut out);
    for _ in 0..3 {
        assert_eq!(
            s.admit(&[vec![100]], TriggerClock::Sim { now: 200, limit: &limit }, &mut out),
            Ok(TriggerAdmit::Run)
        );
        assert!(out.is_empty(), "the trigger stays held");
    }
}

#[test]
fn a_wall_clock_step_back_does_not_move_a_host_fallback() {
    let mut s = one(S);
    assert_eq!(run(&mut s, &[], host_at(1_000, 0)), vec![]);
    // The wall clock steps back by 960 ns; the fallback stays at its steady time.
    assert_eq!(run(&mut s, &[], host_at(40, 99)), vec![]);
    assert_eq!(run(&mut s, &[], host_at(40, 100)), vec![fallback(100, &[0])]);
}

#[test]
fn a_trigger_received_across_a_wall_step_keeps_its_steady_silence() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    // Stamped at wall 1_000 after the wall clock stepped back to 40: sent at once.
    assert_eq!(run(&mut s, &[1_000], host_at(40, 100)), vec![trigger(100, 0)]);
    // Its silence counts from its steady time, whatever the wall clock does next.
    assert_eq!(run(&mut s, &[], host_at(-5_000, 199)), vec![]);
    assert_eq!(run(&mut s, &[], host_at(-5_000, 200)), vec![fallback(200, &[0])]);
}

#[test]
fn a_trigger_with_an_old_wall_stamp_steps_at_a_steady_fallback_before_it() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host_at(1_000, 0));
    assert_eq!(run(&mut s, &[], host_at(1_000, 100)), vec![fallback(100, &[0])]);
    // Sent at steady 50, before the fallback at 100, and 60 ns old: it steps at 100.
    assert_eq!(run(&mut s, &[950], host_at(1_010, 110)), vec![trigger(100, 0)]);
}

#[test]
fn ports_run_in_stamp_then_port_order() {
    let pending = [vec![10, 30], vec![10, 20]];
    let expected = vec![trigger(10, 0), trigger(10, 1), trigger(20, 1), trigger(30, 0)];
    let mut s = TriggerSchedule::new(&[1_000, 1_000]).unwrap();
    assert_eq!(run_ports(&mut s, &pending, sim(100)), expected);
    let mut s = TriggerSchedule::new(&[1_000, 1_000]).unwrap();
    assert_eq!(run_ports(&mut s, &pending, host(100)), expected);
}

#[test]
fn a_port_whose_own_stamps_descend_is_out_of_order_under_sim() {
    // A sort would step 10 of port 0 before 15 of port 1; the merge keeps port 0's order.
    let mut s = TriggerSchedule::new(&[1_000, 1_000]).unwrap();
    let mut out = Vec::new();
    assert_eq!(
        s.admit(&[vec![20, 10], vec![15]], sim(100), &mut out),
        Ok(TriggerAdmit::OutOfOrder { stamp: 10, before: 20 })
    );
    assert!(out.is_empty(), "nothing runs");
}

#[test]
fn host_a_descending_stamp_is_late_on_its_own_port_only() {
    let mut s = TriggerSchedule::new(&[S, S]).unwrap();
    assert_eq!(run_ports(&mut s, &[vec![20], vec![]], host(25)), vec![trigger(20, 0)]);
    // 15 comes after port 0's step at 20; 16 of port 1 comes after no step of its port.
    assert_eq!(
        run_ports(&mut s, &[vec![15], vec![16]], host(30)),
        vec![late(15, 0), trigger(20, 1)]
    );
}

#[test]
fn host_an_earlier_stamp_of_another_port_drained_later_still_runs() {
    let mut s = TriggerSchedule::new(&[S, S]).unwrap();
    assert_eq!(run_ports(&mut s, &[vec![20], vec![]], host(25)), vec![trigger(20, 0)]);
    // Sent at 18, before port 0's step, and drained after it: it steps at 20.
    assert_eq!(run_ports(&mut s, &[vec![], vec![18]], host(26)), vec![trigger(20, 1)]);
}

#[test]
fn host_a_trigger_older_than_its_own_port_limit_is_late() {
    let mut s = TriggerSchedule::new(&[100, 1_000]).unwrap();
    // Both are 200 ns old: past the limit of port 0, within that of port 1.
    assert_eq!(
        run_ports(&mut s, &[vec![300], vec![300]], host(500)),
        vec![late(300, 0), trigger(300, 1)]
    );
}

#[test]
fn sim_a_trigger_waits_until_every_upstream_reported_its_stamp() {
    let held = reported(99);
    let reached = reported(100);
    let mut s = one(1_000);
    let _ = run(&mut s, &[], sim(0));
    assert_eq!(run(&mut s, &[100], TriggerClock::Sim { now: 200, limit: &held }), vec![]);
    assert_eq!(
        run(&mut s, &[100], TriggerClock::Sim { now: 200, limit: &reached }),
        vec![trigger(100, 0)]
    );
    let mut s = TriggerSchedule::new(&[1_000, 1_000]).unwrap();
    let _ = run_ports(&mut s, &[vec![], vec![]], sim(0));
    let tie = [vec![100], vec![100]];
    assert_eq!(run_ports(&mut s, &tie, TriggerClock::Sim { now: 200, limit: &held }), vec![]);
    assert_eq!(
        run_ports(&mut s, &tie, TriggerClock::Sim { now: 200, limit: &reached }),
        vec![trigger(100, 0), trigger(100, 1)]
    );
}

#[test]
fn the_fallback_counts_the_shortest_silence_from_the_last_event() {
    let mut s = TriggerSchedule::new(&[100, 300]).unwrap();
    let _ = run_ports(&mut s, &[vec![], vec![]], host(0));
    assert_eq!(run_ports(&mut s, &[vec![], vec![50]], host(50)), vec![trigger(50, 1)]);
    assert_eq!(run_ports(&mut s, &[vec![], vec![]], host(149)), vec![]);
    assert_eq!(run_ports(&mut s, &[vec![], vec![]], host(150)), vec![fallback(150, &[0])]);
}

#[test]
fn a_fallback_names_only_the_ports_past_their_own_limit() {
    let mut s = TriggerSchedule::new(&[100, 250]).unwrap();
    let _ = run_ports(&mut s, &[vec![], vec![]], sim(0));
    assert_eq!(
        run_ports(&mut s, &[vec![], vec![]], sim(300)),
        vec![fallback(100, &[0]), fallback(200, &[0]), fallback(300, &[0, 1])]
    );
}

#[test]
fn a_trigger_on_one_port_does_not_move_another_ports_deadline() {
    let mut s = TriggerSchedule::new(&[100, 250]).unwrap();
    let _ = run_ports(&mut s, &[vec![], vec![]], sim(0));
    // Port 1 has been silent since the first evaluation, so it is due at 250, not 340.
    assert_eq!(
        run_ports(&mut s, &[vec![90], vec![]], sim(300)),
        vec![trigger(90, 0), fallback(190, &[0]), fallback(290, &[0, 1])]
    );
}

#[test]
fn the_host_fallback_anchor_never_moves_back() {
    let mut s = one(S);
    let _ = run(&mut s, &[], host(0));
    assert_eq!(run(&mut s, &[], host(150)), vec![fallback(100, &[0])]);
    // Sent at 90 and drained at 160: it steps at 100, and the next fallback counts from 100.
    assert_eq!(run(&mut s, &[90], host(160)), vec![trigger(100, 0)]);
    assert_eq!(run(&mut s, &[], host(199)), vec![]);
    assert_eq!(run(&mut s, &[], host(200)), vec![fallback(200, &[0])]);
}

#[test]
fn frontier_stays_below_a_held_tie() {
    let held = reported(99);
    let mut s = TriggerSchedule::new(&[1_000, 1_000]).unwrap();
    let _ = run_ports(&mut s, &[vec![], vec![]], sim(0));
    let tie = [vec![100], vec![100]];
    assert_eq!(run_ports(&mut s, &tie, TriggerClock::Sim { now: 200, limit: &held }), vec![]);
    // The cycle reports at most 99, so a cycle downstream does not pass the tie either.
    assert_eq!(s.frontier(&tie, &held), Some(100));
}

/// Simulated time: the events do not depend on how the arrivals, the upstream reports
/// and the clock are split into turns, as long as each upstream reports a clock only
/// after it sent every sample stamped at or before it.
mod causal_interleaving {
    use proptest::prelude::*;

    use super::*;

    /// The grid the samples are stamped on; ties are common.
    const GRID: i64 = 10;
    /// The clock and every report once the case is over.
    const END: i64 = 300;

    #[derive(Debug, Clone, Copy)]
    enum Action {
        /// The next sample of a port's upstream arrives.
        Arrive(usize),
        /// A port's upstream reports the highest clock it may: below its next sample.
        Report(usize),
        /// The node's clock moves to a grid point, if that is later.
        Clock(i64),
        /// The node runs a turn.
        Turn,
    }

    /// One node, whose trigger port `p` reads the samples of upstream `p`.
    struct Node<'a> {
        samples: &'a [Vec<i64>],
        schedule: TriggerSchedule,
        arrived: Vec<usize>,
        consumed: Vec<usize>,
        reports: Vec<i64>,
        now: i64,
        events: Vec<TriggerEvent>,
    }

    impl<'a> Node<'a> {
        /// A node with nothing arrived, whose first turn is at clock 0.
        fn new(silences: &[i64], samples: &'a [Vec<i64>]) -> Self {
            let mut node = Self {
                samples,
                schedule: TriggerSchedule::new(silences).unwrap(),
                arrived: vec![0; samples.len()],
                consumed: vec![0; samples.len()],
                reports: vec![0; samples.len()],
                now: 0,
                events: Vec::new(),
            };
            node.turn();
            node
        }

        fn apply(&mut self, action: Action) {
            let ports = self.samples.len();
            match action {
                Action::Arrive(port) => {
                    let port = port % ports;
                    if self.arrived[port] < self.samples[port].len() {
                        self.arrived[port] += 1;
                    }
                }
                Action::Report(port) => {
                    let port = port % ports;
                    let next = self.samples[port].get(self.arrived[port]);
                    let reachable = next.map_or(END, |stamp| stamp - 1);
                    self.reports[port] = self.reports[port].max(reachable);
                }
                Action::Clock(now) => self.now = self.now.max(now),
                Action::Turn => self.turn(),
            }
        }

        /// Everything arrives and is reported, the clock reaches the end, and a turn runs.
        fn finish(mut self) -> Vec<TriggerEvent> {
            self.arrived = self.samples.iter().map(Vec::len).collect();
            self.reports.fill(END);
            self.now = END;
            self.turn();
            self.events
        }

        fn turn(&mut self) {
            loop {
                let pending: Vec<Vec<i64>> = self
                    .samples
                    .iter()
                    .zip(self.consumed.iter().zip(&self.arrived))
                    .map(|(stamps, (&from, &to))| stamps[from..to].to_vec())
                    .collect();
                let lowest = self.reports.iter().copied().min().unwrap();
                let limit = reported(lowest);
                let clock = TriggerClock::Sim { now: self.now, limit: &limit };
                let mut out = Vec::new();
                let admitted = self.schedule.admit(&pending, clock, &mut out);
                assert_eq!(admitted, Ok(TriggerAdmit::Run), "a causal replay is in order");
                for event in &out {
                    if let TriggerEvent::Trigger { port, .. } = *event {
                        self.consumed[port] += 1;
                    }
                }
                self.events.extend(out);
                if !self.schedule.capped() {
                    return;
                }
            }
        }
    }

    fn action() -> impl Strategy<Value = Action> {
        prop_oneof![
            (0..4_usize).prop_map(Action::Arrive),
            (0..4_usize).prop_map(Action::Report),
            (0..=END / GRID).prop_map(|step| Action::Clock(step * GRID)),
            Just(Action::Turn),
        ]
    }

    /// One to four ports, each with its silence limit and its ascending grid samples.
    fn ports() -> impl Strategy<Value = (Vec<i64>, Vec<Vec<i64>>)> {
        let port = (
            prop::sample::select(vec![20_i64, 30, 50]),
            prop::collection::vec(1..=20_i64, 0..6).prop_map(|steps| {
                let mut stamps: Vec<i64> = steps.into_iter().map(|step| step * GRID).collect();
                stamps.sort_unstable();
                stamps
            }),
        );
        prop::collection::vec(port, 1..=4).prop_map(|ports| ports.into_iter().unzip())
    }

    proptest! {
        #[test]
        fn any_causal_interleaving_of_arrivals_and_reports_gives_the_same_events(
            (silences, samples) in ports(),
            actions in prop::collection::vec(action(), 0..48),
        ) {
            let reference = Node::new(&silences, &samples).finish();
            let mut node = Node::new(&silences, &samples);
            for action in actions {
                node.apply(action);
            }
            prop_assert_eq!(node.finish(), reference);
        }
    }
}
