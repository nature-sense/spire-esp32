# spire-idf

<!--
  What this library provides, and **how it is intended to be used**.

  This file is the contract between whoever wrote this library and the model that will build against
  it — a component added here, or an application built on it. It is where an *architecture* lives, and
  the tool knows nothing about actors, tasks, mailboxes or protocols. Keep it true as components
  arrive; an invented rule is worse than a missing one.
-->

## Provides

**The framework** — three components, already here, and not mandatory:

- **`ramen`** — the **dataflow** framework (upstream `Zubax/ramen`, MIT, C++20): typed ports
  (`ramen::Pusher` / `Pushable` / `Puller` / `Pullable`), `>>` to wire them, and topics. Pushing is
  **synchronous and inline**: a producer's `Task` runs every wired behaviour before its `Pusher`
  returns. There is no scheduler, no mailbox and no runtime — one task runs a whole chain.
- **`actors`** — the **classical actor** framework: a **scheduler** that owns the actors and stops them
  in reverse, a **registry** for reaching a peer you cannot be handed a ref to, typed **refs** to send
  to, and `on_message`. An actor is a mailbox (a FreeRTOS queue), a task of its own, and a message
  type; a sender holds an `ActorRef<Message>`, never the actor, so a slow actor cannot stall a fast one
  and the order between two actors is a queue's order:

  ```cpp
  struct Add { int amount = 0; };

  class Counter final : public spire::Actor<Add> {
  public:
      explicit Counter(std::atomic<int>& total) : total_(total) {}
  protected:
      void on_message(const Add& message) override { total_ += message.amount; }
  private:
      std::atomic<int>& total_;
  };

  spire::Scheduler scheduler;                                    // owns them, stops them in reverse
  auto counter = scheduler.spawn<Add, Counter>("counter", 4096, 5, tskNO_AFFINITY, total);
  scheduler.start();                                             // nothing runs before this
  counter.send(Add{3});
  scheduler.stop();
  ```

  Spawn everything and wire it **before** `start()` — a message posted early waits in its mailbox,
  which is what makes wiring order irrelevant. An actor that refuses in `init()` starts nothing, and
  a full mailbox refuses the sender rather than blocking it.

  **Spawn a receiver before its sender, and hand the ref over.** For `a -> b` the receiver `b` is
  spawned first, then its ref is passed to `a` at `a`'s `spawn` — so a peer is a **constructor
  argument** (`spire::ActorRef<Peer::Message>`), which is also the order they must start in. An actor
  is constructed with the peers it reaches, and never fetches one by name:

  ```cpp
  auto b = scheduler.spawn<BMsg, B>("b", 4096, 5, tskNO_AFFINITY);
  auto a = scheduler.spawn<AMsg, A>("a", 4096, 5, tskNO_AFFINITY, b);   // b's ref, handed over
  ```

  Only a peer that **cannot** be handed a ref — because it must reach you in turn, a **cycle** — is
  reached **by name**: the actor takes a `const spire::Registry&` **in its constructor**, keeps it as
  a member, and resolves the peer in `init()`, where every actor exists. `main/` hands it over at
  `spawn` by dereferencing the scheduler's registry:

  ```cpp
  class Forwarder final : public spire::Actor<Add> {
  public:
      explicit Forwarder(const spire::Registry& registry) : registry_(registry) {}   // injected
      bool init() override {
          counter_ = registry_.get<Add>("counter");   // `.`, not `->` — registry_ is a reference
          return counter_.valid();                     // false refuses to start
      }
  protected:
      void on_message(const Add& message) override { counter_.send(message); }
  private:
      const spire::Registry& registry_;
      spire::ActorRef<Add> counter_;
  };

  // main/ hands over the scheduler's registry — a shared_ptr, so dereference it at spawn:
  auto forwarder = scheduler.spawn<Add, Forwarder>("forwarder", 4096, 5, tskNO_AFFINITY,
                                                   *scheduler.registry());
  ```

  A `Registry` is the scheduler's and only the scheduler's. There is no `spire::Registry::instance()`
  and no static `spire::Registry::get`, and `spire::Registry()` does not even compile — so an actor
  never makes one and never keeps one as a global; it is *handed* the scheduler's by reference. Resolve
  by name **only** in `init()`, never in the constructor: the registry is complete only after every
  `spawn`.
- **`toolkit`** — `spire::Task`: one loop, on a task of its own, with a stated stack, priority and
  core. It is the **only** thing the two frameworks share: neither requires the other, and a component
  requires neither.

**Components** — the payload. Add lines here as they arrive:

- **`sht20`**, **`sps30`** — I²C **sensor** drivers (temperature and humidity; particulate matter). One
  device, one bus, and a host test each that runs the protocol against a fake bus.
- **`bm8563`** — an I²C **real-time clock** driver (a battery-backed wall clock: the PCF8563-compatible
  part). It reads the calendar and — unlike the sensors — also **writes** it, so its seam carries the
  write form and its fake can refuse one. `now()` reads **false** when the part's voltage-low flag is
  set, which is the part saying its count is wrong: a time that cannot be trusted is not a time, and it
  is the caller's cue to set the clock from a source it does trust.
- **`axp2101`** — an I²C **power-management IC** driver (a battery board's PMIC): the part's identity,
  whether a battery is connected, whether it is charging, the battery voltage in millivolts, and the
  part's own gauge. It **reads and does not configure** — the power tree a board brings up is that
  board's, and a driver that rearranged it would be one no board could adopt.
- **`bmi270`** — an I²C **inertial measurement unit** driver (a six-axis accelerometer and gyroscope,
  and the part's own die temperature): presence by its chip ID, and one `read` of all six axes in the
  units the datasheet's conversions give. It **reads and does not configure**, like the PMIC beside it —
  and it goes one step past assuming, because the full scale is a *setting*: `read` asks the part for its
  ranges rather than trusting the reset default, so a board that chose ±16 g reads in g instead of in a
  number wrong by eight. Bringing the part up — its power mode, its ranges, and Bosch's configuration
  file — is the board's, so a part that was never brought up answers `probe` and reads *still*: a
  reading that is real and unmoving, which is the caller's to tell from a device that is still.
- **`rolling_average`** — a scalar-averaging **algorithm**. No device, no bus, no board.
- **`st7789`** — an SPI **display controller** driver (a colour LCD panel): the power-on sequence, the
  addressing window, and `flush()`. The panel's facts — size, colour order, init commands — live here;
  the board's — the reset line, the data/command line, the clock — do not.
- **`ili9342`** — the SPI **display controller** the CoreS3 carries, and the same shape as `st7789`:
  the panel's facts — size, the frame it is left in, the init tables, `flush()` — live here, the
  board's do not. Two things are its own. It is told which **revision** it drives — the same glass
  ships with an ILI9342C or an ILI9342E, and the two want different power-on tables, so guessing is not
  an option — and its panel needs **timing** as well as bytes: an interval after sleep-out and another
  after display-on. That is why its seam is three calls where `st7789`'s is two: `bus_delay_ms` is the
  panel's fact too, and the fake records it so a case can prove the wait happened.
- **`ft6336`** — an I²C **touch controller** driver (a capacitive panel): presence, and the point being
  touched. Read with `bus_write_read`, like any register.
- **`lvgl_display`** — the LVGL **surface** for a panel driver: one `lv_display`, the draw buffer LVGL
  renders into, the `flush_cb` that hands each changed rectangle to a **pixel sink**, and the tick. It
  owns no task — `render()` is one `lv_timer_handler()` pass — and names no device: `sink_of(panel)`
  builds the sink from any driver whose `flush()` has the panel's signature (`st7789`'s), so *which*
  panel LVGL draws on stays the application's decision, where the board facts are. It is the one
  component here with **no host test**: LVGL is vendored for the chip and compiled by this project's
  build, so `idf.py build` is the test — it compiles the bridge, and the **link** is proven where the
  bridge is used, an application that makes a `Display`.

## Choosing a framework

- **Human timescale, interactive, command/event-driven → `actors`.** A touch screen, a menu, a reading
  that updates once a second, a device reacting to commands and timers while holding state of its own.
  A unit that has to *wait* for its inputs is a mailbox, a task and `on_message`: post a message and get
  on with something else, and a slow unit is slow without stalling a fast one.
- **Machine timescale, high-rate data flowing through stages → `ramen`.** Frames, kHz samples, an
  inference pipeline: a value is *moved and transformed* rather than *waited on*. Inline dispatch is
  simpler and faster than a queue, and one task runs the whole chain — so give the producer's task a
  stack that holds its consumers.

The distinction is not "streaming versus agents" — a stream is a **component's** job either way (a
sensor bus and a status channel are both drivers). It is whether the application **reacts** to the data
or **pumps** it through stages.

Neither is required. A library of plain components that uses neither is a valid library — and a
component must never assume which framework it is used under.

The choice is made **per application**, before the decomposition: the decomposition *is* the framework,
so the two are decided together rather than one being fitted to the other. An application records the
choice in its `CMakeLists.txt` — `set(SPIRE_APPLICATION_FRAMEWORK actors)`, or `ramen` — so no later
reader has to deduce it from the shape of `main/`.

## How to use it

**A component holds a device or an algorithm, and nothing else.** It takes a bus handle, or its
inputs, and it knows no pins, no addresses, no `Task` and no framework — that is what makes it
reusable and what makes it testable on a host.

**An application holds the composition**: the board facts (pins, bus, addresses), the wiring, and the
**actors**. **An actor is a component of its own** — `components/<actor>/`: a message type, the state
behind it, a mailbox and a task — and the **message types** are a component beside it, because a sender
holds `ActorRef<Receiver::Message>`, so the type on an edge belongs to the receiver and two actors can
share one message. `main/` is then what is left: the spawns, the wiring and the board facts. A **ramen
stage** is not a component of its own — the chain is one task and its stages are inline — so the stages
live with the wiring. A component, either way, carries no mailbox and no task: an actor wraps it.

**A display is wiring, and the panel is the application's.** LVGL draws into memory, not at a panel:
`lvgl_display::Display` owns the `lv_display`, the draw buffer and the flush callback, and reaches the
panel through a **sink** — `lvgl_display::sink_of(panel)`, built in the application from the driver it
constructed. That is the whole of the composition, and it is why `lvgl_display` never names `st7789`:

```cpp
st7789::St7789 panel{bus_device};      // the application's SPI device: its pins, its DC line, its clock
panel.init();                          // the panel first — the first flush goes into it
lvgl_display::Display display{lvgl_display::sink_of(panel), st7789::St7789::kWidth,
                              st7789::St7789::kHeight};
display.init();                        // lv_init, the tick, the buffer, the flush_cb
```

Nothing else is LVGL's to hold: `render()` is pumped by whichever loop the application gives a screen
(an actor of its own, usually), `handle()` is there for what that loop needs from the `lv_display` — the
active screen, a theme, or the `lv_indev` built over `ft6336` — and a touch panel is that same shape one
level down.

**This library's build compiles the bridge; an application's build links it.** Nothing in `main/` here
makes a `Display`, so `idf.py build` type-checks and compiles the component but never pulls it — or LVGL
behind it — out of its archive. An application that makes one is what proves the link, and the proof is
two symbols or none:

```sh
# in an application that makes a Display; both arrive together, or the link failed
xtensa-esp32s3-elf-nm -C build/*.elf | grep -E ' T (lv_init|lvgl_display::Display)'
```

**The lifecycle is the same in both frameworks**, so an application reads the same either way:

```
constructed with its configuration   // the board facts go here, and only here
bool init();                         // takes no arguments; false refuses
void shutdown();                     // releases what it holds
```

Start in the order actors were added (a consumer exists before its producer pushes), stop in reverse.

**Wire before starting.** RAMEN links at the point of `>>`, so a `Pusher` connected after a producer's
task begins silently drops everything pushed before the link was made — which reads as a sensor that
never reports.

### Build requirements a component cannot state itself

- **C++20**, because `ramen` does not compile without it. Nothing needs adding for a chip build —
  ESP-IDF compiles C++ at `-std=gnu++2b` for chip targets — but anything built **outside** IDF (a host
  test, a host tool) has to set it itself.
- **LVGL**, for a display. It is **third-party and vendored**, pinned to one version rather than fetched
  from a registry at configure time — a build that reaches the network for a dependency is a build that
  fails on a bench with no wifi. LVGL cannot set its own configuration: it needs an `lv_conf.h` on the
  include path, which the vendoring wrapper (`components/lvgl/`) carries. **`lvgl_display` is what turns
  that dependency into a screen**: it owns the `lv_display`, the buffer and the `flush_cb`, and takes the
  panel as a sink. So a product that wants one requires `lvgl_display` — whose own `REQUIRES lvgl` brings
  LVGL's headers with it — and the panel driver, which the product names itself.
- Anything else this library needs that no component can state: a config symbol, a compiler flag, a
  partition-table entry. Say it here, because nothing else in the tree carries it.

## Not provided

- **A scheduler, a message bus, an event loop.** A `Task` and a `Pusher`, or a `Task` and a mailbox,
  are the whole of the concurrency story, deliberately.
- **A container or a composition helper.** The application owns the composition: the actors, the order
  they start in, and what is wired to what — and every *name* those actors are spawned under. The
  `actors` framework's registry is only the index the scheduler keeps of what it spawned, so a peer
  that cannot be handed a ref can still be found; it is not a place to keep or look up components.
- **Devices and boards.** A **device driver** is a component: one chip, board-neutral, testable on a
  host. A **board** is a composition — pins, bus, addresses, and the wiring between chips — and a
  composition belongs to the application that names the board. So this library ships drivers and seams
  and stays off the shelf; a board support package is never one of its components, and the application
  that owns the board is the one that writes its BSP.
