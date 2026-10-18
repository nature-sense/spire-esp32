# CoreS3 (cores3) — board facts

Board name `cores3`: the M5Stack **CoreS3** (ESP32-S3). This document is the **wiring** — the pins,
the two chips on the I²C bus, and the lines that are not GPIOs. The panel's and touch controller's
protocols are their own documents (the `ili9342` driver, `ft6336.md`); what is here is what the
CoreS3 does with them.

In one line: the `cores3` board is an ESP32-S3 with an ILI9342 320×240 panel, an FT6336 touch
controller, an AW9523 expander and an AXP2101 PMU. The rest of this document is how they are wired.

Taken from `esp-bsp` `bsp/m5stack_core_s3` (`include/bsp/m5stack_core_s3.h`, `src/bsp_display.c`,
`src/bsp_feature_en.c`) — the authority this board's bring-up cites — and verified on hardware.

## I²C — one bus, two chips

- SCL `GPIO11`, SDA `GPIO12`.
- AW9523 I/O expander at `0x58` (`..._ADDRESS_00`).
- AXP2101 PMU at `0x34` (the BSP runs it at 400 kHz; the expander is happy at 100 kHz).

## SPI — the panel, shared with the microSD slot

SPI3 host: SCLK `GPIO36`, MOSI `GPIO37`, MISO `GPIO35`, CS `GPIO3`, 40 MHz, mode 0.

**The data/command line is `GPIO35`, the same pad as MISO**, because the panel's SDO is not wired.
Configure it `GPIO_MODE_INPUT_OUTPUT`: the bus enabled the pad's input for the card, and an
output-only mode would clear that enable and break the microSD. Drive it from the SPI
pre-transfer callback.

There is **no reset GPIO** (`BSP_LCD_RST = NC`, strapped high) and **no backlight GPIO**
(`BSP_LCD_BACKLIGHT = NC`). The only panel reset is the `SWRESET` the driver sends.

## Expander — the lines with no ESP32 pin

AW9523 outputs, both active high, driven before use:

- pin `9` — panel enable (`BSP_LCD_EN`); nothing works until it is high
- pin `0` — touch enable/power (`BSP_TOUCH_EN`); needed before probing `0xA6`

## PMU — the backlight is a regulator, not a PWM

AXP2101 `DLDO1`: level register `0x99`, value `(mV - 500) / 100`; 2.5 V at 1 %, 3.3 V at 100 %,
`0` = off. Enable register `0x90`, bit `0x80` (DLDO1 EN) — read it, then set the bit; the BSP only
ever sets it.

## The panel

ILI9342, 320×240 landscape, driven through the `ili9342` component. It takes **big-endian RGB565**:
the S3 is little-endian, so LVGL's draw buffer is `LV_COLOR_FORMAT_RGB565_SWAPPED`. Which revision
(C or E) is read from the touch controller at `0xA6` — see `ft6336.md`; the panel driver cannot
discover it itself.

## Touch

FT6336 at `0x38`, powered by expander pin `0`; protocol in `ft6336.md`. Its coordinates are panel
pixels on this board — no swap, no scaling.

## Bring-up order

1. expander pin 9 high (panel enable);
2. SPI3 bus, panel device, DC pin;
3. expander pin 0 high, then probe `0xA6` for the revision;
4. construct the panel driver and run its init;
5. set the backlight rail.

Everything above is the `cores3` board's: its pins, its expander lines and its rail. A part's protocol
is in that part's document.
