//! Behavioural tests for the example projects.
//!
//! These assemble the *actual* example sources from `examples/`, link them the
//! way the CLI does, and run them in the simulator. They are the reason the
//! examples in the documentation can be trusted: if someone edits an example
//! and breaks the LED order or the delay, this fails.

use mcs51_asm::{Options, assemble};
use mcs51_ld::{LinkOptions, link};
use mcs51_sim::Machine;

/// Assemble and link one source, returning the code image.
fn firmware(name: &str, source: &str) -> mcs51_ihex::Image {
    let output = assemble(source, &Options::new(name));
    assert!(
        output.is_ok(),
        "{name} failed to assemble:\n{}",
        output.render_diagnostics()
    );
    let result = link(&[output.object], &LinkOptions::default());
    assert!(
        result.is_ok(),
        "{name} failed to link:\n{}",
        result.render_diagnostics()
    );
    result.code
}

/// Run a machine and record every change of a port latch, with cycle stamps.
fn trace_port(mut machine: Machine, port: usize, cycles: u64) -> Vec<(u64, u8)> {
    let mut events = Vec::new();
    let mut last = machine.peripherals().latch(port);
    while machine.cycles < cycles {
        machine.step();
        let value = machine.peripherals().latch(port);
        if value != last {
            events.push((machine.cycles, value));
            last = value;
        }
    }
    events
}

/// Which of the three LED pins is pulled low, if any.
fn lit(event: u8) -> Option<u8> {
    (5..=7).find(|bit| event & (1 << bit) == 0)
}

const LED_FLOW: &str = include_str!("../../../examples/led-flow/src/main.asm");

#[test]
fn the_bare_metal_led_flow_chases_in_order() {
    let image = firmware("led-flow.asm", LED_FLOW);
    // A bare-metal chaser should be tiny.
    assert!(
        image.size_report().bytes < 200,
        "unexpected size: {}",
        image.size_report()
    );

    let mut machine = Machine::from_image(&image);
    // The chaser never halts, so do not treat the final SJMP as a stop.
    machine.halt_on_self_jump = false;
    let events = trace_port(machine, 3, 1_600_000);

    // Collect the order in which LEDs light up.
    let order: Vec<u8> = events.iter().filter_map(|(_, value)| lit(*value)).collect();
    assert!(
        order.len() >= 6,
        "expected at least two full rounds, saw {order:?} in {events:?}"
    );
    assert_eq!(
        &order[..6],
        &[5, 6, 7, 5, 6, 7],
        "the chaser must go P3.5 -> P3.6 -> P3.7 and repeat, saw {order:?}"
    );

    // Exactly one LED may be lit at a time.
    for (cycle, value) in &events {
        let low = (5..=7).filter(|bit| value & (1 << bit) == 0).count();
        assert!(
            low <= 1,
            "at cycle {cycle} {low} LEDs were lit ({value:#04X})"
        );
    }
}

#[test]
fn the_bare_metal_led_flow_dwell_time_is_about_a_quarter_second() {
    let image = firmware("led-flow.asm", LED_FLOW);
    let mut machine = Machine::from_image(&image);
    machine.halt_on_self_jump = false;
    let events = trace_port(machine, 3, 1_600_000);

    // Measure the gap between consecutive "an LED turned on" events.
    let mut on_cycles: Vec<u64> = events
        .iter()
        .filter(|(_, value)| lit(*value).is_some())
        .map(|(cycle, _)| *cycle)
        .collect();
    on_cycles.dedup();
    assert!(on_cycles.len() >= 4, "not enough samples: {on_cycles:?}");

    for window in on_cycles.windows(2) {
        let gap = window[1] - window[0];
        // 250 ms at 12 MHz is 250 000 machine cycles; allow a wide band because
        // the doc only claims "about a quarter of a second".
        assert!(
            (180_000..=320_000).contains(&gap),
            "an LED stayed lit for {gap} cycles, expected about 250 000"
        );
    }
}

#[test]
fn the_bare_metal_example_leaves_the_serial_pins_alone() {
    // P3.0/P3.1 are RXD/TXD; a chaser that clobbered them would break the next
    // download. The example uses bit operations precisely to avoid that.
    let image = firmware("led-flow.asm", LED_FLOW);
    let mut machine = Machine::from_image(&image);
    machine.halt_on_self_jump = false;
    let events = trace_port(machine, 3, 1_200_000);
    for (cycle, value) in &events {
        assert_eq!(
            value & 0x03,
            0x03,
            "P3.0/P3.1 must stay high (they are the serial pins), at cycle {cycle} P3={value:#04X}"
        );
    }
}
