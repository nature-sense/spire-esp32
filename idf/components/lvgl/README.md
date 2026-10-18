# `lvgl` — vendoring wrapper

LVGL is the display library the applications draw their screens with. This component adds nothing to
it: it **pins** it, and it is where the library's LVGL configuration lives.

**The component that turns it into a screen is `lvgl_display`** — the `lv_display`, the draw buffer and
the `flush_cb`, over a panel driver the application hands it. This one is only the vendoring: nothing
here calls LVGL, so a product requires `lvgl_display` and never this.

## Why pinned, not fetched

A component fetched from a registry at configure time is a component whose build needs the network. On
a bench with no wifi that is a build that fails for a reason that has nothing to do with the firmware.
So LVGL is a **git submodule** at `components/lvgl/upstream`, standing at one tag.

The pin has exactly one home, and it is not this file or the `CMakeLists.txt`: it is the gitlink git
records in this repository's index. The version is deliberately repeated nowhere else, because a second
copy of a pin is a fact that can disagree with the source actually compiled.

```sh
git submodule status components/lvgl/upstream      # read the pin back; a leading - = not checked out
```

## Fetching it, and moving the pin

A fresh clone needs the submodule before it can build a display:

```sh
./tools/vendor-lvgl.sh              # check the submodule out — what a fresh clone needs
./tools/vendor-lvgl.sh --status     # the pinned commit
./tools/vendor-lvgl.sh v9.6.1       # move the pin to a tag, and stage the new gitlink
```

`v9.6.0` is the version this library is written against. After moving the pin, commit
`components/lvgl/upstream` — the gitlink — or the new pin is not real.

A checkout that never fetches the submodule still **builds**: the component registers nothing until
`upstream/` is populated, so a missing submodule costs a display, not a failed build. And when it *is*
populated, prove it compiles — the half a machine without a toolchain cannot:

```sh
idf.py set-target esp32s3 build
```

## The configuration

LVGL does not decide its own configuration; it needs an `lv_conf.h` on the include path, and defining
`LV_CONF_INCLUDE_SIMPLE` is what lets that header be found by name. That header is
`components/lvgl/include/lv_conf.h` — **this library's**, version-controlled, and the file to edit when
a display needs a different colour depth, more heap, or a font. It sets only the settings the library
actually decides; LVGL fills every other symbol with its own default, so it is short on purpose.

### The `Kconfig`, and why it is not LVGL's full menu

The wrapper carries a `Kconfig` beside this file, and ESP-IDF discovers it automatically. It defines
exactly one family of symbols: the `LV_USE_*` external decoders (`LZ4`, `LIBPNG`, `LIBJPEG_TURBO`,
`FREETYPE`, `FFMPEG`, `LIBWEBP`, `RLOTTIE`, `THORVG`, `TINY_TTF`) and the two `_INTERNAL` toggles
(`LV_USE_LZ4_INTERNAL`, `LV_USE_THORVG_INTERNAL`) that say "use the bundled copy". All are `n`
except the two bundled-library toggles, which default `y` exactly as upstream's do.

That file exists for **dependency resolution, not for configuration**. A board support package such as
`espressif/m5stack_core_s3` pulls `esp_lvgl_port` and optional decoder components, and the component
manager guards each one with a rule on a `CONFIG_LV_USE_*` symbol *before* the project is configured.
With no `Kconfig` at all, those symbols were undefined and a BSP could not be configured — the failure
read `Missing required kconfig option after retry`. Defining the symbols, even as `n`, is what lets a
BSP coexist with the vendored LVGL.

It is deliberately **not** a copy of `upstream/Kconfig`. The upstream file is a generated, ~3,500-line
menu, and `upstream/` is a submodule that may not be checked out; a Kconfig that sources it would fail
the configure step on a clone that never ran `tools/vendor-lvgl.sh`, which is the one outcome this
wrapper exists to prevent.

**`lv_conf.h` remains the authority for what is compiled.** Enabling one of the symbols above does not,
by itself, make LVGL decode that format: the vendored tree is compiled with `lv_conf.h`, so a decoder is
wired in by naming its `LV_USE_*` there and its component in this wrapper's `PRIV_REQUIRES`. Saying that
out loud is the point — the Kconfig answers the component manager; `lv_conf.h` answers the compiler.

## Why a submodule, and not a committed tree

A v9.6.0 checkout is about 200 MB and 6600 files, and this repository does not need to carry a byte of
it: the gitlink is a few bytes, a clone stays small, and whoever builds fetches the source from the
same tag. `git clone --recurse-submodules` gets the source in the same step; without it the display is
simply absent until `tools/vendor-lvgl.sh` is run.
