# spire-idf — the API

<!--
  What each component EXPOSES, read off its header: every public type and function, its signature, what
  its parameters mean, and what its return value means.

  This is the other half of `SPIRE.md`. That file is the architecture — which framework to reach for,
  what order to start things in, and what belongs in a component rather than an application. This file
  is the surface: the reference a reader (a person, or a model writing against this library) needs so
  that it does not have to open sixteen headers to send a message or read a sensor.

  A component's header is the truth; this file is a copy of it, and where a change to a header has an
  obvious place to be reflected. Keep them together.

  Third-party components are not documented here. Vendored `lvgl` keeps upstream's surface, and `ramen`
  is upstream `Zubax/ramen` whole; what this library adds on top of each (`lvgl_display`, nothing but the
  header for `ramen`) is here.
-->

Two contracts hold across every component here, and knowing them is most of reading a signature:

- **`bool` is the contract, and `false` is a refusal — never a log line.** A component in this library
  does not print; it returns `false` and lets the caller decide what that is worth. For a *reading* it
  means more than "an error": a value that cannot be trusted — a failed checksum, a stopped oscillator, a
  range the driver cannot scale — is reported as **no reading** rather than as a number to average.
- **A component takes the bus *handle*, never the address or the pins.** The bus, its speed, the pins and
  the device address are facts about a **board**, and the application states them. A component is handed
  what came back from opening the bus, so a bare `0x40` where a handle is wanted does not compile.

The lifecycle is the same everywhere, and is stated once in `SPIRE.md`: an object is **constructed with
its configuration**, `init()` takes no arguments and returns `false` to refuse, and `shutdown()` releases
what it holds. Start in the order things were created; stop in reverse.

## The bus seam

Each driver reaches its device through a handful of free functions declared in its own
`<name>_bus.hpp`, inside the driver's namespace. This is the component's entire interface to the bus, and
the **only** place IDF's driver API appears: a host test compiles the same header against a *fake* with
the same names, records what the protocol wrote and answers from a script — no board, no chip, no IDF.

There are two shapes, one per bus. The I²C parts (`sht20`, `sps30`, `bm8563`, `axp2101`, `bmi270`,
`ft6336`) declare:

```cpp
using BusHandle = i2c_master_dev_handle_t;

bool bus_write(BusHandle device, const uint8_t* out, std::size_t out_len);
bool bus_read(BusHandle device, uint8_t* in, std::size_t in_len);
bool bus_write_read(BusHandle device, const uint8_t* out, std::size_t out_len,
                    uint8_t* in, std::size_t in_len);
```

- `bus_write` — send `out_len` bytes. `false` when the bus refused the transfer.
- `bus_read` — receive exactly `in_len` bytes. `false` when the bus returned fewer.
- `bus_write_read` — send and receive as **one** exchange, with no STOP in between. The form a register
  read uses, and what makes one sample a single instant rather than a write and a read that could straddle
  a conversion. Reaching for `bus_write` followed by `bus_read` where a device wants one exchange is the
  most common way a driver works on one device and not on the next.

`sps30` adds one more, because its device needs a settling interval rather than a delay the caller guesses:

```cpp
void bus_delay_ms(std::uint32_t ms);
```

The SPI panels (`st7789`, `ili9342`) declare a two-function seam instead — a panel is written, never read
back:

```cpp
using BusHandle = spi_device_handle_t;

bool bus_command(BusHandle device, const uint8_t* out, std::size_t out_len);
bool bus_data(BusHandle device, const uint8_t* out, std::size_t out_len);
```

- `bus_command` — send a command word (and any parameters) with the data/command line low.
- `bus_data` — send pixel data (or a parameter stream) with it high.

`ili9342` also carries `bus_delay_ms` (`void bus_delay_ms(std::uint32_t ms)`).

`BusHandle` is a type alias **per component namespace**, so `sht20::BusHandle` and `bmi270::BusHandle` are
distinct names for the same IDF type: a component names its own alias and never an IDF type, which is what
lets one header compile on a host and on the chip.

## `toolkit` — `spire::Task`

`#include <task.hpp>`

One loop, on a FreeRTOS task of its own — the seam both application frameworks stand on. It is the only
thing `ramen` and `actors` have in common: a dataflow actor's loop pushes inline, so its stack has to
hold the whole chain of consumers, and a mailbox actor's loop receives and dispatches; neither is this
class's business. `std::thread` would also work, but it cannot say how much stack the loop needs or which
core it must share.

```cpp
namespace spire {

class Task {
public:
    Task();
    Task(const Task&) = delete;
    Task& operator=(const Task&) = delete;
    ~Task();

    bool start(const char* name,
               std::uint32_t stack_bytes,
               UBaseType_t priority,
               BaseType_t core,
               void (*body)(void*),
               void* arg);

    bool should_run() const;
    void stop();
    bool running() const;
};

}  // namespace spire
```

- `start(name, stack_bytes, priority, core, body, arg)` — start `body(arg)` on its own task. `stack_bytes`
  is what the loop actually needs (one that parses a frame wants far more than one that reads a register).
  `core` is `0`/`1` to pin, `tskNO_AFFINITY` to let the scheduler choose. Returns `false` when FreeRTOS
  refused (almost always: not enough heap for the stack). **`name` must outlive the task** — FreeRTOS keeps
  the pointer, not a copy.
- `should_run()` — the flag the loop polls: `while (task.should_run()) { ... }`. A loop that ignores it is
  a bug in that loop, and one `stop()` reports by hanging rather than detaching.
- `stop()` — ask the loop to finish and wait for it. Idempotent, and safe on a task that never started.
- `running()` — whether the task is up and not stopped.

## `ramen` — the dataflow framework

`#include <ramen.hpp>`

Upstream **`Zubax/ramen`** (MIT, C++20), vendored whole as a single header. Pushing is **synchronous and
inline**: a producer's `Task` runs every wired behaviour before its `Pusher` returns. There is no
scheduler, no mailbox and no runtime — one task runs a whole chain.

The four types an application wires:

```cpp
ramen::Pushable<T...>   // a behaviour: void(const T&...) — what receives a value
ramen::Pusher<T...>     // the event that pushes into it
ramen::Pullable<T...>   // a behaviour: void(T&...) — what fills a value
ramen::Puller<T...>     // the event that pulls from it
```

An event (`Pusher`/`Puller`) is linked to a behaviour (`Pushable`/`Pullable`) with **`>>`**, which points
from the producer to the consumer. The linked group is a **topic**: triggering any event on it runs the
behaviours, in the order they were linked. Linking after a producer's task has begun silently drops
everything pushed before the link was made — see `SPIRE.md`, "Wire before starting".

This is upstream's surface, not this library's: `SPIRE.md` describes the subset a product here uses, and
the vendored header carries the rest (`Ctor`, `Finalizer`, `PushCast`/`PullCast`, the `Nary` behaviours,
footprints). Upstream's own docs are at `github.com/Zubax/ramen`.

## `actors` — the classical actor framework

Five headers: `actor.hpp`, `actor_ref.hpp`, `mailbox.hpp`, `registry.hpp`, `scheduler.hpp`. An actor is a
mailbox, a task of its own and a message type; a sender holds an `ActorRef<Message>`, never the actor, so a
slow actor cannot stall a fast one and the order between two actors is a queue's order.

### `spire::Actor<Message, depth = 8>`

The base class an actor subclasses. `#include <actor.hpp>`

```cpp
namespace spire {

template <typename Message, std::size_t depth = 8>
class Actor {
public:
    static constexpr std::size_t mailbox_depth = depth;   // `depth`, as a compile-time fact

    Actor();
    Actor(const Actor&) = delete;
    Actor& operator=(const Actor&) = delete;
    virtual ~Actor() = default;

    ActorRef<Message> self() const;

protected:
    virtual void on_message(const Message& message) = 0;   // the one thing a subclass must write
    virtual bool init();                                   // { return true; } — false refuses
    virtual void shutdown();                               // {} — releases what it holds
};

}  // namespace spire
```

- `Message` must be **trivially copyable and default-constructible** (a `static_assert` says so): it is
  copied into a FreeRTOS queue, so a message that owned memory would be copied into a slot that outlives
  neither. Keep it plain.
- `mailbox_depth` — the actor's mailbox depth as a compile-time fact: `spawn` needs it before anything
  exists, and depth is a property of the actor rather than of the call site. A subclass states it as the
  second template argument — `class Counter : public spire::Actor<Add, 16>`.
- `self()` — this actor's own address, for what an actor does to itself: a retry, a follow-up, a deadline
  that expires into its own mailbox. Valid from `init()` onwards.
- `on_message(message)` — handle one message. Runs on the **actor's own task**, so it may take as long as
  it likes without holding anybody else up. Protected and pure virtual: the subclass writes it.
- `init()` — runs on the **caller's** task, before the mailbox or the task exists, so a refusal costs
  nothing and is reported as `false` rather than becoming a task that quietly does nothing. No arguments,
  because the actor was constructed with its configuration. This is also where an actor resolves peers
  through the registry.
- `shutdown()` — called once, on the caller's task, after the loop has ended.

(`Actor` also carries the `hook_init` / `hook_message` / `hook_shutdown` static function templates whose
addresses the scheduler takes. They are public because their *addresses* are taken; they are not meant to
be called by hand.)

### `spire::ActorRef<Message>`

A typed, shared handle to one actor's mailbox — the address you send to. `#include <actor_ref.hpp>`

```cpp
namespace spire {

template <typename Message>
class ActorRef {
public:
    ActorRef();

    bool valid() const;
    bool send(const Message& message, TickType_t wait = 0) const;
    std::size_t waiting() const;

    friend bool operator==(const ActorRef& a, const ActorRef& b);
};

}  // namespace spire
```

- The type is the **message**, not the actor: `ActorRef<Reading>` can be sent a `Reading` and nothing else,
  which is the whole of what a sender needs to know.
- `valid()` — whether the actor can still be sent to: it was spawned, its `init()` accepted, and it has
  not been stopped. A default-constructed ref is invalid — that is its whole state.
- `send(message, wait = 0)` — post a message, waiting up to `wait` ticks for room. A full mailbox is a
  **refusal, not a block**, by default: an actor that waits on a busy peer has given up the reason it is on
  a task of its own. A caller that would rather wait passes a timeout and means it.
- `waiting()` — how many messages are waiting to be handled.
- `operator==` — whether two refs address the same mailbox (the same actor, into the same queue).
- A ref is copyable, cheap, and safe to keep: it shares the mailbox, so it stays valid until the scheduler
  closes it — which is why an application may hold one past `stop()`.

### `spire::Mailbox`

One actor's mailbox: a FreeRTOS queue, and nothing else. Non-template on purpose — a queue does not care
what a message *means*, only how big it is, so the typed part of the framework is a thin wrapper over this
and the FreeRTOS calls live in `src/scheduler.cpp` rather than in a header. `#include <mailbox.hpp>`

```cpp
namespace spire {

class Mailbox {
public:
    Mailbox(std::size_t item_size, std::size_t depth);
    ~Mailbox();
    Mailbox(const Mailbox&) = delete;
    Mailbox& operator=(const Mailbox&) = delete;

    bool open();
    void close();
    bool is_open() const;
    bool post(const void* item, std::size_t size, TickType_t wait);
    bool receive(void* out, TickType_t wait);
    std::size_t waiting() const;
    std::size_t item_size() const;
    std::size_t depth() const;
};

}  // namespace spire
```

- `open()` — create the queue. Called once by the scheduler at the moment the actor is **spawned**, so a
  ref is a usable address before the system starts and a message posted early waits rather than being lost.
- `close()` — drop the queue. Every send and receive after this refuses, and the object stays alive for
  whoever still holds it. It is **reference-counted** rather than owned by the actor, deliberately: an
  `ActorRef` outliving the thing that spawned it is not an edge case, and a queue deleted underneath a live
  handle is a use-after-free discovered during shutdown.
- `post(item, size, wait)` — post `size` bytes, waiting up to `wait` ticks for room. `false` when the
  mailbox is closed or full, **and** when `size` is not this mailbox's item size — a wrong-typed
  `ActorRef`, a programming error a caller should hear about rather than watch messages vanish for.
- `receive(out, wait)` — take one item, waiting up to `wait` ticks. `false` on timeout or when closed.
- `waiting()` / `item_size()` / `depth()` — how many are queued, the slot size, and the queue's depth.

### `spire::Registry` — a name → mailbox index

The second way to reach an actor, beside the ref you were handed. `#include <registry.hpp>`

```cpp
namespace spire {

template <typename Message>
const void* message_tag();   // a process-unique tag for a message type

class Registry {
public:
    static std::shared_ptr<Registry> create();   // the one way a Registry comes into being

    Registry(const Registry&) = delete;
    Registry& operator=(const Registry&) = delete;

    void add(const char* name, const void* type_tag, std::shared_ptr<Mailbox> mailbox);

    template <typename Message>
    ActorRef<Message> get(const char* name) const;

    std::size_t size() const;
};

}  // namespace spire
```

- **Why it exists.** Two actors that must reach each other cannot both be *constructed* with the other's
  ref: one of them is always constructed first and its peer does not exist yet. The registry breaks that
  cycle **by name** — an actor is handed a `const Registry&` in its constructor, keeps it, and resolves its
  peers in `init()`, the one moment every actor exists at once.
- `message_tag<Message>()` — a **process-unique tag for a message type**, used to type a lookup. `sizeof`
  cannot answer "is this the right message type" (two structs of the same size are indistinguishable) and
  RTTI is off, so the tag is the address of one object per `Message` type.
- `create()` — the **one** way a `Registry` comes into being, and it is the `Scheduler` that calls it. There
  is deliberately no public constructor: a registry is never a singleton, never a stack temporary, and never
  something application code makes for itself.
- `add(name, type_tag, mailbox)` — register one actor's mailbox under the name it was spawned with. Called
  by the `Scheduler`; application code reads the index and does not write to it. A name registered twice is
  the last one registered.
- `get<Message>(name)` — resolve a peer by name, **checked against the message type** it was registered
  with. An unknown name, or a name that takes a different message, is an **invalid ref** (`valid()` is
  false, `send()` fails) — which is how a wiring mistake becomes a refusal in `init()` rather than
  corruption later.
- `size()` — how many names are registered (the actor count, once the system has been spawned).
- The scheduler fills it at `spawn` and `spawn` is refused once the system has started, so the index is
  written on one thread before the system runs and read-only afterwards. It needs no lock for the same
  reason a constant does.

### `spire::Scheduler`

Owns the actors, and therefore owns the thing that has to be stopped. It starts them in the order they were
spawned and stops them in reverse. `#include <scheduler.hpp>`

```cpp
namespace spire {

class Scheduler {
public:
    Scheduler();
    explicit Scheduler(std::shared_ptr<Registry> registry);
    ~Scheduler();
    Scheduler(const Scheduler&) = delete;
    Scheduler& operator=(const Scheduler&) = delete;

    template <typename Message, typename A, typename... Args>
    ActorRef<Message> spawn(const char* name,
                            std::uint32_t stack_bytes,
                            UBaseType_t priority,
                            BaseType_t core,
                            Args&&... args);

    bool start();
    void stop();
    bool running() const;
    std::size_t size() const;
    const std::shared_ptr<Registry>& registry() const;
};

}  // namespace spire
```

The whole lifecycle, in order:

```cpp
std::atomic<int> total{0};
spire::Scheduler scheduler;                                    // owns them, stops them in reverse
auto counter = scheduler.spawn<Add, Counter>("counter", 4096, 5, tskNO_AFFINITY, total);
scheduler.start();                                             // nothing runs before this
counter.send(Add{3});
scheduler.stop();
```

- `Scheduler(registry)` — share an index the caller already holds (the same one it hands to its actors)
  rather than the one `Scheduler()` makes for itself. A null pointer means "make one", so `registry()` is
  never null and always has something to hand back.
- `spawn<Message, A>(name, stack_bytes, priority, core, args...)` — create an actor `A`, give it a mailbox,
  and return the address to send to. `Message` is the actor's message type and `A` its type; the rest is
  `A`'s constructor arguments, so an actor that needs a peer's ref or a board constant is constructed with
  it rather than reaching for a global. `spawn` is a **no-op returning an invalid ref** once the system has
  started: starting one actor in the middle of a running system is a lifecycle nobody asked for, and
  refusing is how a caller finds out. It also registers the actor's name in the `Registry`.
- `start()` — start every actor, in the order it was spawned. **All or nothing**: if one refuses (`init()`
  says no, or FreeRTOS could not give it a task), the ones already started are stopped in reverse before
  `false` is returned. A refused start must not leave half a system running.
- `stop()` — stop every actor in **reverse** the order it was spawned, then release them. Idempotent. A
  scheduler is not restartable: `stop()` closes the mailboxes, so refs held past it go invalid rather than
  dangling.
- `running()` — whether the system has been started and not stopped.
- `size()` — how many actors were spawned.
- `registry()` — the shared name → mailbox index, filled as actors are spawned. An application reads it to
  hand to its actors (at `spawn`, by dereferencing it), which resolve their peers through it in `init()`.

**Order.** Spawn the receiver before its sender and hand the ref over — `auto b = ...; auto a = ...` — which
is also the order they must start in. And spawn and wire everything **before** `start()`: a message posted
early waits in its mailbox, which is what makes wiring order irrelevant and a half-built system unable to
run half-built.

## The drivers

Eight device drivers, one chip each — board-neutral, host-testable, and framework-free. Each owns its bus
handle, holds only the **part's** or **panel's** own facts, names no actor, no task and no pin, and reaches
its device through the seam above and nothing else. Each reports failure by returning `false`.

### `sht20::Sht20` — humidity and temperature

A Sensirion **SHT20**, the SHT2x family's part, read by command over I²C. It reports two measurements, each
triggered by its own command: a temperature in degrees Celsius and a relative humidity in percent. Nothing
above this layer — a rolling mean, a dew point, a decision — is this component's business.
`#include <sht20.hpp>`

```cpp
namespace sht20 {

class Sht20 {
public:
    explicit Sht20(BusHandle device);

    Sht20(const Sht20&) = delete;
    Sht20& operator=(const Sht20&) = delete;

    bool probe();
    bool temperature(float& celsius);
    bool humidity(float& relative_humidity);
};

}  // namespace sht20
```

- `Sht20(device)` — wrap the bus handle a board has already opened and added at the part's address.
- `probe()` — whether the sensor is really there: reads its user register and sees whether the device
  answers. The SHT2x family exposes **no chip-id register**, so this reports **presence, not identity**: a
  device answered at the address. An absent part and a bus that will not answer both read `false`.
- `temperature(celsius)` — measure the temperature in °C into `celsius`. `false` when the exchange failed or
  the reply's checksum did not match — a reading that cannot be trusted is not a reading. The sensor's own
  **hold-master** timing is inside this call: it blocks for the measurement, so the value is complete when
  this returns.
- `humidity(relative_humidity)` — the relative humidity in percent into `relative_humidity`, on the same
  terms: a failed exchange, or a reply whose checksum did not match, reads `false`.

### `sps30::Sps30` — particulate matter

A Sensirion **SPS30**, read by command over I²C. Unlike a one-shot sensor it is **stateful**: a sample is
produced on *its* clock, not on the caller's, so reading one is a loop — start measuring once, watch for a
sample to be ready, then take it. The driver refuses to hide that loop behind a single call, because *when*
to sample is the product's decision. `#include <sps30.hpp>`

```cpp
namespace sps30 {

class Sps30 {
public:
    explicit Sps30(BusHandle device);

    Sps30(const Sps30&) = delete;
    Sps30& operator=(const Sps30&) = delete;

    /// One sample: the ten values the device reports, in the datasheet's order and units.
    struct Measurement {
        float mass_pm1              = 0.0f;  ///< Mass concentration, PM1.0   [µg/m³]
        float mass_pm2_5            = 0.0f;  ///< Mass concentration, PM2.5   [µg/m³]
        float mass_pm4              = 0.0f;  ///< Mass concentration, PM4.0   [µg/m³]
        float mass_pm10             = 0.0f;  ///< Mass concentration, PM10    [µg/m³]
        float number_pm0_5          = 0.0f;  ///< Number concentration, PM0.5 [#/cm³]
        float number_pm1            = 0.0f;  ///< Number concentration, PM1.0 [#/cm³]
        float number_pm2_5          = 0.0f;  ///< Number concentration, PM2.5 [#/cm³]
        float number_pm4            = 0.0f;  ///< Number concentration, PM4.0 [#/cm³]
        float number_pm10           = 0.0f;  ///< Number concentration, PM10  [#/cm³]
        float typical_particle_size = 0.0f;  ///< Typical particle size        [µm]
    };

    bool probe();
    bool start_measurement();
    bool stop_measurement();
    bool data_ready();
    bool read(Measurement& measurement);
};

}  // namespace sps30
```

- `probe()` — whether the sensor is really there: reads its **serial number** and checks the reply's
  checksums. Unlike the SHT2x, this part *has* an identity, so a valid reply is evidence the device at the
  address is an SPS30 and not merely *something*. A silent bus, a short reply and a corrupt one all read
  `false`.
- `start_measurement()` / `stop_measurement()` — begin or stop measuring, each waiting out the device's own
  settling interval (through `bus_delay_ms`) before returning. `false` when the command was refused.
  Starting an already-measuring device is harmless — the command simply restarts the loop.
- `data_ready()` — whether a fresh sample is waiting to be read; poll it while measuring. `false` means
  "nothing to read this time" — either the device has not produced its next sample or the bus would not
  answer — and the caller's response to both is the same: do not read, try again next tick. A caller that
  must tell "not ready" from "gone" apart uses `read()`, which fails only on a broken exchange.
- `read(measurement)` — take the ready sample into `measurement`, its checksums verified. All ten values are
  decoded together (the frame carries twenty words, a checksum after each). `false` when the exchange
  failed, when the reply was too short, or when any checksum did not match: a sample that cannot be trusted
  is not a sample. On a failed read `measurement` is left as it was, never half-written. Read only when
  `data_ready()` said so — reading early returns the previous sample, which the device keeps until the next
  one replaces it.
- `Measurement` — one sample: ten `float`s in the datasheet's order and units, four mass concentrations in
  µg/m³, five number concentrations in #/cm³, and a typical particle size in µm.

### `bm8563::Bm8563` — real-time clock

A **BM8563** (PCF8563-compatible) battery-backed wall clock over I²C. It remembers the time that has passed
since it was last set, kept by an oscillator of its own that a small cell runs while the board has no power.
It is **not** a network time source: it neither knows the real date nor can learn it — where the time comes
from (an SNTP client, a console, a default) is the application's decision, handed in through `set()`.
`#include <bm8563.hpp>`

```cpp
namespace bm8563 {

struct DateTime {
    int year    = 0;  ///< Full year, century included — 2026, not 26.
    int month   = 1;  ///< 1..12.
    int day     = 1;  ///< 1..31, the day of the month.
    int weekday = 0;  ///< 0..6, a day number the application assigns; the part defines no mapping.
    int hour    = 0;  ///< 0..23.
    int minute  = 0;  ///< 0..59.
    int second  = 0;  ///< 0..59.
};

class Bm8563 {
public:
    explicit Bm8563(BusHandle device);

    Bm8563(const Bm8563&) = delete;
    Bm8563& operator=(const Bm8563&) = delete;

    bool probe();
    bool now(DateTime& when);
    bool set(const DateTime& when);
};

}  // namespace bm8563
```

- `DateTime` — a point on the calendar in the terms a caller thinks in, not in the bytes the part stores. The
  **year is the full one**, century included, because the part keeps the century as a bit rather than a
  number and a caller that has to add 1900 or 2000 itself is doing the driver's job. `weekday` is **the
  part's and nobody else's**: the part stores a day number without saying which day is zero, so that
  assignment is the application's, by convention. A caller that does not care reads the calendar fields and
  ignores it.
- `probe()` — reads a control register and sees whether the device answers. The part exposes **no chip-id
  register**, so this is **presence, not identity**. An absent part and a bus that will not answer both read
  `false`.
- `now(when)` — read the clock into `when`. Returns `false` when the exchange failed, **and** when the part
  says the time is not to be trusted: its **voltage-low** flag is set, so the oscillator stopped since the
  clock was last set and the fields are wrong. Both are one `false` because the caller does the same thing
  with each — get the time another way and call `set()`. A caller that ignores the return value and reads
  `when` anyway is reading a time the driver cannot vouch for. The whole clock is **one** exchange (the
  address pointer auto-increments), so it is one instant rather than a sequence that could straddle a tick.
- `set(when)` — set the clock from `when` and start it counting from there. This is also what makes `now()`
  trustworthy again: writing the time **clears the voltage-low flag**, so a caller that saw `now()` fail can
  set the clock from a trustworthy source and carry on. `false` when the bus refused the write, in which case
  the clock still holds its previous, untrusted value.

### `axp2101::Axp2101` — power management

An **AXP2101** power-management IC over I²C — the PMIC that stands between the cell, the charger and the
rails. It decides what is powered, charges the battery when the cable is in, and *measures*: battery
voltage, the charge it is putting back, the battery's own gauge. The state machine that makes those numbers
mean something is the product's. `#include <axp2101.hpp>`

```cpp
namespace axp2101 {

class Axp2101 {
public:
    explicit Axp2101(BusHandle device);

    Axp2101(const Axp2101&) = delete;
    Axp2101& operator=(const Axp2101&) = delete;

    bool probe();
    bool battery_connected(bool& connected);
    bool charging(bool& charging);
    bool battery_voltage_mv(std::uint16_t& millivolts);
    bool battery_percent(int& percent);
};

}  // namespace axp2101
```

- It **reads** and it does not **configure**: bring-up — which rails come up at what voltage, the charge
  current, the watchdog — is the board's, and a driver that silently rearranged a board's power tree would
  be one no board could adopt. Nothing here turns a rail on.
- `probe()` — whether this really is an AXP2101: reads its ID register (`0x03`) and checks the value. Unlike
  the sensor and clock drivers beside it, this part **identifies itself**, so this reports **identity**, not
  mere presence: a different XPowers part answering at the address reads `false` rather than being mistaken
  for this one.
- `battery_connected(connected)` — whether a battery is connected, written to `connected`. Returns `false`
  **only** when the exchange failed: a battery that is absent is a successful reading of *no battery*, not a
  failure. A caller deciding whether to run on the cell wants the answer, not an error.
- `charging(charging)` — whether the part is charging, written to `charging`. `charging == false` covers
  everything the part reports instead — standing by with the cable in, and discharging with the cable out —
  because they call for the same response from a thin client: wait, or run on the cell.
- `battery_voltage_mv(millivolts)` — the battery voltage in millivolts. The part's ADC measures in
  millivolts, so there is no scale to apply and no unit to guess. With no battery fitted the measurement is
  meaningless — the caller checks `battery_connected()` first rather than the driver inventing a value.
- `battery_percent(percent)` — the battery's own gauge in percent, as the part computes it; the driver
  neither smooths nor corrects it. Only meaningful while a battery is fitted and the gauge has learned one,
  which is why it is a reading beside the voltage rather than a derived *state of charge*.

### `bmi270::Bmi270` — inertial measurement

A **BMI270** six-axis IMU and its own temperature, over I²C: three axes of acceleration and three of
rotation, each a signed sixteen-bit count the part produces from its own MEMS elements. What those counts
mean — a tilt, a tap, a step — is the product's. `#include <bmi270.hpp>`

```cpp
namespace bmi270 {

class Bmi270 {
public:
    /// One reading of all six axes, in the units the datasheet's conversions give.
    struct Sample {
        float accel_x;
        float accel_y;
        float accel_z;
        float gyro_x;
        float gyro_y;
        float gyro_z;
    };

    explicit Bmi270(BusHandle device);

    Bmi270(const Bmi270&) = delete;
    Bmi270& operator=(const Bmi270&) = delete;

    bool probe();
    bool read(Sample& sample);
    bool temperature(float& celsius);
};

}  // namespace bmi270
```

- It **reads** and it does not **configure**: bringing the part up — out of suspend, its power mode, the
  full-scale ranges, and Bosch's configuration file — is the board's. A part left in its reset state still
  *answers* and still returns counts; what it will not do is *change* them, because its sensors are not
  running. That distinction is the caller's to make, and the driver does not paper over it.
- `Sample` — acceleration in **g** and rotation in **degrees per second**, each scaled by the full-scale
  range the part is actually set to (read from the part rather than assumed), so the numbers mean the same
  thing whichever range the board chose.
- `probe()` — whether this really is a BMI270: reads its chip-ID register (`0x00`) and checks the value.
  The part **identifies itself**, so this reports **identity**, not mere presence.
- `read(sample)` — the six axes into `sample`. The accelerometer's and gyroscope's ranges are read from the
  part first, because the full scale is a *setting* — the board's, not the driver's — and the same count is
  a different number of g per range. The counts come out of the twelve consecutive data registers in a
  single exchange, so the six axes are one instant rather than six that could straddle a sample. Returns
  `false` when an exchange failed, **and** when a range register holds a value this driver does not know: a
  reading it cannot scale is not a reading.
- `temperature(celsius)` — the part's own **die** temperature in °C. A fine check that the part is answering
  and warmed up, and **not** an ambient reading: the part warms itself, so the number runs above the room.

### `st7789::St7789` — SPI panel

An **ST7789** panel controller driven over SPI, command by command. It is a *device*, not a screen: it knows
the panel's geometry and the addressing window a rectangle of pixels goes into. It holds the **panel's**
facts — geometry and colour order are properties of the ST7789, the same on every board that fits one — and
takes no board facts: the reset line, the data/command line, the backlight and the clock speed are the
application's. `#include <st7789.hpp>`

```cpp
namespace st7789 {

class St7789 {
public:
    static constexpr std::uint16_t kWidth  = 240;   ///< the panel's dimensions, in pixels, portrait
    static constexpr std::uint16_t kHeight = 320;

    explicit St7789(BusHandle device);

    St7789(const St7789&) = delete;
    St7789& operator=(const St7789&) = delete;

    bool init();
    bool flush(std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
               const std::uint8_t* pixels);
};

}  // namespace st7789
```

- `kWidth` / `kHeight` — the **panel's** dimensions in pixels, portrait: the ST7789 is the same panel on
  every board. A board's rotation is the board's, applied by whoever draws, not a second panel.
- `init()` — bring the panel up — the power-on sequence, the pixel format, the addressing window — and leave
  it in a state where `flush()` means something, answering only if the panel is really there rather than
  trusting that a bus handle exists.
- `flush(x0, y0, x1, y1, pixels)` — push one rectangle of pixels: `(x1 - x0 + 1) * (y1 - y0 + 1)` of them,
  **two bytes each**, big-endian **RGB565** — the panel's format, and the reason this signature names a
  colour order rather than choosing one. The corners are inclusive; the addressing window is the driver's to
  set, and the caller hands pixels and a rectangle and nothing else. `pixels` is not copied.
- **A stub, deliberately.** This header's `init()` carries a `TODO`: the command words, the parameter each
  one carries, the delay between them, and the frame the panel is left in are the panel's own and come off
  its datasheet. The signature, the geometry and the colour contract are settled; the command table is not.
  This is the one driver here whose body is still to be written, and the shape above is what it will keep.

### `ili9342::Ili9342` — SPI panel

An **ILI9342** panel controller driven over SPI — the CoreS3's own. Like the ST7789 driver it is a *device*,
not a screen: it knows the panel's geometry, the frame the panel must be left in, and the addressing window
a rectangle of pixels goes into. It does **not** scale or rotate: a caller that wants landscape draws in
landscape, and the driver takes the rectangle it is given. `#include <ili9342.hpp>`

```cpp
namespace ili9342 {

/// Which revision of the panel this is.
enum class Revision {
    C,  ///< the vendor's stock sequence
    E,  ///< the vendor's sequence plus the E revision's extra tuning
};

class Ili9342 {
public:
    static constexpr std::uint16_t kWidth  = 320;   ///< in the orientation the panel's MADCTL leaves it
    static constexpr std::uint16_t kHeight = 240;

    explicit Ili9342(BusHandle device, Revision revision);

    Ili9342(const Ili9342&) = delete;
    Ili9342& operator=(const Ili9342&) = delete;

    bool init();
    bool flush(std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
               const std::uint8_t* pixels);
};

}  // namespace ili9342
```

- `Revision` — which revision of this panel the board carries. The same glass — the CoreS3's 320x240 IPS —
  ships with an ILI9342C or an ILI9342E behind it, and the two want different power-on tables: the C is
  happy with the vendor's stock sequence, and the E is not. That is a fact about the *panel*, so both
  sequences live in this driver. *Which* one is fitted is a fact about the *board* and is the application's
  to state, so the driver is **told** rather than guessing: an init that picked a table by trying one and
  seeing is an init that cannot tell a wrong table from a dead panel.
- `Ili9342(device, revision)` — wrap the bus handle a board has already opened. `revision` is required.
- `init()` — bring the panel up: the power-on sequence for the given revision, the pixel format, and the
  frame the panel is left in. Returns `false` the moment the bus refuses a transfer. A display's seam is
  write-only, so there is no id to read back and no honest way to distinguish "not fitted" from "not
  answering": both are reported the same way, and the caller decides what an unresponsive screen is worth.
  Safe to call twice — a second bring-up is another reset-and-init, which the panel tolerates.
- `flush(x0, y0, x1, y1, pixels)` — push one rectangle of pixels, `(x1 - x0 + 1) * (y1 - y0 + 1)` of them,
  **two bytes each**, big-endian **RGB565**. The corners are **inclusive** and the addressing window is the
  driver's to set. `pixels` is not copied and must outlive the call — the transfer is synchronous, so
  "outlive" means until this returns. Returns `false` on a rectangle that is inside out, a null buffer, or a
  bus that refuses.

### `ft6336::Ft6336` — capacitive touch

A FocalTech **FT6336** capacitive touch controller over I²C, as fitted to the CoreS3's panel. It reports
**one point** — whether a finger is down and, when it is, its coordinate — and recognises no gestures on its
own: a tap, a swipe or a long press is a shape in a *stream* of these reports, and the timing that tells one
from another lives above this layer. `#include <ft6336.hpp>`

```cpp
namespace ft6336 {

/// One report from the panel: where a finger is, and whether there is one at all.
struct Touch {
    std::uint16_t x = 0;
    std::uint16_t y = 0;
    bool down = false;  ///< whether a finger is on the panel. `x`/`y` mean nothing when this is false.
};

class Ft6336 {
public:
    static constexpr std::uint16_t kWidth  = 240;   ///< the module's grid, in the panel's own units
    static constexpr std::uint16_t kHeight = 320;

    explicit Ft6336(BusHandle device);

    Ft6336(const Ft6336&) = delete;
    Ft6336& operator=(const Ft6336&) = delete;

    bool probe();
    bool read(Touch& touch);
};

}  // namespace ft6336
```

- `Touch` — where a finger is and whether there is one at all. The coordinates are the **controller's** — the
  panel's own grid, the same on every board that fits this module — not the display's. Mapping a point onto a
  widget is a different job, done above this layer.
- `kWidth` / `kHeight` — the module's grid in the panel's own units. `read()` reports the controller's own
  coordinate, which comes out of twelve-bit registers (`0x000`–`0xFFF`); these are the panel dimensions that
  grid is laid over, so a caller that needs panel units scales `x * kWidth / 4096`. The driver does not scale
  for you: a full-scale is an assumption about the panel, and a driver that assumed one could only be checked
  against it.
- `probe()` — whether the controller is really there: reads its chip id and compares it with the FT6336U's. A
  probe that returned true because a bus handle was passed would report success on a board where the
  controller is not fitted; a bus that will not answer reads `false` too.
- `read(touch)` — read the controller's touch registers and fill `touch` (the first point, when a finger is
  down). Returns `false` **only** when an exchange failed; it returns **true** with `Touch::down == false`
  when the panel was read successfully and simply has no finger on it. Collapsing those two into one `false`
  is how a dropped I²C transaction becomes a stuck cursor.

## Library pieces — neither framework nor device

### `rolling_average::RollingAverage`

A sliding-window average of `float` samples, where the **caller owns the storage**: it hands in a buffer of
`size` floats, and the object keeps a running sum and a count of what is currently in the window. It is a
library component in the strictest sense — pure, no global mutable state, no IDF type, no actor, no task, and
failure reported by returning `false` rather than by logging. `#include <rolling_average.hpp>`

```cpp
namespace rolling_average {

class RollingAverage {
public:
    bool init(float* buffer, std::size_t size);
    bool push(float sample);
    bool mean(float& out) const;
    bool reset();

    std::size_t count() const;
    std::size_t size() const;
};

}  // namespace rolling_average
```

- `init(buffer, size)` — bind the object to the caller's buffer. `buffer` must be non-null and `size` must be
  greater than zero. The buffer is **not owned** and must outlive this object. Returns `false` if `buffer` is
  null or `size` is zero.
- `push(sample)` — push a new sample into the window, evicting the oldest if it is full. Returns `false` if
  the object has not been initialised.
- `mean(out)` — read the mean of the samples currently in the window. Returns `false` if the object has not
  been initialised or the window is empty; otherwise writes the mean to `out` and returns `true`.
- `reset()` — clear the window, discarding every sample currently in it. The buffer binding and capacity are
  kept, so the object stays usable without a new `init()`. Returns `false` if the object has not been
  initialised.
- `count()` — number of samples currently in the window. `size()` — the window's capacity, as given to
  `init()`.
- A partial window is a window: `push()` once and `mean()` is that sample, not a division by the capacity —
  the mean divides by `count()`, the samples actually in it.

### `lvgl_display::Display` — the LVGL surface

The bridge between LVGL and a panel: one `lv_display`, the draw buffer LVGL renders into, and the `flush_cb`
that hands each changed rectangle to a **pixel sink** the application supplies. The seam is `PixelSink`, and
`sink_of(panel)` builds one from any device whose `flush()` has the panel's signature — so `st7789` stays a
driver and the decision that *this* panel is what LVGL draws on stays where board facts live: the
application. This component never names a panel, and requires none. It owns no task and no colour format: who
calls `render()`, how often and on which task is the application's (normally an actor of its own).
`#include <lvgl_display.hpp>`

```cpp
namespace lvgl_display {

/// Where a rectangle of pixels goes: the seam.
struct PixelSink {
    bool (*flush)(void* context, std::uint16_t x0, std::uint16_t y0,
                  std::uint16_t x1, std::uint16_t y1, const std::uint8_t* pixels);
    void* context;
};

/// Build a sink from any device whose `flush()` has the panel's signature.
template <class Device>
PixelSink sink_of(Device& device);

class Display {
public:
    /// The draw buffer, in panel rows — not in bytes.
    static constexpr std::size_t kDrawBufferRows = 32;

    Display(PixelSink sink, std::uint16_t width, std::uint16_t height);
    ~Display();

    Display(const Display&) = delete;
    Display& operator=(const Display&) = delete;

    bool init();
    void render();
    bool healthy() const;
    lv_display_t* handle() const;
    void shutdown();
};

}  // namespace lvgl_display
```

- `PixelSink` — **the seam**: one call and one context, no heap and no allocation — a sink is *made*, per
  board, at startup. The signature is the panel's own: corners inclusive, and `pixels` is
  `(x1 - x0 + 1) * (y1 - y0 + 1)` pixels in the display's colour format, which `lv_conf.h` names — RGB565,
  two bytes each, big-endian. Returning `false` is how a panel that stopped answering is reported: LVGL
  cannot be told, because its `flush_cb` must call `lv_display_flush_ready()` either way or LVGL waits for a
  flush that never comes, so the failure is *kept* for the loop that calls `render()` to notice.
- `sink_of(device)` — build a sink from any device whose `flush()` has the panel's signature
  (`st7789::St7789::flush()`). A template rather than a `std::function`, deliberately: what is stored is a
  function pointer and a context, nothing is allocated, and the device's *type* is resolved here instead of
  being named by this component. **The device must outlive the display.**
- `Display(sink, width, height)` — a display `width` x `height` **pixels**: the panel's geometry, not the
  board's rotation. A rotated screen is the same panel drawn in a different order, not a second display.
- `kDrawBufferRows` — the draw buffer, in **panel rows** (not bytes), because the width is geometry this
  component is already told. Thirty-two rows of a 240-pixel panel is 15 KiB: enough that the panel is still
  taking the last band while LVGL renders the next, which is what partial rendering (`LV_DISPLAY_RENDER_MODE_PARTIAL`)
  is for — and the difference between this and a screen-sized 150 KiB buffer on the S3.
- `init()` — bring up LVGL and this display: `lv_init()` (idempotent — LVGL is **process-global**, so a
  second display warns rather than re-initialising it), the tick, the display itself, the draw buffer and the
  flush callback. Returns `false` rather than logging, like every driver here. Safe to call twice — the
  second call is a no-op.
- `render()` — one `lv_timer_handler()` pass: timers, animations, and the redraw of whatever is dirty, which
  reaches the panel through the sink. Call it in a loop — LVGL answers with how long it would like to sleep,
  and ignoring that costs a busy screen, not correctness. Does nothing before `init()` and after `shutdown()`.
- `healthy()` — whether every flush so far has reached the panel. A display whose sink returned `false` is
  one whose screen is stale, and this is where that becomes visible.
- `handle()` — the `lv_display_t*` this owns, for what this component deliberately does not wrap: the active
  screen, a theme, or an `lv_indev` built from a touch panel (`lv_indev_set_display(..., handle())`).
  **`nullptr` before `init()`.**
- `shutdown()` — destroy the display and release the buffer. Idempotent, and safe on a display that never
  initialised.
- A flush **blocks** for as long as the transfer takes — the sink is called inline, and LVGL is told the
  flush is done when it returns. The cost is stated rather than hidden: while a band transfers, the task that
  called `render()` is inside this call. Because the seam is write-only and polling, an asynchronous flush
  would need a completion callback it does not have.
- This is the one component here with **no host test**: LVGL is vendored for the chip and compiled by this
  project's build, so there is no host build to link a case against — its `idf.py build` *is* the test. What
  a host case would substitute is the sink, which is why the seam is one function wide.
