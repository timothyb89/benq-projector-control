# benq-control

An unofficial library and daemon for remotely controlling benq-branded
projectors via their serial protocol.

It provides:
 * a simple tool for issuing commands to the projector;
 * a web server to issue commands via a REST API; and
 * a Rust library for controlling projectors from your own applications

## Requirements

 * A compatible projector with an rs232 serial port
 * An rs232 adapter

Depending on your projector and adapter, you may also need a null model cable
and/or a crossover cable. The projector's user manual will tell you if
a crossover cable is needed (and if serial commands are supported in the first
place).

This was developed for use with a BenQ TH685, but BenQ's serial protocol seems
to be the same across most of their devices. The `projector-tool`'s built-in
utilities were designed with this projector in mind and so all options may not
be compatible, however it can still execute arbitrary commands with
`projector-tool exec ...`. Refer to the user manual for a full list.


## Building

To build all binaries, install the Rust toolchain (see https://rustup.rs) and
run:

```bash
cargo build --bins --all-features --release
```

[`cross`] is recommended for cross-compiling. To build for all Raspberry Pi
models, run:
```bash
cross build --target-dir $(pwd)/target-cross --target=arm-unknown-linux-gnueabi --all-features --bins --release
```

(note: `--target-dir` is recommended to prevent spurious rebuilds when using
both `cargo build` and `cross build`)

[`cross`]: https://github.com/rust-embedded/cross

## FAQs

### I sent a command shortly after powering on the projector and commands stopped working. What gives?

Sending commands while the projector is changing power states occasionally
crashes the projector's serial interface. You'll need to physically unplug the
projector to fix this.

The library attempts to work around this by pausing command processing when
power states are changed, but if the projector is powered on or off externally
(e.g. via the power button) crashes can still occur.

Library users can submit `Command::Sleep()` commands to delay the processing
thread if they notice the projector's power state has changed. Usually a single
power query command (`pow=?`) is safe so long as you don't send further
commands, however it may error ("Block item").
