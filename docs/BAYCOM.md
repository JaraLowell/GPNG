# BayCom ser12 with FT232R USB (experimental)

This backend targets classic **1200 bit/s BayCom ser12** modems using a
**full-handshake FT232R/FT232RL USB-to-RS232 adapter**. It is not a TNC hostmode
driver, a generic COM-byte protocol, or a Linux BayCom kernel-driver frontend.
The host runs the same software AX.25 sessions as KISS, without KISS wrapping.

**Status:** Windows x64/MSVC compilation and simulated tests only. No physical
adapter/modem, oscilloscope, Windows 10, macOS, Linux, or on-air interoperability
test has been performed. macOS and Linux use the same D2XX backend but still
need platform and field validation. Treat all cables, including commercially
advertised FT232RL cables, as unverified until measured.

## Required hardware and safety

- FT232R-family chip with a **unique USB serial number**, normally VID `0403`,
  PID `6001`. The D2XX API must identify it as an FT232R.
- Real RS232 voltage-level conversion and connected **TXD, DTR, RTS, CTS and
  signal ground**. A three-wire serial cable or TTL-only FT232RL breakout is
  not a substitute. Chip branding alone does not establish suitable wiring.
- A compatible ser12 modem and correctly wired radio audio/PTT connection.
  Some modems take their power from serial signals. Check the modem schematic,
  adapter output-current capability, and voltages under load.
- PL2303, other FTDI families, 9600-bit/s par96/PICPAR, and parallel-port modems
  are not supported. Changing a COM baud rate cannot add that support.

**Disable the transmitter while plugging in, opening, closing, and initially
testing the adapter.** USB initialization and mode changes are not proven
glitch-free. Software attempts to release PTT on errors and shutdown, but
cannot guarantee unkeying after a crash, USB failure, suspend, or loss of
power. Use the radio's transmit timeout and an accessible manual way to unkey.
Never leave an experimental installation transmitting unattended.

### Intended signal mapping

| FT232R GPIO | RS232 signal | DTE DB9 pin | ser12 role |
|---|---|---|---|
| 0 | TXD | 3 | Held at the positive RS232 level for modem power |
| 2 | RTS | 7 | PTT: asserted while transmitting |
| 3 | CTS | 8 | Received NRZI data |
| 4 | DTR | 4 | Transmitted NRZI data; asserted in receive |
| - | Ground | 5 | Signal ground |

These are standard **DTE pin numbers**, not a prescription to connect them
blindly to an unknown modem. Confirm connector gender, any crossover/null-modem
wiring, level-shifter inversion, and actual modem schematic.
FT232R GPIO levels precede the RS232 inverter: receive idle is TXD low,
RTS high (PTT off), DTR low. Both receive-data polarities are handled through
NRZI transitions. The PTT readback verifies the FT232R GPIO, **not** voltage
at the cable or the radio's RF state.

## Driver setup

Use the official [FTDI D2XX packages and platform instructions](https://ftdichip.com/drivers/d2xx-drivers/).
Do not install random driver packs, change EEPROM, replace drivers with Zadig,
or disable OS security protections for this experiment.

GPNG links the vendor D2XX library through
[`libftd2xx`](https://docs.rs/libftd2xx/0.33.1/libftd2xx/) with its `static`
feature. A separate application-local D2XX DLL/shared library should not be
needed for supported build targets, but USB access and OS driver ownership
still need correct setup. The vendor library has its own distribution terms;
consult the dependency's bundled FTDI license when packaging releases.

### Windows 10/11

1. Install the official FTDI driver for the adapter and Windows architecture.
   FTDI's combined driver package normally supplies VCP and direct D2XX access.
2. Verify the adapter and serial identity in Device Manager. Close terminal
   programs and anything else using its COM port.
3. Select that COM port, or the matching `FTDI:<serial-number>` entry, in GPNG.
   The backend maps COM selection to the exact USB identity; it does not open
   an ordinary serial data stream or pick the first FTDI device.
4. On access failure, check driver installation, competing programs, and
   whether the cable really exposes an FT232R and all required signals.

The currently validated build is x64 MSVC. Other Windows architectures and
GNU toolchains have not been validated; vendor static-library availability
constrains build targets.

### macOS

1. Follow FTDI's current macOS D2XX installation and compatibility guidance for
   your macOS release and Intel/Apple Silicon architecture.
2. Apple's or FTDI's VCP driver may claim the interface, preventing direct USB
   access. Follow FTDI's supported helper/driver-ownership procedure if needed.
   GPNG does not unload system drivers or disable SIP.
3. After direct access is available, choose the exact `FTDI:<serial-number>`
   entry. A `/dev/cu.*` entry may disappear when VCP releases the interface;
   this is why the direct-USB selector exists.

Bundled-library architecture support and physical operation on your macOS
version still require verification. An access error is not proof of bad modem
wiring.

### Linux

1. Give the logged-in user access to **this adapter's USB device**. A
   distribution-appropriate udev rule can match its VID, PID, and serial.
   For a systemd desktop session, an example administrator-installed rule is:

   ```udev
   SUBSYSTEM=="usb", ATTR{idVendor}=="0403", ATTR{idProduct}=="6001", ATTR{serial}=="YOUR_SERIAL", TAG+="uaccess"
   ```

   Substitute the actual identity. Headless/SSH sessions may instead need a
   dedicated group and `MODE="0660"` under local policy. Do not make all USB
   devices world-writable or run the entire application as root.
2. If `ftdi_sio` owns the device, D2XX may require detaching it. Identify and
   unbind **only the chosen adapter's USB interface** using your distribution's
   documented procedure, then choose its `FTDI:<serial-number>` entry.
   GPNG does not unload modules or unbind hardware automatically.
3. Rebind the interface or reconnect the adapter when ordinary VCP use is
   needed again. Remember that group membership for `/dev/ttyUSB*` alone does
   not necessarily grant direct USB access.

Neither the Linux BayCom kernel module nor a Linux-only modem service is
required.

## GPNG setup and behavior

1. Choose **Baycom**, your call sign, the exact adapter, and **1200** in Radio
   setup. Read the experimental warning before pressing **Use this**.
2. Ready is reported only after device validation, bit-bang configuration,
   PTT-off GPIO confirmation, and receipt of GPIO samples.
3. One modem is **radio port 0**. Set `kiss_port = 0` and all `channel_ports`
   to zero (an empty list uses the default). Multiple logical AX.25 connections
   share that radio, its FIFO, carrier sensing, and half-duplex transmitter;
   they are not additional physical ports.
4. Incoming valid-FCS AX.25 frames and completed outgoing bursts reach the
   normal RX/TX monitor. Monitor filters still apply. Corrupt HDLC is reported
   as aggregated warnings, not presented as valid packets. Abort indications
   can also be idle fill after a burst; they do not alone prove packet damage.
5. Session retry timers begin after the final hardware-clocked TX sample,
   not when a frame is enqueued. Local TX completion is **not** an RF ACK.
   Link counters and addressed replies come from the shared session service.

The backend uses only `[params]` from the configured `KISS.INI`; it does **not**
execute any KISS terminal initialization or close scripts:

| Parameter | Units / supported value | Default if absent |
|---|---|---|
| TXDELAY (`T`, command 1) | 10 ms | 300 ms |
| PERSIST (`P`, command 2) | 0..255 | 63 |
| SLOTTIME (`W`, command 3) | 10 ms; must be nonzero | 100 ms |
| TXTAIL (`Y`, command 4, if defined) | 10 ms | 30 ms |
| FULLDUP (`D`, command 5, if defined) | 0 only | Half duplex |

The supplied file's explicit values override these defaults. GUI Esc parameter
edits use the same definitions and persist there. RX turnaround is 100 ms.
Preamble/tail durations round up to whole HDLC flags, with at least one each.
Stop/reopen the radio to reinitialize. Hostmode commands are not supported.
An existing parameter file must be readable and contain `[params]`; a missing
file uses the defaults above.

### Timing limits

FT232R asynchronous bit-bang provides a 9600-sample/s hardware clock
(600 configured baud times 16), eight GPIO samples per 1200-bit/s radio bit.
RX recovers bit centers from CTS transitions; software carrier sensing holds
busy for 100 ms after sustained appropriately spaced edges. TX is buffered,
including key-up/tail flags and a final PTT-off sample, whose GPIO readback
gates completion. No per-bit host timer or FT232R-incompatible MPSSE is used.

USB/D2XX **does not supply sample timestamps or reliable overflow/underrun
markers** in this mode. The driver stops for a 200 ms servicing stall, a
250 ms receive backlog, 500 ms without samples, or I/O/completion errors.
These checks do not prove continuous sampling: shorter FIFO gaps or USB TX
underruns can still escape detection and damage packets. FCS rejects corrupted
received frames, but is not a guarantee of detecting every hardware fault.
CPU load, USB hubs, system sleep, driver versions, power and wiring all need
bench testing. Reopen after an error; ambiguous writes are not replayed.

## First field-test checklist

1. With RF transmission disabled, measure adapter voltages, modem power, CTS
   activity, and **RTS/PTT polarity**. Verify opening/closing and unplugging do
   not leave the radio keyed.
2. Use a scope/logic analyzer at appropriate voltage levels to verify the
   1200-bit/s DTR waveform, flags, and PTT lead/tail timing under USB/CPU load.
   A basic TXD-to-RXD serial loopback does not exercise this driver.
3. Start receive-only with a known-good 1200 AFSK peer. Check monitor addresses
   and payloads, FCS warnings, sample stalls, and signal polarity.
4. Only after bench checks, permit a short low-power/dummy-load TX. Check the
   peer decodes it and that PTT releases after success, cancel and errors.
5. Test connect/UA, bidirectional I/RR, retransmissions, disconnect, binary
   data, and two simultaneous logical connections on port 0. Check each local
   callsign/SSID and remote gets its own sequence numbers and counters.
6. Record OS/architecture, driver version, cable/chip identity, schematic,
   measured timings, and sanitized monitor/log output when reporting results.
   Do not include passwords or private traffic.

## Implementation references

- [Linux ser12 reference](https://github.com/torvalds/linux/blob/v6.6/drivers/net/hamradio/baycom_ser_hdx.c):
  modem signal roles and half-duplex behavior, not a runtime dependency.
- [PyFtdi source](https://github.com/eblot/pyftdi/blob/master/pyftdi/ftdi.py):
  FT232R bit-bang clock factor and FIFO constraints.
- [PyFtdi GPIO limitations](https://eblot.github.io/pyftdi/api/gpio.html):
  hardware-clocked sampling and buffering limitations.
- [D2XX Rust API](https://docs.rs/libftd2xx/0.33.1/libftd2xx/trait.FtdiCommon.html):
  native bit-bang, USB access and GPIO readback.
